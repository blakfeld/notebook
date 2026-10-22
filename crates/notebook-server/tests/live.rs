//! The live channel over a real socket: authentication, delivery, socket
//! pushes, lifetime limits, and shutdown.
//!
//! Timings use short real durations from `LiveSettings` rather than paused
//! tokio time: sqlx runs SQLite on its own thread, so a paused clock would
//! auto-advance during every query.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use notebook_core::auth::api::{SessionKind, Token};
use notebook_core::note::NoteId;
use notebook_core::protocol::{
    Bytes, CLOSE_AUTH, CLOSE_RESYNC, CLOSE_SHUTDOWN, CLOSE_TOO_MANY, ClientFrame, Entry, ErrorBody,
    ErrorCode, OpId, PushOutcome, PushUpdate, ServerFrame,
};
use notebook_server::app::{self, AppState};
use notebook_server::auth::sessions::{self, ACCESS_TTL_SECS};
use notebook_server::auth::store::{NewAccount, create_account};
use notebook_server::auth::{keyfile, unix_now};
use notebook_server::config::Config;
use notebook_server::db;
use notebook_server::sync::{LiveSettings, SyncState, store};
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

const MAX_UPDATE: usize = 1000;
const WAIT: Duration = Duration::from_secs(5);

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Server {
    _dir: TempDir,
    state: AppState,
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

async fn server(settings: LiveSettings) -> Server {
    let config = Config::from_sources(None, |k| match k {
        "NOTEBOOK_PUBLIC_URL" => Some("https://notes.example.com".to_owned()),
        "NOTEBOOK_SYNC_MAX_UPDATE_BYTES" => Some(MAX_UPDATE.to_string()),
        _ => None,
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("db")).await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = AppState {
        sync: Arc::new(SyncState::new(settings)),
        ..AppState::new(pool, config, keyfile::generate())
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let sync = state.sync.clone();
    let task = tokio::spawn(app::run(listener, app::router(state.clone()), async move {
        let _ = rx.await;
        sync.shutdown();
    }));
    Server {
        _dir: dir,
        state,
        addr,
        shutdown: Some(tx),
        task,
    }
}

fn vault(n: u8) -> [u8; 16] {
    [n + 100; 16]
}

impl Server {
    /// Creates user `n` (vault [`vault`]`(n)`).
    async fn account(&self, n: u8) {
        let mut tx = self.state.pool.begin().await.unwrap();
        create_account(
            &mut tx,
            &NewAccount {
                user_id: [n; 16],
                username: &format!("user{n}"),
                opaque_record: b"r",
                recovery_verifier_hash: [0; 32],
                is_admin: false,
                vault_id: vault(n),
                key_bundle: b"b",
                now: unix_now(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    /// A new session for user `n` whose access token expires in `ttl` seconds.
    async fn token_expiring_in(&self, n: u8, ttl: i64) -> (Token, [u8; 16]) {
        let mut conn = self.state.pool.acquire().await.unwrap();
        let issued = sessions::issue(
            &mut conn,
            &[n; 16],
            &vault(n),
            "device",
            SessionKind::Native,
            unix_now() - ACCESS_TTL_SECS + ttl,
        )
        .await
        .unwrap();
        (issued.access_token, issued.session_id)
    }

    async fn token(&self, n: u8) -> Token {
        self.token_expiring_in(n, ACCESS_TTL_SECS).await.0
    }

    async fn connect(&self) -> Ws {
        connect_async(format!("ws://{}/sync/v1/live", self.addr))
            .await
            .unwrap()
            .0
    }

    /// Connects and authenticates; returns the socket and the ready head.
    async fn live(&self, token: &Token) -> (Ws, u64) {
        let mut ws = self.connect().await;
        send(
            &mut ws,
            &ClientFrame::Auth {
                token: token.clone(),
            },
        )
        .await;
        match next(&mut ws).await {
            Event::Frame(ServerFrame::Ready { head }) => (ws, head),
            other => panic!("expected ready, got {other:?}"),
        }
    }

    async fn push(&self, n: u8, updates: &[PushUpdate]) -> Vec<u64> {
        self.state
            .sync
            .push(&self.state.pool, &vault(n), updates, MAX_UPDATE)
            .await
            .unwrap()
            .seqs
    }

    async fn op_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM ops")
            .fetch_one(&self.state.pool)
            .await
            .unwrap()
    }
}

fn note(n: u128) -> NoteId {
    NoteId::from_parts(1, n)
}

fn upd(o: u128, n: u128, data: &[u8]) -> PushUpdate {
    PushUpdate {
        id: OpId::from_parts(2, o),
        note: note(n),
        data: Bytes(data.to_vec()),
        note_key: Some(Bytes(format!("key-{n}").into_bytes())),
    }
}

async fn send(ws: &mut Ws, frame: &ClientFrame) {
    ws.send(Message::text(serde_json::to_string(frame).unwrap()))
        .await
        .unwrap();
}

#[derive(Debug)]
enum Event {
    Frame(ServerFrame),
    Closed(Option<u16>),
    Timeout,
}

/// The next server frame or close, skipping pings.
async fn next_within(ws: &mut Ws, wait: Duration) -> Event {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return Event::Timeout,
            Ok(Some(Ok(Message::Text(text)))) => {
                return Event::Frame(serde_json::from_str(&text).unwrap());
            }
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Event::Closed(frame.map(|f| f.code.into()));
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_)) | None) => return Event::Closed(None),
        }
    }
}

async fn next(ws: &mut Ws) -> Event {
    next_within(ws, WAIT).await
}

#[track_caller]
fn expect_close(event: Event, code: u16) {
    match event {
        Event::Closed(Some(c)) => assert_eq!(c, code),
        other => panic!("expected close {code}, got {other:?}"),
    }
}

async fn expect_entry(ws: &mut Ws) -> Entry {
    match next(ws).await {
        Event::Frame(ServerFrame::Entry(entry)) => entry,
        other => panic!("expected an entry, got {other:?}"),
    }
}

// --- 5.2: subscription and delivery ---

#[tokio::test]
async fn ready_carries_the_head() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    s.push(1, &[upd(1, 1, b"a"), upd(2, 1, b"b")]).await;
    let (_ws, head) = s.live(&s.token(1).await).await;
    assert_eq!(head, 2);
}

#[tokio::test]
async fn pushes_and_purges_are_delivered() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    let (mut ws, _) = s.live(&s.token(1).await).await;

    s.push(1, &[upd(1, 1, b"first")]).await;
    match expect_entry(&mut ws).await {
        Entry::Update {
            seq,
            data,
            note_key,
            ..
        } => {
            assert_eq!((seq, data.0.as_slice()), (1, b"first".as_slice()));
            assert!(note_key.is_some(), "registering update carries its key");
        }
        other => panic!("{other:?}"),
    }
    s.push(
        1,
        &[PushUpdate {
            note_key: None,
            ..upd(2, 1, b"second")
        }],
    )
    .await;
    assert!(matches!(
        expect_entry(&mut ws).await,
        Entry::Update {
            seq: 2,
            note_key: None,
            ..
        }
    ));
    s.state
        .sync
        .purge(&s.state.pool, &vault(1), note(1))
        .await
        .unwrap();
    assert_eq!(
        expect_entry(&mut ws).await,
        Entry::Purge {
            seq: 3,
            note: note(1)
        }
    );
}

#[tokio::test]
async fn other_vaults_are_not_delivered() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    s.account(2).await;
    let (mut ws, _) = s.live(&s.token(1).await).await;
    s.push(2, &[upd(1, 1, b"theirs")]).await;
    s.push(1, &[upd(2, 1, b"mine")]).await;
    match expect_entry(&mut ws).await {
        Entry::Update { data, .. } => assert_eq!(data.0, b"mine"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn ready_then_pull_has_no_gap() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    s.push(1, &[upd(1, 1, b"a")]).await;
    let cursor = 1;
    s.push(1, &[upd(2, 1, b"b"), upd(3, 2, b"c")]).await;

    let (mut ws, head) = s.live(&s.token(1).await).await;
    let later = s.push(1, &[upd(4, 1, b"d"), upd(5, 2, b"e")]).await;

    let mut seen: Vec<u64> = store::changes(&s.state.pool, &vault(1), cursor)
        .await
        .unwrap()
        .entries
        .iter()
        .map(Entry::seq)
        .collect();
    for _ in &later {
        let seq = expect_entry(&mut ws).await.seq();
        assert!(seq > head, "live entries are after ready");
        seen.push(seq);
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen, [2, 3, 4, 5]);
}

#[tokio::test]
async fn silent_connection_is_closed() {
    let s = server(LiveSettings {
        auth_timeout: Duration::from_millis(300),
        ..Default::default()
    })
    .await;
    let mut ws = s.connect().await;
    expect_close(next(&mut ws).await, CLOSE_AUTH);
}

#[tokio::test]
async fn bad_token_is_closed() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    let (expired, _) = s.token_expiring_in(1, -1).await;
    for token in [Token("garbage".into()), expired] {
        let mut ws = s.connect().await;
        send(&mut ws, &ClientFrame::Auth { token }).await;
        expect_close(next(&mut ws).await, CLOSE_AUTH);
    }
}

#[tokio::test]
async fn foreign_origin_is_refused() {
    let s = server(LiveSettings::default()).await;
    let url = format!("ws://{}/sync/v1/live", s.addr);
    let mut request = url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", HeaderValue::from_static("https://evil.example"));
    match connect_async(request).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        other => panic!("expected 403, got {other:?}"),
    }
    // The server's own origin is accepted.
    let mut request = url.as_str().into_client_request().unwrap();
    request.headers_mut().insert(
        "origin",
        HeaderValue::from_static("https://notes.example.com"),
    );
    assert!(connect_async(request).await.is_ok());
}

#[tokio::test]
async fn connections_are_capped_per_vault() {
    let s = server(LiveSettings {
        max_connections_per_vault: 2,
        ..Default::default()
    })
    .await;
    s.account(1).await;
    let token = s.token(1).await;
    let (mut a, _) = s.live(&token).await;
    let (_b, _) = s.live(&token).await;
    let mut c = s.connect().await;
    send(&mut c, &ClientFrame::Auth { token }).await;
    expect_close(next(&mut c).await, CLOSE_TOO_MANY);

    s.push(1, &[upd(1, 1, b"a")]).await;
    assert_eq!(expect_entry(&mut a).await.seq(), 1, "existing unaffected");
}

#[tokio::test]
async fn lagging_connection_is_told_to_resync() {
    let s = server(LiveSettings {
        channel_capacity: 2,
        ..Default::default()
    })
    .await;
    s.account(1).await;
    let (mut ws, _) = s.live(&s.token(1).await).await;
    let batch: Vec<PushUpdate> = (0..6).map(|i| upd(i, 1, b"x")).collect();
    s.push(1, &batch).await;
    loop {
        match next(&mut ws).await {
            Event::Frame(ServerFrame::Entry(_)) => {}
            other => return expect_close(other, CLOSE_RESYNC),
        }
    }
}

#[tokio::test]
async fn unanswered_pings_close_the_connection() {
    let s = server(LiveSettings {
        ping_interval: Duration::from_millis(200),
        ..Default::default()
    })
    .await;
    s.account(1).await;
    let (mut ws, _) = s.live(&s.token(1).await).await;
    // Not reading means not answering pings.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    // The close frame (1001) may race the client's late pong replies into a
    // reset, so accept either; what matters is that the server hung up.
    match next(&mut ws).await {
        Event::Closed(None | Some(1001)) => {}
        other => panic!("expected the server to close, got {other:?}"),
    }
}

#[tokio::test]
async fn unparseable_frame_after_auth_is_closed() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    let (mut ws, _) = s.live(&s.token(1).await).await;
    ws.send(Message::text("{not json")).await.unwrap();
    expect_close(next(&mut ws).await, 1008);
}

// --- 5.3: pushes over the socket ---

#[tokio::test]
async fn socket_push_is_acknowledged_and_relayed() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    let token = s.token(1).await;
    let (mut pusher, _) = s.live(&token).await;
    let (mut other, _) = s.live(&token).await;
    send(
        &mut pusher,
        &ClientFrame::Push {
            request_id: "r1".into(),
            updates: vec![upd(1, 1, b"a"), upd(2, 1, b"b")],
        },
    )
    .await;

    // The pusher sees its own entries too, so look for the result.
    let mut result = None;
    for _ in 0..3 {
        if let Event::Frame(ServerFrame::PushResult {
            request_id,
            outcome,
        }) = next(&mut pusher).await
        {
            result = Some((request_id, outcome));
        }
    }
    let (request_id, outcome) = result.expect("a push result");
    assert_eq!(request_id, "r1");
    match outcome {
        PushOutcome::Accepted(pushed) => assert_eq!(pushed.seqs, [1, 2]),
        other => panic!("{other:?}"),
    }
    assert_eq!(expect_entry(&mut other).await.seq(), 1);
    assert_eq!(expect_entry(&mut other).await.seq(), 2);
}

#[tokio::test]
async fn rejected_socket_push_keeps_the_socket_open() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    let token = s.token(1).await;
    let (mut ws, _) = s.live(&token).await;
    let (mut other, _) = s.live(&token).await;
    send(
        &mut ws,
        &ClientFrame::Push {
            request_id: "big".into(),
            updates: vec![upd(1, 1, b"ok"), upd(2, 1, &[0; MAX_UPDATE + 1])],
        },
    )
    .await;
    match next(&mut ws).await {
        Event::Frame(ServerFrame::PushResult {
            request_id,
            outcome,
        }) => {
            assert_eq!(request_id, "big");
            assert_eq!(
                outcome,
                PushOutcome::Rejected(ErrorBody {
                    error: ErrorCode::UpdateTooLarge
                })
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(s.op_count().await, 0);
    assert!(matches!(
        next_within(&mut other, Duration::from_millis(300)).await,
        Event::Timeout
    ));

    // Still open: a valid push goes through.
    send(
        &mut ws,
        &ClientFrame::Push {
            request_id: "ok".into(),
            updates: vec![upd(3, 1, b"ok")],
        },
    )
    .await;
    assert_eq!(expect_entry(&mut other).await.seq(), 1);
}

#[tokio::test]
async fn push_before_auth_is_closed() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    let mut ws = s.connect().await;
    send(
        &mut ws,
        &ClientFrame::Push {
            request_id: "r".into(),
            updates: vec![upd(1, 1, b"a")],
        },
    )
    .await;
    expect_close(next(&mut ws).await, CLOSE_AUTH);
    assert_eq!(s.op_count().await, 0);
}

// --- 5.4: lifetime ---

#[tokio::test]
async fn expired_token_closes_the_connection() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    let (token, _) = s.token_expiring_in(1, 2).await;
    let (mut ws, _) = s.live(&token).await;
    expect_close(next(&mut ws).await, CLOSE_AUTH);
}

#[tokio::test]
async fn refreshed_token_extends_the_connection() {
    let s = server(LiveSettings::default()).await;
    s.account(1).await;
    s.account(2).await;
    let (token, _) = s.token_expiring_in(1, 2).await;
    let (mut ws, _) = s.live(&token).await;
    send(
        &mut ws,
        &ClientFrame::Auth {
            token: s.token(1).await,
        },
    )
    .await;
    assert!(matches!(
        next_within(&mut ws, Duration::from_secs(4)).await,
        Event::Timeout
    ));
    s.push(1, &[upd(1, 1, b"a")]).await;
    assert_eq!(expect_entry(&mut ws).await.seq(), 1, "still open");

    // Another vault's token is refused.
    send(
        &mut ws,
        &ClientFrame::Auth {
            token: s.token(2).await,
        },
    )
    .await;
    expect_close(next(&mut ws).await, CLOSE_AUTH);
}

#[tokio::test]
async fn revoked_session_is_closed_within_a_ping_interval() {
    let s = server(LiveSettings {
        ping_interval: Duration::from_millis(200),
        ..Default::default()
    })
    .await;
    s.account(1).await;
    let (token, session) = s.token_expiring_in(1, ACCESS_TTL_SECS).await;
    let (mut ws, _) = s.live(&token).await;
    // Pings are answered while reading, so the connection stays up.
    assert!(matches!(
        next_within(&mut ws, Duration::from_millis(700)).await,
        Event::Timeout
    ));
    assert!(
        sessions::revoke(&s.state.pool, &[1; 16], &session)
            .await
            .unwrap()
    );
    expect_close(
        next_within(&mut ws, Duration::from_millis(600)).await,
        CLOSE_AUTH,
    );
}

#[tokio::test]
async fn shutdown_closes_sockets_and_finishes() {
    let mut s = server(LiveSettings::default()).await;
    s.account(1).await;
    let (mut ws, _) = s.live(&s.token(1).await).await;
    s.shutdown.take().unwrap().send(()).unwrap();
    expect_close(next(&mut ws).await, CLOSE_SHUTDOWN);
    let result = tokio::time::timeout(WAIT, s.task)
        .await
        .expect("server did not stop with a socket open")
        .unwrap();
    assert!(result.is_ok(), "{result:?}");
}
