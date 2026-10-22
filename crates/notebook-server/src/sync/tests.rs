//! Router tests for `/sync/v1` through the full middleware stack, and the
//! hub's broadcast ordering.

use std::net::SocketAddr;

use axum::Router;
use axum::body::{Body, Bytes as BodyBytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http_body_util::BodyExt;
use notebook_core::auth::api::{SessionKind, Token};
use notebook_core::note::NoteId;
use notebook_core::protocol::{
    Bytes, ChangesResponse, Entry, ErrorBody, ErrorCode, OpId, PushRequest, PushResponse,
    PushUpdate,
};
use serde_json::json;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::app::{self, AppState};
use crate::auth::sessions::{self, ACCESS_TTL_SECS};
use crate::auth::store::{NewAccount, create_account};
use crate::auth::{keyfile, unix_now};
use crate::config::{Config, LogFormat};
use crate::db;
use crate::logging::{self, test_writer::Captured};

const MAX_UPDATE: usize = 1000;
const MAX_BODY: usize = 4096;

struct Harness {
    _dir: TempDir,
    state: AppState,
    router: Router,
}

async fn harness() -> Harness {
    let config = Config::from_sources(None, |k| match k {
        "NOTEBOOK_PUBLIC_URL" => Some("http://localhost".to_owned()),
        "NOTEBOOK_SYNC_MAX_UPDATE_BYTES" => Some(MAX_UPDATE.to_string()),
        "NOTEBOOK_MAX_BODY_BYTES" => Some(MAX_BODY.to_string()),
        _ => None,
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("db")).await.unwrap();
    db::migrate(&pool).await.unwrap();
    let state = AppState::new(pool, config, keyfile::generate());
    let router =
        app::router(state.clone()).layer(MockConnectInfo(SocketAddr::from(([192, 0, 2, 1], 1))));
    Harness {
        _dir: dir,
        state,
        router,
    }
}

impl Harness {
    /// Creates user `n` with vault `[n + 100; 16]`; returns an access token
    /// issued at `issued_at`.
    async fn account_at(&self, n: u8, issued_at: i64) -> Token {
        let mut tx = self.state.pool.begin().await.unwrap();
        let vault = [n + 100; 16];
        create_account(
            &mut tx,
            &NewAccount {
                user_id: [n; 16],
                username: &format!("user{n}"),
                opaque_record: b"r",
                recovery_verifier_hash: [0; 32],
                is_admin: false,
                vault_id: vault,
                key_bundle: b"b",
                now: issued_at,
            },
        )
        .await
        .unwrap();
        let issued = sessions::issue(
            &mut tx,
            &[n; 16],
            &vault,
            "d",
            SessionKind::Native,
            issued_at,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        issued.access_token
    }

    async fn account(&self, n: u8) -> Token {
        self.account_at(n, unix_now()).await
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Body,
        token: Option<&Token>,
    ) -> (StatusCode, BodyBytes) {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token.0));
        }
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        let response = self
            .router
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            response.into_body().collect().await.unwrap().to_bytes(),
        )
    }

    async fn push_raw(&self, token: &Token, body: String) -> (StatusCode, BodyBytes) {
        self.call(Method::POST, "/sync/v1/push", Body::from(body), Some(token))
            .await
    }

    async fn push(&self, token: &Token, updates: Vec<PushUpdate>) -> (StatusCode, BodyBytes) {
        self.push_raw(
            token,
            serde_json::to_string(&PushRequest { updates }).unwrap(),
        )
        .await
    }

    async fn changes(&self, token: &Token, query: &str) -> (StatusCode, BodyBytes) {
        self.call(
            Method::GET,
            &format!("/sync/v1/changes{query}"),
            Body::empty(),
            Some(token),
        )
        .await
    }

    async fn snapshot(
        &self,
        token: &Token,
        note: &str,
        query: &str,
        data: Vec<u8>,
    ) -> (StatusCode, BodyBytes) {
        self.call(
            Method::PUT,
            &format!("/sync/v1/notes/{note}/snapshot{query}"),
            Body::from(data),
            Some(token),
        )
        .await
    }

    async fn purge(&self, token: &Token, note: &str) -> (StatusCode, BodyBytes) {
        self.call(
            Method::DELETE,
            &format!("/sync/v1/notes/{note}"),
            Body::empty(),
            Some(token),
        )
        .await
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

fn keyless(o: u128, n: u128, data: &[u8]) -> PushUpdate {
    PushUpdate {
        note_key: None,
        ..upd(o, n, data)
    }
}

#[track_caller]
fn expect_error(reply: (StatusCode, BodyBytes), status: StatusCode, code: ErrorCode) {
    assert_eq!(reply.0, status, "{}", String::from_utf8_lossy(&reply.1));
    let body: ErrorBody = serde_json::from_slice(&reply.1).unwrap();
    assert_eq!(body.error, code);
}

#[tokio::test]
async fn every_endpoint_requires_a_valid_token() {
    let h = harness().await;
    let expired = h.account_at(1, unix_now() - ACCESS_TTL_SECS - 10).await;
    let push = || {
        Body::from(
            serde_json::to_string(&PushRequest {
                updates: vec![upd(1, 1, b"a")],
            })
            .unwrap(),
        )
    };
    let n = note(1).to_string();
    let requests = [
        (Method::POST, "/sync/v1/push".to_owned()),
        (Method::GET, "/sync/v1/changes?after=0".to_owned()),
        (Method::PUT, format!("/sync/v1/notes/{n}/snapshot?covers=1")),
        (Method::DELETE, format!("/sync/v1/notes/{n}")),
    ];
    for (method, path) in requests {
        for token in [None, Some(&expired), Some(&Token("garbage".into()))] {
            let reply = h.call(method.clone(), &path, push(), token).await;
            expect_error(reply, StatusCode::UNAUTHORIZED, ErrorCode::InvalidToken);
        }
    }
    let ops: i64 = sqlx::query_scalar("SELECT count(*) FROM ops")
        .fetch_one(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(ops, 0);
}

#[tokio::test]
async fn success_paths() {
    let h = harness().await;
    let t = h.account(1).await;
    let (status, body) = h.push(&t, vec![upd(1, 1, b"a"), keyless(2, 1, b"b")]).await;
    assert_eq!(status, StatusCode::OK);
    let pushed: PushResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        pushed,
        PushResponse {
            seqs: vec![1, 2],
            head: 2
        }
    );

    let n = note(1).to_string();
    let (status, _) = h.snapshot(&t, &n, "?covers=1", b"snap".to_vec()).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = h.changes(&t, "?after=0").await;
    assert_eq!(status, StatusCode::OK);
    let page: ChangesResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.notes[0].snapshot.as_ref().unwrap().data.0, b"snap");
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.cursor, 2);

    let (status, _) = h.purge(&t, &n).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = h.purge(&t, &n).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "idempotent");
}

#[tokio::test]
async fn error_table() {
    let h = harness().await;
    let t = h.account(1).await;
    let n1 = note(1).to_string();
    use ErrorCode::*;
    use StatusCode as S;

    // 400
    expect_error(h.push(&t, vec![]).await, S::BAD_REQUEST, BadRequest);
    expect_error(h.push_raw(&t, "{".into()).await, S::BAD_REQUEST, BadRequest);
    expect_error(
        h.push_raw(
            &t,
            json!({"updates": [{"id": "x", "note": "y", "data": ""}]}).to_string(),
        )
        .await,
        S::BAD_REQUEST,
        BadRequest,
    );
    expect_error(
        h.push_raw(
            &t,
            json!({"updates": [{"id": OpId::from_parts(1, 1), "note": note(1), "data": "%%"}]})
                .to_string(),
        )
        .await,
        S::BAD_REQUEST,
        BadRequest,
    );
    for query in ["", "?after=-1", "?after=x"] {
        expect_error(h.changes(&t, query).await, S::BAD_REQUEST, BadRequest);
    }
    expect_error(
        h.snapshot(&t, "not-a-note", "?covers=1", vec![]).await,
        S::BAD_REQUEST,
        BadRequest,
    );
    expect_error(h.purge(&t, "not-a-note").await, S::BAD_REQUEST, BadRequest);

    // 422, then register note 1.
    expect_error(
        h.push(&t, vec![keyless(1, 1, b"a")]).await,
        S::UNPROCESSABLE_ENTITY,
        MissingNoteKey,
    );
    assert_eq!(h.push(&t, vec![upd(1, 1, b"a")]).await.0, S::OK);

    // 400 for a snapshot past the head or without `covers`.
    for query in ["?covers=5", "", "?covers=-1"] {
        expect_error(
            h.snapshot(&t, &n1, query, b"s".to_vec()).await,
            S::BAD_REQUEST,
            BadRequest,
        );
    }

    // 404
    let n9 = note(9).to_string();
    expect_error(
        h.snapshot(&t, &n9, "?covers=1", b"s".to_vec()).await,
        S::NOT_FOUND,
        UnknownNote,
    );
    expect_error(h.purge(&t, &n9).await, S::NOT_FOUND, UnknownNote);

    // 409
    expect_error(
        h.push(&t, vec![keyless(1, 1, b"different")]).await,
        S::CONFLICT,
        OpIdConflict,
    );
    let mut wrong_key = keyless(2, 1, b"b");
    wrong_key.note_key = Some(Bytes(b"other".to_vec()));
    expect_error(
        h.push(&t, vec![wrong_key]).await,
        S::CONFLICT,
        NoteKeyConflict,
    );
    assert_eq!(
        h.snapshot(&t, &n1, "?covers=1", b"s".to_vec()).await.0,
        S::NO_CONTENT
    );
    expect_error(
        h.snapshot(&t, &n1, "?covers=1", b"s".to_vec()).await,
        S::CONFLICT,
        SnapshotStale,
    );
    expect_error(h.changes(&t, "?after=99").await, S::CONFLICT, CursorAhead);

    // 413
    expect_error(
        h.push(&t, vec![keyless(3, 1, &[0; MAX_UPDATE + 1])]).await,
        S::PAYLOAD_TOO_LARGE,
        UpdateTooLarge,
    );
    assert_eq!(
        h.push(&t, vec![keyless(3, 1, &[0; MAX_UPDATE])]).await.0,
        S::OK,
        "exactly at the limit"
    );
    let (status, _) = h
        .snapshot(&t, &n1, "?covers=2", vec![0; MAX_BODY + 1])
        .await;
    assert_eq!(status, S::PAYLOAD_TOO_LARGE);
    let (status, _) = h
        .push_raw(
            &t,
            format!("{{\"updates\": [], \"pad\": \"{}\"}}", "x".repeat(MAX_BODY)),
        )
        .await;
    assert_eq!(status, S::PAYLOAD_TOO_LARGE);

    // 410
    assert_eq!(h.purge(&t, &n1).await.0, S::NO_CONTENT);
    expect_error(
        h.push(&t, vec![keyless(4, 1, b"x")]).await,
        S::GONE,
        NotePurged,
    );
    expect_error(
        h.snapshot(&t, &n1, "?covers=1", b"s".to_vec()).await,
        S::GONE,
        NotePurged,
    );
}

#[tokio::test]
async fn vaults_are_isolated() {
    let h = harness().await;
    let alice = h.account(1).await;
    let bob = h.account(2).await;
    assert_eq!(
        h.push(&alice, vec![upd(1, 1, b"alice's")]).await.0,
        StatusCode::OK
    );
    // Same note ID and op ID in another vault: no conflict, no leak.
    assert_eq!(
        h.push(&bob, vec![upd(1, 1, b"bob's")]).await.0,
        StatusCode::OK
    );
    let (_, body) = h.changes(&alice, "?after=0").await;
    let page: ChangesResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert!(matches!(&page.entries[0], Entry::Update { data, .. } if data.0 == b"alice's"));
    assert!(!String::from_utf8_lossy(&body).contains(&STANDARD.encode(b"bob's")));
    // Bob can't purge or snapshot Alice's note by ID either; it's his own.
    assert_eq!(
        h.purge(&bob, &note(1).to_string()).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, body) = h.changes(&alice, "?after=0").await;
    let page: ChangesResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.entries.len(), 1, "Alice's note is untouched");
}

#[tokio::test]
async fn arbitrary_bytes_round_trip() {
    let h = harness().await;
    let t = h.account(1).await;
    let data: Vec<u8> = (0..=255).collect();
    let key: Vec<u8> = (0..=255).rev().collect();
    let mut update = upd(1, 1, &data);
    update.note_key = Some(Bytes(key.clone()));
    assert_eq!(h.push(&t, vec![update]).await.0, StatusCode::OK);
    let (_, body) = h.changes(&t, "?after=0").await;
    let page: ChangesResponse = serde_json::from_slice(&body).unwrap();
    assert_eq!(page.notes[0].key.as_ref().unwrap().0, key);
    assert!(matches!(&page.entries[0], Entry::Update { data: d, .. } if d.0 == data));
}

#[tokio::test]
async fn payloads_are_never_logged() {
    let h = harness().await;
    let t = h.account(1).await;
    let markers = [
        b"UPDATE-MARKER-4417".as_slice(),
        b"KEY-MARKER-9921",
        b"SNAPSHOT-MARKER-3308",
    ];
    // Register the request path's callsites before installing the scoped
    // subscriber, so a concurrent test can't cache them as disabled.
    assert_eq!(
        h.push(&t, vec![upd(1, 5, b"warm-up")]).await.0,
        StatusCode::OK
    );
    h.changes(&t, "?after=0").await;
    for format in [LogFormat::Text, LogFormat::Json] {
        let out = Captured::default();
        let _guard = tracing::subscriber::set_default(logging::subscriber(
            "trace",
            format,
            false,
            out.clone(),
        ));
        tracing::callsite::rebuild_interest_cache();
        let n = note(u128::from(format == LogFormat::Json));
        let mut update = upd(u128::from(format == LogFormat::Json) + 10, 0, markers[0]);
        update.note = n;
        update.note_key = Some(Bytes(markers[1].to_vec()));
        assert_eq!(h.push(&t, vec![update]).await.0, StatusCode::OK);
        let (_, body) = h.changes(&t, "?after=0").await;
        let head = serde_json::from_slice::<ChangesResponse>(&body)
            .unwrap()
            .cursor;
        let (status, _) = h
            .snapshot(
                &t,
                &n.to_string(),
                &format!("?covers={head}"),
                markers[2].to_vec(),
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        h.changes(&t, "?after=0").await;

        let log = out.contents();
        assert!(log.contains("/sync/v1/push"), "{log}");
        for marker in markers {
            let text = String::from_utf8_lossy(marker);
            assert!(!log.contains(&*text), "{text} leaked: {log}");
            assert!(
                !log.contains(&STANDARD.encode(marker)),
                "{text} leaked as base64"
            );
        }
        assert!(!log.contains(&t.0), "token leaked");
    }
}

#[tokio::test]
async fn concurrent_pushes_broadcast_in_order() {
    let h = harness().await;
    h.account(1).await;
    let vault = [101u8; 16];
    let mut rx = h.state.sync.hub(&vault).subscribe();
    let tasks: Vec<_> = (0..20u128)
        .map(|i| {
            let state = h.state.clone();
            tokio::spawn(async move {
                state
                    .sync
                    .push(&state.pool, &vault, &[upd(i, i % 3, b"x")], MAX_UPDATE)
                    .await
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    // A failed push broadcasts nothing.
    let err = h
        .state
        .sync
        .push(&h.state.pool, &vault, &[keyless(99, 7, b"x")], MAX_UPDATE)
        .await
        .unwrap_err();
    assert_eq!(err.0, ErrorCode::MissingNoteKey);

    let mut seqs = Vec::new();
    while let Ok(entry) = rx.try_recv() {
        seqs.push(entry.seq());
    }
    assert_eq!(seqs, (1..=20).collect::<Vec<_>>());
}

#[test]
fn hub_caps_connections() {
    let sync = super::SyncState::new(super::LiveSettings {
        max_connections_per_vault: 2,
        ..Default::default()
    });
    let a = sync.join(b"v").unwrap();
    let _b = sync.join(b"v").unwrap();
    assert!(sync.join(b"v").is_none());
    assert!(sync.join(b"w").is_some(), "per vault");
    drop(a);
    assert!(sync.join(b"v").is_some(), "slot freed on drop");
}
