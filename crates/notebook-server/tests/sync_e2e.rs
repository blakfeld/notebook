//! End to end: clients built on `notebook_core::protocol` and real encrypted
//! `NoteDocument`s create, edit, snapshot, and purge notes over HTTP and the
//! live channel, and a fresh client bootstraps to the same state.
//!
//! HTTP requests go through the full router with `oneshot`; the live channel
//! is a real socket on the same server state.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, header};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use notebook_core::auth::api::{SessionKind, Token};
use notebook_core::crypto::{NoteKey, VaultId, VaultKey};
use notebook_core::document::NoteDocument;
use notebook_core::note::{NoteId, Timestamp};
use notebook_core::protocol::{
    Bytes, ChangesResponse, ClientFrame, Entry, OpId, PushOutcome, PushRequest, PushResponse,
    PushUpdate, ServerFrame,
};
use notebook_server::app::{self, AppState};
use notebook_server::auth::sessions;
use notebook_server::auth::store::{NewAccount, create_account};
use notebook_server::auth::{keyfile, unix_now};
use notebook_server::config::Config;
use notebook_server::db;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const VAULT: [u8; 16] = [7; 16];

struct Env {
    state: AppState,
    router: Router,
    addr: SocketAddr,
    vault_key: VaultKey,
}

impl Env {
    async fn token(&self) -> Token {
        let mut conn = self.state.pool.acquire().await.unwrap();
        sessions::issue(
            &mut conn,
            &[1; 16],
            &VAULT,
            "d",
            SessionKind::Native,
            unix_now(),
        )
        .await
        .unwrap()
        .access_token
    }

    async fn http(&self, token: &Token, method: Method, path: &str, body: Vec<u8>) -> Vec<u8> {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {}", token.0))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            status.is_success(),
            "{path}: {status} {}",
            String::from_utf8_lossy(&body)
        );
        body.to_vec()
    }
}

/// A client: its documents, pull cursor, and next op ID.
struct Client {
    name: &'static str,
    token: Token,
    cursor: u64,
    docs: BTreeMap<NoteId, NoteDocument>,
    next_op: u128,
    live: Option<Ws>,
}

fn vault_id() -> VaultId {
    VaultId::from_bytes(VAULT)
}

fn now() -> Timestamp {
    Timestamp(unix_now() * 1000)
}

impl Client {
    async fn new(env: &Env, name: &'static str, n: u128) -> Self {
        Client {
            name,
            token: env.token().await,
            cursor: 0,
            docs: BTreeMap::new(),
            next_op: n << 64,
            live: None,
        }
    }

    fn op(&mut self) -> OpId {
        self.next_op += 1;
        OpId::from_parts(1, self.next_op)
    }

    fn open(&mut self, env: &Env, note: NoteId, wrapped: &[u8]) {
        let key = env
            .vault_key
            .unwrap_note_key(vault_id(), note, wrapped)
            .unwrap();
        self.docs
            .entry(note)
            .or_insert_with(|| NoteDocument::open(vault_id(), note, key).unwrap());
    }

    fn apply(&mut self, env: &Env, entry: &Entry) {
        match entry {
            Entry::Update {
                note,
                data,
                note_key,
                ..
            } => {
                if let Some(key) = note_key {
                    self.open(env, *note, &key.0);
                }
                let doc = self
                    .docs
                    .get_mut(note)
                    .unwrap_or_else(|| panic!("{}: update before key", self.name));
                doc.apply_update(&data.0).unwrap();
            }
            Entry::Purge { note, .. } => {
                self.docs.remove(note);
            }
        }
        self.cursor = self.cursor.max(entry.seq());
    }

    /// Pulls every page from the cursor.
    async fn pull(&mut self, env: &Env) {
        loop {
            let body = env
                .http(
                    &self.token,
                    Method::GET,
                    &format!("/sync/v1/changes?after={}", self.cursor),
                    Vec::new(),
                )
                .await;
            let page: ChangesResponse = serde_json::from_slice(&body).unwrap();
            for n in &page.notes {
                if let Some(key) = &n.key {
                    self.open(env, n.note, &key.0);
                }
                if let Some(snapshot) = &n.snapshot {
                    self.docs
                        .get_mut(&n.note)
                        .unwrap()
                        .load_snapshot(&snapshot.data.0)
                        .unwrap();
                }
            }
            for entry in &page.entries {
                self.apply(env, entry);
            }
            self.cursor = page.cursor;
            if !page.more {
                return;
            }
        }
    }

    /// Opens the live channel, then pulls, so nothing is missed.
    async fn go_live(&mut self, env: &Env) {
        let mut ws = connect_async(format!("ws://{}/sync/v1/live", env.addr))
            .await
            .unwrap()
            .0;
        send(
            &mut ws,
            &ClientFrame::Auth {
                token: self.token.clone(),
            },
        )
        .await;
        assert!(matches!(recv(&mut ws).await, ServerFrame::Ready { .. }));
        self.live = Some(ws);
        self.pull(env).await;
    }

    /// Applies live frames until an entry at or above `seq` has arrived.
    async fn wait_for(&mut self, env: &Env, seq: u64) {
        while self.cursor < seq {
            let frame = recv(self.live.as_mut().unwrap()).await;
            if let ServerFrame::Entry(entry) = frame
                && entry.seq() > self.cursor
            {
                self.apply(env, &entry);
            }
        }
    }

    fn create(&mut self, env: &Env, note: NoteId, body: &str) -> PushUpdate {
        let key = NoteKey::generate().unwrap();
        let wrapped = env.vault_key.wrap_note_key(vault_id(), note, &key).unwrap();
        let (doc, update) = NoteDocument::create(vault_id(), note, key, body, now()).unwrap();
        self.docs.insert(note, doc);
        PushUpdate {
            id: self.op(),
            note,
            data: Bytes(update.into_bytes()),
            note_key: Some(Bytes(wrapped)),
        }
    }

    fn edit(&mut self, note: NoteId, body: &str) -> PushUpdate {
        let update = self
            .docs
            .get_mut(&note)
            .unwrap()
            .set_body(body, now())
            .unwrap()
            .unwrap();
        PushUpdate {
            id: self.op(),
            note,
            data: Bytes(update.into_bytes()),
            note_key: None,
        }
    }

    async fn push_http(&mut self, env: &Env, updates: Vec<PushUpdate>) -> u64 {
        let body = serde_json::to_vec(&PushRequest { updates }).unwrap();
        let body = env
            .http(&self.token, Method::POST, "/sync/v1/push", body)
            .await;
        serde_json::from_slice::<PushResponse>(&body).unwrap().head
    }

    /// Pushes over the socket; returns the head from the push result.
    async fn push_live(&mut self, env: &Env, updates: Vec<PushUpdate>) -> u64 {
        let ours: Vec<OpId> = updates.iter().map(|u| u.id).collect();
        let ws = self.live.as_mut().unwrap();
        send(
            ws,
            &ClientFrame::Push {
                request_id: "p".into(),
                updates,
            },
        )
        .await;
        loop {
            match recv(self.live.as_mut().unwrap()).await {
                ServerFrame::PushResult {
                    outcome: PushOutcome::Accepted(pushed),
                    ..
                } => return pushed.head,
                ServerFrame::PushResult { outcome, .. } => panic!("{outcome:?}"),
                // Our own updates echo back; we already have them.
                ServerFrame::Entry(Entry::Update { id, seq, .. }) if ours.contains(&id) => {
                    self.cursor = self.cursor.max(seq);
                }
                ServerFrame::Entry(entry) if entry.seq() > self.cursor => self.apply(env, &entry),
                ServerFrame::Entry(_) | ServerFrame::Ready { .. } => {}
            }
        }
    }

    fn bodies(&self) -> BTreeMap<NoteId, String> {
        self.docs
            .iter()
            .map(|(id, doc)| (*id, doc.note().unwrap().body().to_owned()))
            .collect()
    }
}

async fn send(ws: &mut Ws, frame: &ClientFrame) {
    ws.send(Message::text(serde_json::to_string(frame).unwrap()))
        .await
        .unwrap();
}

async fn recv(ws: &mut Ws) -> ServerFrame {
    loop {
        let message = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a frame")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

#[tokio::test]
async fn two_clients_sync_and_a_third_bootstraps() {
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
    let state = AppState::new(pool, config, keyfile::generate());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(app::run(
        listener,
        app::router(state.clone()),
        std::future::pending(),
    ));
    let env = Env {
        router: app::router(state.clone())
            .layer(MockConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1)))),
        state,
        addr,
        vault_key: VaultKey::generate().unwrap(),
    };

    let n1 = NoteId::from_parts(1, 1);
    let n2 = NoteId::from_parts(1, 2);
    let mut a = Client::new(&env, "a", 1).await;
    let mut b = Client::new(&env, "b", 2).await;
    a.go_live(&env).await;
    b.go_live(&env).await;

    // A creates over HTTP; B gets it live, key and all.
    let update = a.create(&env, n1, "# Groceries\n\neggs");
    let head = a.push_http(&env, vec![update]).await;
    b.wait_for(&env, head).await;
    assert_eq!(b.bodies()[&n1], "# Groceries\n\neggs");

    // B edits over the socket; A pulls over HTTP.
    let update = b.edit(n1, "# Groceries\n\neggs\nmilk");
    b.push_live(&env, vec![update]).await;
    a.pull(&env).await;
    assert_eq!(a.bodies()[&n1], "# Groceries\n\neggs\nmilk");

    // Concurrent edits converge.
    let ua = a.edit(n1, "# Groceries\n\neggs\nmilk\nbread");
    let ub = b.edit(n1, "# Shopping\n\neggs\nmilk");
    a.push_live(&env, vec![ua]).await;
    let head = b.push_live(&env, vec![ub]).await;
    a.wait_for(&env, head).await;
    b.wait_for(&env, head).await;
    assert_eq!(a.bodies(), b.bodies());

    // A creates a second note and snapshots the first.
    let update = a.create(&env, n2, "temporary");
    let head = a.push_live(&env, vec![update]).await;
    b.wait_for(&env, head).await;
    let snapshot = a.docs[&n1].snapshot().unwrap().into_bytes();
    env.http(
        &a.token,
        Method::PUT,
        &format!("/sync/v1/notes/{n1}/snapshot?covers={head}"),
        snapshot,
    )
    .await;

    // B purges the second note; A hears of it live.
    env.http(
        &b.token,
        Method::DELETE,
        &format!("/sync/v1/notes/{n2}"),
        Vec::new(),
    )
    .await;
    a.wait_for(&env, head + 1).await;
    assert!(!a.docs.contains_key(&n2));

    // An edit after the snapshot.
    let update = a.edit(n1, "# Shopping\n\neggs\nmilk\nbread\ncoffee");
    let head = a.push_http(&env, vec![update]).await;
    b.wait_for(&env, head).await;

    // C bootstraps: the snapshot plus the later update, nothing of n2.
    let mut c = Client::new(&env, "c", 3).await;
    c.pull(&env).await;
    assert_eq!(c.cursor, head);
    assert_eq!(c.bodies(), a.bodies());
    assert_eq!(c.bodies(), b.bodies());
    assert_eq!(c.bodies().len(), 1);
    assert!(c.bodies()[&n1].ends_with("coffee"));
    let ops: i64 = sqlx::query_scalar("SELECT count(*) FROM ops")
        .fetch_one(&env.state.pool)
        .await
        .unwrap();
    assert_eq!(ops, 1, "the snapshot compacted n1; the purge removed n2");
}
