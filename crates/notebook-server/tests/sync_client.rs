//! The client sync engine against the real server: two engines with the
//! native transport edit one note concurrently, converge, and see a purge.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures_util::StreamExt;
use notebook_core::auth::api::{SessionKind, Token};
use notebook_core::crypto::VaultId;
use notebook_core::note::{NoteId, Timestamp};
use notebook_core::protocol::{Bytes, OpId, PushRequest, PushUpdate};
use notebook_core::storage::{MemoryStorage, Storage};
use notebook_core::sync::fake::vault_keys;
use notebook_core::sync::{
    AccessToken, LiveChannel, NativeTransport, RefreshError, Replica, SessionSource, Shared,
    SyncConfig, SyncEngine, SyncHandle, Timer, TokioTimer, Transport, TransportError,
};
use notebook_server::app::{self, AppState};
use notebook_server::auth::sessions;
use notebook_server::auth::store::{NewAccount, create_account};
use notebook_server::auth::{keyfile, unix_now};
use notebook_server::config::Config;
use notebook_server::db;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use tokio::task::LocalSet;

const VAULT: [u8; 16] = [7; 16];

fn vault() -> VaultId {
    VaultId::from_bytes(VAULT)
}

fn now() -> Timestamp {
    Timestamp(unix_now() * 1000)
}

/// Issues sessions straight from the server's database.
#[derive(Clone)]
struct DbSession {
    pool: SqlitePool,
    timer: TokioTimer,
    token: Rc<RefCell<Option<AccessToken>>>,
}

impl DbSession {
    async fn new(pool: &SqlitePool, timer: TokioTimer) -> Self {
        let session = Self {
            pool: pool.clone(),
            timer,
            token: Rc::new(RefCell::new(None)),
        };
        session.refresh().await.unwrap();
        session
    }
}

impl SessionSource for DbSession {
    fn access_token(&self) -> Option<AccessToken> {
        self.token.borrow().clone()
    }

    async fn refresh(&self) -> Result<(), RefreshError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|_| RefreshError::Unavailable)?;
        let issued = sessions::issue(
            &mut conn,
            &[1; 16],
            &VAULT,
            "device",
            SessionKind::Native,
            unix_now(),
        )
        .await
        .map_err(|_| RefreshError::Unavailable)?;
        *self.token.borrow_mut() = Some(AccessToken {
            token: issued.access_token,
            // Access tokens live 15 minutes.
            expires_at: self.timer.now() + 14 * 60 * 1000,
        });
        Ok(())
    }
}

/// A native transport whose first HTTP push response is lost after the
/// server stored it, and whose live channel is refused, so pushes go over
/// HTTP.
#[derive(Clone)]
struct LosesFirstAck {
    inner: NativeTransport,
    lost: Rc<RefCell<bool>>,
}

impl Transport for LosesFirstAck {
    type Live = <NativeTransport as Transport>::Live;

    async fn push(
        &self,
        token: &Token,
        request: &PushRequest,
    ) -> Result<notebook_core::protocol::PushResponse, TransportError> {
        let response = self.inner.push(token, request).await?;
        if !std::mem::replace(&mut *self.lost.borrow_mut(), true) {
            return Err(TransportError::Network("response lost".into()));
        }
        Ok(response)
    }

    async fn changes(
        &self,
        token: &Token,
        after: u64,
    ) -> Result<notebook_core::protocol::ChangesResponse, TransportError> {
        self.inner.changes(token, after).await
    }

    async fn upload_snapshot(
        &self,
        token: &Token,
        note: NoteId,
        covers: u64,
        data: &[u8],
    ) -> Result<(), TransportError> {
        self.inner.upload_snapshot(token, note, covers, data).await
    }

    async fn purge(&self, token: &Token, note: NoteId) -> Result<(), TransportError> {
        self.inner.purge(token, note).await
    }

    async fn connect_live(&self) -> Result<Self::Live, TransportError> {
        Err(TransportError::Network("live channel refused".into()))
    }
}

struct Device {
    replica: Shared<Replica<MemoryStorage>>,
    handle: SyncHandle,
}

impl Device {
    async fn start<T: Transport + 'static>(pool: &SqlitePool, transport: T) -> Self {
        let timer = TokioTimer::new();
        let session = DbSession::new(pool, timer).await;
        let [key] = vault_keys(vault());
        let replica = Shared::new(Replica::open(MemoryStorage::new(vault()), key).unwrap());
        let (engine, handle, mut events) = SyncEngine::new(
            replica.clone(),
            transport,
            timer,
            session,
            SyncConfig::default(),
            Box::new(|| Ok(MemoryStorage::new(vault()))),
        );
        tokio::task::spawn_local(engine.run());
        tokio::task::spawn_local(async move { while events.next().await.is_some() {} });
        Self { replica, handle }
    }

    fn body(&self, id: NoteId) -> Option<String> {
        self.replica.lock().note(id).map(|n| n.body().to_owned())
    }
}

/// Polls `check` until it holds, for up to 10 seconds.
async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn stored_copies(pool: &SqlitePool, op: OpId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ops WHERE op_id = ?")
        .bind(op.to_bytes().as_slice())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn native_engines_converge_retry_and_purge() {
    let config = Config::from_sources(None, |k| {
        (k == "NOTEBOOK_PUBLIC_URL").then(|| "http://localhost".to_owned())
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("db")).await.unwrap();
    db::migrate(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    create_account(
        &mut tx,
        &NewAccount {
            user_id: [1; 16],
            username: "alice",
            opaque_record: b"r",
            recovery_verifier_hash: [0; 32],
            is_admin: false,
            vault_id: VAULT,
            key_bundle: b"b",
            now: unix_now(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let state = AppState::new(pool.clone(), config, keyfile::generate());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(app::run(
        listener,
        app::router(state),
        std::future::pending(),
    ));

    LocalSet::new()
        .run_until(async {
            let a = Device::start(&pool, NativeTransport::new(&url).unwrap()).await;
            let b = Device::start(&pool, NativeTransport::new(&url).unwrap()).await;

            // A creates; B gets it over the live channel.
            let id = a.replica.lock().create_note("base", now()).unwrap();
            eventually("b to get the note", || b.body(id).is_some()).await;

            // Concurrent edits to the same note converge with both kept.
            a.replica.lock().set_body(id, "A: base", now()).unwrap();
            b.replica.lock().set_body(id, "base :B", now()).unwrap();
            eventually("the edits to converge", || {
                let (x, y) = (a.body(id), b.body(id));
                x == y && x.as_deref() == Some("A: base :B")
            })
            .await;
            assert_eq!(a.handle.flush().await, Ok(()));
            assert_eq!(b.handle.flush().await, Ok(()));
            assert_eq!(a.replica.lock().notes(), b.replica.lock().notes());

            // A third engine pushes over HTTP and loses the first response
            // after the server stored it; the update is stored once.
            let c = Device::start(
                &pool,
                LosesFirstAck {
                    inner: NativeTransport::new(&url).unwrap(),
                    lost: Rc::new(RefCell::new(false)),
                },
            )
            .await;
            eventually("c to bootstrap", || c.body(id).is_some()).await;
            let from_c = c.replica.lock().create_note("from c", now()).unwrap();
            let op = {
                let replica = c.replica.lock();
                let outbox = replica_outbox(&replica);
                outbox[0]
            };
            eventually("c's note to reach a", || a.body(from_c).is_some()).await;
            eventually("c's outbox to empty", || {
                c.replica.lock().pending().unwrap() == 0
            })
            .await;
            assert_eq!(stored_copies(&pool, op).await, 1);

            // A raw retry of a stored push returns the same sequence number.
            let token = DbSession::new(&pool, TokioTimer::new())
                .await
                .access_token()
                .unwrap()
                .token;
            let transport = NativeTransport::new(&url).unwrap();
            let request = PushRequest {
                updates: vec![PushUpdate {
                    id: OpId::from_parts(1, 99),
                    note: id,
                    data: Bytes(
                        a.replica
                            .lock()
                            .storage()
                            .unwrap()
                            .load_note(id)
                            .unwrap()
                            .unwrap()
                            .entries[0]
                            .bytes
                            .clone(),
                    ),
                    note_key: None,
                }],
            };
            let first = transport.push(&token, &request).await.unwrap();
            let again = transport.push(&token, &request).await.unwrap();
            assert_eq!(first.seqs, again.seqs);
            assert_eq!(stored_copies(&pool, OpId::from_parts(1, 99)).await, 1);

            // The native live channel authenticates and is ready.
            let mut live = transport.connect_live().await.unwrap();
            live.send(&notebook_core::protocol::ClientFrame::Auth { token })
                .await
                .unwrap();
            assert!(matches!(
                live.next().await,
                notebook_core::sync::LiveEvent::Ready { .. }
            ));

            // A purge on A removes the note from B.
            a.replica.lock().set_trashed(id, true).unwrap();
            eventually("b to see the trash flag", || {
                b.replica.lock().note(id).is_some_and(|n| n.is_trashed())
            })
            .await;
            assert_eq!(a.handle.purge(id).await, Ok(()));
            eventually("b to drop the purged note", || b.body(id).is_none()).await;
            assert_eq!(
                b.replica.lock().storage().unwrap().load_note(id).unwrap(),
                None
            );
            assert!(a.body(id).is_none());

            for device in [&a, &b, &c] {
                device.handle.shutdown();
            }
        })
        .await;
}

/// The op IDs queued in `replica`'s outbox.
fn replica_outbox(replica: &Replica<MemoryStorage>) -> Vec<OpId> {
    replica
        .storage()
        .unwrap()
        .outbox()
        .unwrap()
        .into_iter()
        .map(|e| e.id.into())
        .collect()
}
