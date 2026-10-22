//! Router tests for `/auth`, driving the real client flows from
//! `notebook_core::auth` through the full middleware stack.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http_body_util::BodyExt;
use notebook_core::auth::api::{
    KeyBundleResponse, LoginFinishRequest, LoginFinishResponse, LoginStartRequest,
    LoginStartResponse, PassphraseFinishRequest, PassphraseStartResponse, RecoverStartResponse,
    RefreshRequest, RegisterStartResponse, Session, SessionInfo, SessionKind, Token, Tokens,
};
use notebook_core::auth::{
    AuthError, Device, Login, LoginKeys, NewAccount, PassphraseChange, Recovery, Registration,
};
use notebook_core::crypto::{AuthSecret, KeyBundle, RecoveryKey, VaultId, VaultKey};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tower::ServiceExt;

use super::sessions::{ACCESS_TTL_SECS, AuthSession};
use super::{AuthState, RegistrationPolicy, keyfile};
use crate::app::{self, AppState};
use crate::config::Config;
use crate::{db, user};

const T0: i64 = 1_800_000_000;
const MARKER: &str = "walrus-MARKER-8812";

fn pass(tag: &str) -> String {
    format!("{MARKER} {tag}")
}

struct Harness {
    dir: TempDir,
    state: AppState,
    router: Router,
    clock: Arc<AtomicI64>,
    next_peer: AtomicU32,
}

async fn harness(policy: RegistrationPolicy) -> Harness {
    let config = Config::from_sources(None, |k| {
        (k == "NOTEBOOK_PUBLIC_URL").then(|| "http://localhost".to_owned())
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pool = db::connect(&dir.path().join("db")).await.unwrap();
    db::migrate(&pool).await.unwrap();
    let setup = keyfile::load_or_create(&pool, &dir.path().join("key"))
        .await
        .unwrap();
    let mut auth = AuthState::new(setup, policy);
    let clock = auth.fix_clock(T0);
    let state = AppState {
        auth: Arc::new(auth),
        ..AppState::new(pool, config, keyfile::generate())
    };
    let protected = Router::new()
        .route(
            "/test/whoami",
            get(|session: AuthSession| async move { URL_SAFE_NO_PAD.encode(session.user_id) }),
        )
        .with_state(state.clone());
    let router = app::router(state.clone()).merge(app::harden(protected, &state));
    Harness {
        dir,
        state,
        router,
        clock,
        next_peer: AtomicU32::new(1),
    }
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Reply {
    fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "{e}: {:?} {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn error(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl Harness {
    fn advance(&self, secs: i64) {
        self.clock.fetch_add(secs, Ordering::SeqCst);
    }

    /// A fresh client address per request, so the per-address limits don't
    /// interfere unless a test picks the address.
    fn peer(&self) -> String {
        let n = self.next_peer.fetch_add(1, Ordering::SeqCst);
        format!("10.{}.{}.{}:4000", (n >> 16) & 255, (n >> 8) & 255, n & 255)
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        token: Option<&Token>,
        peer: &str,
    ) -> Reply {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", token.0));
        }
        let body = match body {
            Some(json) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(json)
            }
            None => Body::empty(),
        };
        let response = self
            .router
            .clone()
            .layer(MockConnectInfo(peer.parse::<SocketAddr>().unwrap()))
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        Reply {
            status,
            headers,
            body,
        }
    }

    async fn post(&self, path: &str, body: &impl Serialize, token: Option<&Token>) -> Reply {
        let peer = self.peer();
        self.post_from(&peer, path, body, token).await
    }

    async fn post_from(
        &self,
        peer: &str,
        path: &str,
        body: &impl Serialize,
        token: Option<&Token>,
    ) -> Reply {
        let json = serde_json::to_string(body).unwrap();
        self.request(Method::POST, path, Some(json), token, peer)
            .await
    }

    async fn get(&self, path: &str, token: Option<&Token>) -> Reply {
        self.request(Method::GET, path, None, token, &self.peer())
            .await
    }

    async fn delete(&self, path: &str, token: Option<&Token>) -> Reply {
        self.request(Method::DELETE, path, None, token, &self.peer())
            .await
    }

    async fn register(&self, username: &str, passphrase: &str) -> (Session, NewAccount) {
        let (reg, start) = Registration::start(username, passphrase).unwrap();
        let reply = self.post("/auth/register/start", &start, None).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
        let response: RegisterStartResponse = reply.json();
        let (finish, account) = reg
            .finish(&response, Device::default(), (T0 * 1000) as u64)
            .unwrap();
        let reply = self.post("/auth/register/finish", &finish, None).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
        (reply.json(), account)
    }

    async fn login_as(
        &self,
        username: &str,
        passphrase: &str,
        device: Device,
    ) -> Result<(LoginFinishResponse, LoginKeys), String> {
        let (login, start) = Login::start(username, passphrase).unwrap();
        let reply = self.post("/auth/login/start", &start, None).await;
        if reply.status != StatusCode::OK {
            return Err(reply.error());
        }
        let response: LoginStartResponse = reply.json();
        let (finish, keys) = login
            .finish(&response, device)
            .map_err(|e| format!("client: {e:?}"))?;
        let reply = self.post("/auth/login/finish", &finish, None).await;
        if reply.status != StatusCode::OK {
            return Err(reply.error());
        }
        Ok((reply.json(), keys))
    }

    async fn login(&self, username: &str, passphrase: &str) -> (LoginFinishResponse, LoginKeys) {
        self.login_as(username, passphrase, Device::default())
            .await
            .unwrap()
    }

    async fn refresh(&self, token: &Token) -> Reply {
        self.post(
            "/auth/refresh",
            &RefreshRequest {
                refresh_token: token.clone(),
            },
            None,
        )
        .await
    }

    async fn whoami(&self, token: &Token) -> StatusCode {
        self.get("/test/whoami", Some(token)).await.status
    }

    async fn key_bundle(&self, token: &Token) -> Vec<u8> {
        let reply = self.get("/auth/key-bundle", Some(token)).await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
        reply.json::<KeyBundleResponse>().key_bundle
    }

    async fn change_passphrase(
        &self,
        token: &Token,
        old: &str,
        new: &str,
        edit: impl FnOnce(&mut PassphraseFinishRequest),
    ) -> Reply {
        let bundle = self.key_bundle(token).await;
        let (change, start) = PassphraseChange::start(old, new).unwrap();
        let reply = self
            .post("/auth/passphrase/start", &start, Some(token))
            .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.error());
        let response: PassphraseStartResponse = reply.json();
        let (mut finish, _) = change.finish(&response, &bundle).unwrap();
        edit(&mut finish);
        self.post("/auth/passphrase/finish", &finish, Some(token))
            .await
    }

    async fn recover(
        &self,
        username: &str,
        key: RecoveryKey,
        new: &str,
    ) -> Result<Session, Box<Reply>> {
        let (recovery, start) = Recovery::start(username, key, new).unwrap();
        let reply = self.post("/auth/recover/start", &start, None).await;
        if reply.status != StatusCode::OK {
            return Err(Box::new(reply));
        }
        let response: RecoverStartResponse = reply.json();
        let (finish, _) = recovery.finish(&response, Device::default()).unwrap();
        let reply = self.post("/auth/recover/finish", &finish, None).await;
        if reply.status != StatusCode::OK {
            return Err(Box::new(reply));
        }
        Ok(reply.json())
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&self.state.pool)
            .await
            .unwrap()
    }

    async fn stored(&self) -> (Vec<u8>, Vec<u8>) {
        sqlx::query_as("SELECT u.opaque_record, b.bundle FROM users u JOIN vaults v USING (user_id) JOIN key_bundles b USING (vault_id)")
            .fetch_one(&self.state.pool)
            .await
            .unwrap()
    }
}

fn exact_error(reply: &Reply, status: StatusCode, code: &str) {
    assert_eq!(reply.status, status, "{}", reply.error());
    assert_eq!(reply.error(), format!(r#"{{"error":"{code}"}}"#));
}

/// Checks `vault_key` is `account`'s by round-tripping a note key wrapped
/// with the original.
fn assert_same_vault_key(account: &NewAccount, vault_key: &VaultKey) {
    use notebook_core::crypto::{NoteKey, PayloadKind};
    use notebook_core::note::NoteId;
    let note = NoteId::from_parts(1, 1);
    let note_key = NoteKey::generate().unwrap();
    let wrapped = account
        .vault_key
        .wrap_note_key(account.vault_id, note, &note_key)
        .unwrap();
    let sealed = note_key
        .encrypt(account.vault_id, note, PayloadKind::NotePayload, b"n")
        .unwrap();
    let key = vault_key
        .unwrap_note_key(account.vault_id, note, &wrapped)
        .unwrap();
    assert_eq!(
        &*key
            .decrypt(account.vault_id, note, PayloadKind::NotePayload, &sealed)
            .unwrap(),
        b"n"
    );
}

// --- access tokens ---

#[tokio::test]
async fn protected_route_requires_a_live_access_token() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, _) = h.register("alice", &pass("1")).await;
    assert_eq!(h.whoami(&session.access_token).await, StatusCode::OK);

    let missing = h.get("/test/whoami", None).await;
    exact_error(&missing, StatusCode::UNAUTHORIZED, "invalid_token");
    for bad in ["nope", "", &URL_SAFE_NO_PAD.encode([1u8; 32])] {
        let reply = h.get("/test/whoami", Some(&Token(bad.into()))).await;
        exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_token");
    }
    let basic = h
        .request(Method::GET, "/test/whoami", None, None, "10.9.9.9:1")
        .await;
    assert_eq!(basic.status, StatusCode::UNAUTHORIZED);

    h.advance(ACCESS_TTL_SECS + 1);
    let expired = h.get("/test/whoami", Some(&session.access_token)).await;
    exact_error(&expired, StatusCode::UNAUTHORIZED, "invalid_token");

    let (login, _) = h.login("alice", &pass("1")).await;
    let reply = h
        .post("/auth/logout", &(), Some(&login.session.access_token))
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.whoami(&login.session.access_token).await,
        StatusCode::UNAUTHORIZED
    );
}

// --- registration ---

#[tokio::test]
async fn registration_then_login_returns_the_same_bundle() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, account) = h.register("Alice", &pass("1")).await;
    assert_eq!(session.username, "alice");
    assert!(!session.is_admin);
    assert_eq!(session.access_expires_in, 900);
    let registered = h.key_bundle(&session.access_token).await;

    let (login, keys) = h.login("ALICE", &pass("1")).await;
    assert_eq!(login.key_bundle, registered);
    assert_same_vault_key(&account, &keys.unlock(&login.key_bundle).unwrap());
}

#[tokio::test]
async fn malformed_bundle_creates_nothing() {
    let h = harness(RegistrationPolicy::Open).await;
    let (reg, start) = Registration::start("alice", &pass("1")).unwrap();
    let response: RegisterStartResponse = h.post("/auth/register/start", &start, None).await.json();
    let (mut finish, _) = reg.finish(&response, Device::default(), 0).unwrap();
    finish.key_bundle = vec![1, 2, 3];
    let reply = h.post("/auth/register/finish", &finish, None).await;
    exact_error(&reply, StatusCode::BAD_REQUEST, "invalid_request");
    for table in ["users", "vaults", "key_bundles", "sessions"] {
        assert_eq!(h.count(table).await, 0, "{table}");
    }
}

#[tokio::test]
async fn vault_id_in_use_creates_nothing() {
    let h = harness(RegistrationPolicy::Open).await;
    let (_, existing) = h.register("alice", &pass("1")).await;
    let (reg, start) = Registration::start("bob", &pass("2")).unwrap();
    let response: RegisterStartResponse = h.post("/auth/register/start", &start, None).await.json();
    let (mut finish, account) = reg.finish(&response, Device::default(), 0).unwrap();
    finish.key_bundle = KeyBundle::create(
        existing.vault_id,
        &account.vault_key,
        &AuthSecret::from_bytes(&[1; 64]).unwrap(),
        &account.recovery_key,
    )
    .unwrap()
    .to_bytes();
    let reply = h.post("/auth/register/finish", &finish, None).await;
    exact_error(&reply, StatusCode::BAD_REQUEST, "invalid_request");
    for table in ["users", "vaults", "key_bundles", "sessions"] {
        assert_eq!(h.count(table).await, 1, "{table}");
    }
}

#[tokio::test]
async fn invalid_requests_are_uniform_json() {
    let h = harness(RegistrationPolicy::Open).await;
    let reply = h
        .request(
            Method::POST,
            "/auth/login/start",
            Some("{not json".into()),
            None,
            "10.8.8.8:1",
        )
        .await;
    exact_error(&reply, StatusCode::BAD_REQUEST, "invalid_request");
    let reply = h
        .post(
            "/auth/register/start",
            &serde_json::json!({"username": "al ice", "registration_request": "AAAA"}),
            None,
        )
        .await;
    exact_error(&reply, StatusCode::BAD_REQUEST, "invalid_username");
    let reply = h
        .post(
            "/auth/login/start",
            &serde_json::json!({"username": "alice", "credential_request": "!!"}),
            None,
        )
        .await;
    exact_error(&reply, StatusCode::BAD_REQUEST, "invalid_request");
}

#[tokio::test]
async fn duplicate_username_is_taken() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("bob", &pass("1")).await;
    let (reg, start) = Registration::start("BOB", &pass("2")).unwrap();
    let response: RegisterStartResponse = h.post("/auth/register/start", &start, None).await.json();
    let (finish, _) = reg.finish(&response, Device::default(), 0).unwrap();
    let reply = h.post("/auth/register/finish", &finish, None).await;
    exact_error(&reply, StatusCode::CONFLICT, "username_taken");
    assert_eq!(h.count("users").await, 1);
    h.login("bob", &pass("1")).await;
}

#[tokio::test]
async fn concurrent_registrations_leave_one_account() {
    let h = harness(RegistrationPolicy::Open).await;
    let mut finishes = Vec::new();
    for tag in ["a", "b"] {
        let (reg, start) = Registration::start("carol", &pass(tag)).unwrap();
        let response: RegisterStartResponse =
            h.post("/auth/register/start", &start, None).await.json();
        finishes.push(reg.finish(&response, Device::default(), 0).unwrap().0);
    }
    let (a, b) = tokio::join!(
        h.post("/auth/register/finish", &finishes[0], None),
        h.post("/auth/register/finish", &finishes[1], None),
    );
    let mut statuses = [a.status, b.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);
    assert_eq!(h.count("users").await, 1);
    assert_eq!(h.count("vaults").await, 1);
}

#[tokio::test]
async fn closed_registration_reveals_nothing() {
    let h = harness(RegistrationPolicy::Closed).await;
    user::create(
        &h.state.pool,
        &h.state.auth.setup,
        "taken",
        &pass("1"),
        false,
    )
    .await
    .unwrap();
    let mut bodies = Vec::new();
    for name in ["taken", "unused"] {
        let (reg, start) = Registration::start(name, &pass("1")).unwrap();
        let reply = h.post("/auth/register/start", &start, None).await;
        exact_error(&reply, StatusCode::FORBIDDEN, "registration_closed");
        bodies.push(reply.body);

        // A finish built against a throwaway open server is refused the same way.
        let open = harness(RegistrationPolicy::Open).await;
        let response: RegisterStartResponse =
            open.post("/auth/register/start", &start, None).await.json();
        let (finish, _) = reg.finish(&response, Device::default(), 0).unwrap();
        let reply = h.post("/auth/register/finish", &finish, None).await;
        exact_error(&reply, StatusCode::FORBIDDEN, "registration_closed");
        bodies.push(reply.body);
    }
    assert!(bodies.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(h.count("users").await, 1);
}

#[tokio::test]
async fn long_device_name_is_rejected() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("1")).await;
    let ok = "é".repeat(64);
    let (login, _) = h
        .login_as(
            "alice",
            &pass("1"),
            Device {
                name: ok.clone(),
                kind: SessionKind::Native,
            },
        )
        .await
        .unwrap();
    let list: Vec<SessionInfo> = h
        .get("/auth/sessions", Some(&login.session.access_token))
        .await
        .json();
    assert!(list.iter().any(|s| s.device_name == ok));

    let err = h
        .login_as(
            "alice",
            &pass("1"),
            Device {
                name: "x".repeat(65),
                kind: SessionKind::Native,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, r#"{"error":"invalid_request"}"#);
}

// --- login ---

#[tokio::test]
async fn wrong_passphrase_and_unknown_user_fail_alike() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("1")).await;
    let sessions_before = h.count("sessions").await;

    // The client detects the failure itself...
    assert_eq!(
        h.login("alice", &pass("1")).await.0.session.username,
        "alice"
    );
    let err = h
        .login_as("alice", &pass("2"), Device::default())
        .await
        .unwrap_err();
    assert!(err.contains("InvalidCredentials"), "{err}");

    // ...and a finish that doesn't prove the passphrase gets the same body
    // for a wrong passphrase and an unknown user.
    let mut sizes = Vec::new();
    for name in ["alice", "nobody"] {
        let (_, start) = Login::start(name, &pass("2")).unwrap();
        let reply = h.post("/auth/login/start", &start, None).await;
        assert_eq!(reply.status, StatusCode::OK);
        let response: LoginStartResponse = reply.json();
        sizes.push(response.credential_response.len());
        let finish = LoginFinishRequest {
            handshake_id: response.handshake_id,
            credential_finalization: vec![7; 64],
            device_name: String::new(),
            session_kind: SessionKind::Native,
        };
        let reply = h.post("/auth/login/finish", &finish, None).await;
        exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_credentials");
        assert!(!reply.body.windows(10).any(|w| w == b"key_bundle"));
    }
    assert_eq!(sizes[0], sizes[1]);
    assert_eq!(h.count("sessions").await, sessions_before + 1);
}

#[tokio::test]
async fn unknown_user_response_is_stable() {
    let h = harness(RegistrationPolicy::Open).await;
    let (_, start) = Login::start("nobody", "whatever passphrase").unwrap();
    let first: LoginStartResponse = h.post("/auth/login/start", &start, None).await.json();
    let second: LoginStartResponse = h.post("/auth/login/start", &start, None).await.json();
    // The OPRF evaluation (the first 32 bytes) is derived from the seed and
    // username, as it is for a real account.
    assert_eq!(
        first.credential_response[..32],
        second.credential_response[..32]
    );

    let (_, other) = Login::start("someone", "whatever passphrase").unwrap();
    let other_response: LoginStartResponse = h
        .post(
            "/auth/login/start",
            &LoginStartRequest {
                username: "someone".into(),
                credential_request: start.credential_request.clone(),
            },
            None,
        )
        .await
        .json();
    assert_ne!(
        first.credential_response[..32],
        other_response.credential_response[..32]
    );
    drop(other);
}

#[tokio::test]
async fn late_and_replayed_finishes_fail() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("1")).await;

    let (login, start) = Login::start("alice", &pass("1")).unwrap();
    let response: LoginStartResponse = h.post("/auth/login/start", &start, None).await.json();
    let (finish, _) = login.finish(&response, Device::default()).unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    let reply = h.post("/auth/login/finish", &finish, None).await;
    exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_credentials");

    let (login, start) = Login::start("alice", &pass("1")).unwrap();
    let response: LoginStartResponse = h.post("/auth/login/start", &start, None).await.json();
    let (finish, _) = login.finish(&response, Device::default()).unwrap();
    assert_eq!(
        h.post("/auth/login/finish", &finish, None).await.status,
        StatusCode::OK
    );
    let sessions = h.count("sessions").await;
    let reply = h.post("/auth/login/finish", &finish, None).await;
    exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_credentials");
    assert_eq!(h.count("sessions").await, sessions);
}

// --- sessions ---

#[tokio::test]
async fn refresh_rotates_and_detects_reuse() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, _) = h.register("alice", &pass("1")).await;
    let reply = h.refresh(&session.refresh_token).await;
    assert_eq!(reply.status, StatusCode::OK);
    let tokens: Tokens = reply.json();
    assert_eq!(h.whoami(&tokens.access_token).await, StatusCode::OK);
    assert_eq!(
        h.whoami(&session.access_token).await,
        StatusCode::UNAUTHORIZED
    );

    let reused = h.refresh(&session.refresh_token).await;
    exact_error(&reused, StatusCode::UNAUTHORIZED, "session_ended");
    assert_eq!(
        h.whoami(&tokens.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    exact_error(
        &h.refresh(&tokens.refresh_token).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );
    exact_error(
        &h.refresh(&Token("garbage".into())).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );
}

#[tokio::test]
async fn sessions_are_listed_and_revocable_per_user() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("1")).await;
    let laptop = Device {
        name: "Work laptop".into(),
        kind: SessionKind::Native,
    };
    let browser = Device {
        name: "Browser".into(),
        kind: SessionKind::Browser,
    };
    let (a, _) = h.login_as("alice", &pass("1"), laptop).await.unwrap();
    let (b, _) = h.login_as("alice", &pass("1"), browser).await.unwrap();

    let list: Vec<SessionInfo> = h
        .get("/auth/sessions", Some(&a.session.access_token))
        .await
        .json();
    assert_eq!(list.len(), 3);
    let current: Vec<_> = list.iter().filter(|s| s.current).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].device_name, "Work laptop");
    assert_eq!(current[0].id, a.session.session_id);
    assert_eq!(current[0].session_kind, SessionKind::Native);
    assert_eq!(current[0].created_at, T0);
    let other = list.iter().find(|s| s.id == b.session.session_id).unwrap();
    assert_eq!(other.session_kind, SessionKind::Browser);

    // Another user can't see or revoke alice's sessions.
    let (mallory, _) = h.register("mallory", &pass("m")).await;
    let list: Vec<SessionInfo> = h
        .get("/auth/sessions", Some(&mallory.access_token))
        .await
        .json();
    assert_eq!(list.len(), 1);
    let reply = h
        .delete(
            &format!("/auth/sessions/{}", b.session.session_id),
            Some(&mallory.access_token),
        )
        .await;
    exact_error(&reply, StatusCode::NOT_FOUND, "not_found");
    assert_eq!(h.whoami(&b.session.access_token).await, StatusCode::OK);
    let reply = h
        .delete("/auth/sessions/not-an-id", Some(&mallory.access_token))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);

    // Alice revokes the browser session from the laptop.
    let reply = h
        .delete(
            &format!("/auth/sessions/{}", b.session.session_id),
            Some(&a.session.access_token),
        )
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.whoami(&b.session.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    exact_error(
        &h.refresh(&b.session.refresh_token).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );
    assert_eq!(h.whoami(&a.session.access_token).await, StatusCode::OK);
}

#[tokio::test]
async fn logout_ends_both_tokens() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, _) = h.register("alice", &pass("1")).await;
    let reply = h
        .post("/auth/logout", &(), Some(&session.access_token))
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.whoami(&session.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    exact_error(
        &h.refresh(&session.refresh_token).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );
}

#[tokio::test]
async fn browser_sessions_end_after_twelve_hours() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("1")).await;
    let browser = Device {
        name: String::new(),
        kind: SessionKind::Browser,
    };
    let (b, _) = h.login_as("alice", &pass("1"), browser).await.unwrap();
    let (n, _) = h.login("alice", &pass("1")).await;
    h.advance(13 * 3600);
    exact_error(
        &h.refresh(&b.session.refresh_token).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );
    assert_eq!(
        h.refresh(&n.session.refresh_token).await.status,
        StatusCode::OK
    );

    h.advance(31 * 24 * 3600);
    exact_error(
        &h.refresh(&n.session.refresh_token).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );
}

// --- key bundle, passphrase change, recovery ---

#[tokio::test]
async fn key_bundle_matches_login_and_needs_a_token() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("1")).await;
    let (login, _) = h.login("alice", &pass("1")).await;
    let token = &login.session.access_token;
    assert_eq!(h.key_bundle(token).await, login.key_bundle);
    assert_eq!(h.key_bundle(token).await, login.key_bundle);
    exact_error(
        &h.get("/auth/key-bundle", None).await,
        StatusCode::UNAUTHORIZED,
        "invalid_token",
    );
}

#[tokio::test]
async fn passphrase_change_signs_out_other_devices() {
    let h = harness(RegistrationPolicy::Open).await;
    let (_, account) = h.register("alice", &pass("old")).await;
    let (a, _) = h.login("alice", &pass("old")).await;
    let (b, _) = h.login("alice", &pass("old")).await;

    let reply = h
        .change_passphrase(&a.session.access_token, &pass("old"), &pass("new"), |_| {})
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.error());

    assert_eq!(h.whoami(&a.session.access_token).await, StatusCode::OK);
    assert_eq!(
        h.whoami(&b.session.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    exact_error(
        &h.refresh(&b.session.refresh_token).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );

    let new_bundle = h.key_bundle(&a.session.access_token).await;
    assert_ne!(new_bundle, a.key_bundle);
    let (login, keys) = h.login("alice", &pass("new")).await;
    assert_eq!(login.key_bundle, new_bundle);
    assert_same_vault_key(&account, &keys.unlock(&login.key_bundle).unwrap());
    assert!(
        h.login_as("alice", &pass("old"), Device::default())
            .await
            .is_err()
    );
    let bundle = KeyBundle::from_bytes(&new_bundle).unwrap();
    assert_same_vault_key(
        &account,
        &bundle.unlock_with_recovery(&account.recovery_key).unwrap(),
    );
}

#[tokio::test]
async fn wrong_current_passphrase_changes_nothing() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, _) = h.register("alice", &pass("old")).await;
    let before = h.stored().await;
    let token = &session.access_token;

    let (change, start) = PassphraseChange::start(&pass("wrong"), &pass("new")).unwrap();
    let response: PassphraseStartResponse = h
        .post("/auth/passphrase/start", &start, Some(token))
        .await
        .json();
    let bundle = h.key_bundle(token).await;
    assert_eq!(
        change.finish(&response, &bundle).unwrap_err(),
        AuthError::InvalidCredentials
    );

    // A finish that doesn't prove the current passphrase is refused.
    let (change, start) = PassphraseChange::start(&pass("old"), &pass("new")).unwrap();
    let response: PassphraseStartResponse = h
        .post("/auth/passphrase/start", &start, Some(token))
        .await
        .json();
    let (mut finish, _) = change.finish(&response, &bundle).unwrap();
    finish.credential_finalization = vec![7; 64];
    let reply = h
        .post("/auth/passphrase/finish", &finish, Some(token))
        .await;
    exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_credentials");

    assert_eq!(h.stored().await, before);
    h.login("alice", &pass("old")).await;
}

#[tokio::test]
async fn replacement_bundles_must_keep_the_vault_and_recovery_entry() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, account) = h.register("alice", &pass("old")).await;
    let before = h.stored().await;
    let auth = AuthSecret::from_bytes(&[1; 64]).unwrap();

    let altered = KeyBundle::create(
        account.vault_id,
        &account.vault_key,
        &auth,
        &RecoveryKey::generate().unwrap(),
    )
    .unwrap()
    .to_bytes();
    let reply = h
        .change_passphrase(&session.access_token, &pass("old"), &pass("new"), |f| {
            f.key_bundle = altered
        })
        .await;
    exact_error(&reply, StatusCode::BAD_REQUEST, "invalid_request");

    let other_vault = KeyBundle::create(
        VaultId::from_parts(1, 1),
        &account.vault_key,
        &auth,
        &account.recovery_key,
    )
    .unwrap()
    .to_bytes();
    let reply = h
        .change_passphrase(&session.access_token, &pass("old"), &pass("new"), |f| {
            f.key_bundle = other_vault
        })
        .await;
    exact_error(&reply, StatusCode::BAD_REQUEST, "invalid_request");

    assert_eq!(h.stored().await, before);
    assert_eq!(h.whoami(&session.access_token).await, StatusCode::OK);
}

#[tokio::test]
async fn recovery_resets_the_passphrase_and_every_session() {
    let h = harness(RegistrationPolicy::Open).await;
    let (first, account) = h.register("alice", &pass("old")).await;
    let (other, _) = h.login("alice", &pass("old")).await;
    let shown = account.recovery_key.to_display();

    let session = h
        .recover("ALICE", RecoveryKey::parse(&shown).unwrap(), &pass("new"))
        .await
        .unwrap_or_else(|r| panic!("{}", r.error()));
    assert_eq!(session.username, "alice");
    assert_eq!(h.whoami(&session.access_token).await, StatusCode::OK);
    for old in [&first.access_token, &other.session.access_token] {
        assert_eq!(h.whoami(old).await, StatusCode::UNAUTHORIZED);
    }
    exact_error(
        &h.refresh(&first.refresh_token).await,
        StatusCode::UNAUTHORIZED,
        "session_ended",
    );
    let (login, keys) = h.login("alice", &pass("new")).await;
    assert_same_vault_key(&account, &keys.unlock(&login.key_bundle).unwrap());
    assert!(
        h.login_as("alice", &pass("old"), Device::default())
            .await
            .is_err()
    );

    // The same recovery key works again.
    h.recover("alice", RecoveryKey::parse(&shown).unwrap(), &pass("newer"))
        .await
        .unwrap_or_else(|r| panic!("{}", r.error()));
    h.login("alice", &pass("newer")).await;
}

#[tokio::test]
async fn handshakes_started_before_a_credential_change_cannot_finish() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, account) = h.register("alice", &pass("old")).await;
    let token = &session.access_token;

    // An attacker who knows the old passphrase holds a login open...
    let (login, start) = Login::start("alice", &pass("old")).unwrap();
    let response: LoginStartResponse = h.post("/auth/login/start", &start, None).await.json();
    let (login_finish, _) = login.finish(&response, Device::default()).unwrap();

    // ...and a passphrase change is started, checked against the old record.
    let bundle = h.key_bundle(token).await;
    let (change, start) = PassphraseChange::start(&pass("old"), &pass("stale")).unwrap();
    let response: PassphraseStartResponse = h
        .post("/auth/passphrase/start", &start, Some(token))
        .await
        .json();
    let (stale_change, _) = change.finish(&response, &bundle).unwrap();

    // The owner changes the passphrase from the same session.
    let reply = h
        .change_passphrase(token, &pass("old"), &pass("new"), |_| {})
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    let sessions = h.count("sessions").await;

    let reply = h.post("/auth/login/finish", &login_finish, None).await;
    exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_credentials");
    let reply = h
        .post("/auth/passphrase/finish", &stale_change, Some(token))
        .await;
    exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_credentials");
    assert_eq!(h.count("sessions").await, sessions);
    let (login, keys) = h.login("alice", &pass("new")).await;
    assert_same_vault_key(&account, &keys.unlock(&login.key_bundle).unwrap());
}

#[tokio::test]
async fn wrong_recovery_key_and_unknown_user_fail_alike() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("old")).await;
    let before = h.stored().await;
    let mut bodies = Vec::new();
    for name in ["alice", "nobody"] {
        let reply = h
            .recover(name, RecoveryKey::generate().unwrap(), &pass("new"))
            .await
            .unwrap_err();
        exact_error(&reply, StatusCode::UNAUTHORIZED, "invalid_credentials");
        bodies.push(reply.body);
    }
    assert_eq!(bodies[0], bodies[1]);
    assert_eq!(h.stored().await, before);
    h.login("alice", &pass("old")).await;
}

// --- errors and storage ---

#[tokio::test]
async fn database_holds_no_tokens_verifier_or_passphrase() {
    let h = harness(RegistrationPolicy::Open).await;
    let (session, account) = h.register("alice", &pass("1")).await;
    let (login, _) = h.login("alice", &pass("1")).await;
    let tokens: Tokens = h.refresh(&login.session.refresh_token).await.json();
    let verifier = account.recovery_key.verifier();
    h.state.pool.close().await;

    let mut needles: Vec<Vec<u8>> = vec![MARKER.as_bytes().to_vec(), verifier.to_vec()];
    for token in [
        &session.access_token,
        &session.refresh_token,
        &login.session.access_token,
        &login.session.refresh_token,
        &tokens.access_token,
        &tokens.refresh_token,
    ] {
        needles.push(token.0.as_bytes().to_vec());
        needles.push(URL_SAFE_NO_PAD.decode(&token.0).unwrap());
    }
    let mut files = 0;
    for entry in std::fs::read_dir(h.dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if !path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("db")
        {
            continue;
        }
        files += 1;
        let bytes = std::fs::read(&path).unwrap();
        for needle in &needles {
            assert!(
                !bytes.windows(needle.len()).any(|w| w == needle.as_slice()),
                "{} contains a secret",
                path.display()
            );
        }
    }
    assert!(files >= 1);
}

#[tokio::test]
async fn no_cookies_anywhere() {
    let h = harness(RegistrationPolicy::Open).await;
    let mut replies = Vec::new();
    let (reg, start) = Registration::start("alice", &pass("1")).unwrap();
    let r = h.post("/auth/register/start", &start, None).await;
    let (finish, _) = reg.finish(&r.json(), Device::default(), 0).unwrap();
    replies.push(r);
    replies.push(h.post("/auth/register/finish", &finish, None).await);

    let (login, start) = Login::start("alice", &pass("1")).unwrap();
    let r = h.post("/auth/login/start", &start, None).await;
    let (finish, _) = login.finish(&r.json(), Device::default()).unwrap();
    replies.push(r);
    let r = h.post("/auth/login/finish", &finish, None).await;
    let session: LoginFinishResponse = r.json();
    replies.push(r);
    let r = h.refresh(&session.session.refresh_token).await;
    let tokens: Tokens = r.json();
    replies.push(r);
    replies.push(h.get("/auth/sessions", Some(&tokens.access_token)).await);
    replies.push(
        h.post("/auth/logout", &(), Some(&tokens.access_token))
            .await,
    );
    replies.push(h.refresh(&tokens.refresh_token).await);
    for reply in replies {
        assert!(reply.headers.get(header::SET_COOKIE).is_none());
    }
}

// --- rate limits ---

#[tokio::test]
async fn login_is_limited_per_username_across_addresses() {
    let h = harness(RegistrationPolicy::Open).await;
    h.register("alice", &pass("1")).await;
    for name in ["alice", "nobody"] {
        let (_, start) = Login::start(name, "guess").unwrap();
        for i in 0..10 {
            let reply = h.post("/auth/login/start", &start, None).await;
            assert_eq!(reply.status, StatusCode::OK, "{name} attempt {i}");
        }
        let reply = h.post("/auth/login/start", &start, None).await;
        assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS, "{name}");
        assert!(reply.headers.contains_key(header::RETRY_AFTER));
    }
    // A different user from another address is unaffected.
    h.register("bob", &pass("b")).await;
    h.login("bob", &pass("b")).await;
}

#[tokio::test]
async fn login_start_is_limited_per_address() {
    let h = harness(RegistrationPolicy::Open).await;
    let peer = "192.0.2.50:1000";
    for i in 0..10 {
        let (_, start) = Login::start(&format!("user{i}"), "guess").unwrap();
        let reply = h.post_from(peer, "/auth/login/start", &start, None).await;
        assert_eq!(reply.status, StatusCode::OK, "attempt {i}");
    }
    let (_, start) = Login::start("user10", "guess").unwrap();
    let reply = h.post_from(peer, "/auth/login/start", &start, None).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn recovery_and_registration_are_limited_per_address() {
    let h = harness(RegistrationPolicy::Open).await;
    let peer = "192.0.2.60:1000";
    let (_, start) =
        Recovery::start("alice", RecoveryKey::generate().unwrap(), &pass("n")).unwrap();
    for i in 0..5 {
        let reply = h.post_from(peer, "/auth/recover/start", &start, None).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let reply = h.post_from(peer, "/auth/recover/start", &start, None).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(reply.headers.contains_key(header::RETRY_AFTER));

    let (_, start) = Registration::start("alice", &pass("1")).unwrap();
    for _ in 0..5 {
        let reply = h
            .post_from(peer, "/auth/register/start", &start, None)
            .await;
        assert_eq!(reply.status, StatusCode::OK);
    }
    let reply = h
        .post_from(peer, "/auth/register/start", &start, None)
        .await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
}

// --- end to end ---

#[tokio::test]
async fn user_add_then_devices_change_and_recover() {
    let h = harness(RegistrationPolicy::Closed).await;
    let shown = user::create(
        &h.state.pool,
        &h.state.auth.setup,
        "Dana",
        &pass("1"),
        false,
    )
    .await
    .unwrap();

    // Two devices; the first sign-in is an ordinary login.
    let (laptop, laptop_keys) = h.login("dana", &pass("1")).await;
    let browser = Device {
        name: "Browser".into(),
        kind: SessionKind::Browser,
    };
    let (tab, tab_keys) = h.login_as("dana", &pass("1"), browser).await.unwrap();
    let vault_key = laptop_keys.unlock(&laptop.key_bundle).unwrap();
    let bundle = KeyBundle::from_bytes(&laptop.key_bundle).unwrap();
    let account = NewAccount {
        vault_id: bundle.vault_id(),
        vault_key,
        recovery_key: RecoveryKey::parse(&shown).unwrap(),
        keys: tab_keys,
    };
    assert_same_vault_key(&account, &account.keys.unlock(&tab.key_bundle).unwrap());

    let tokens: Tokens = h.refresh(&laptop.session.refresh_token).await.json();
    let reply = h
        .change_passphrase(&tokens.access_token, &pass("1"), &pass("2"), |_| {})
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert_eq!(
        h.whoami(&tab.session.access_token).await,
        StatusCode::UNAUTHORIZED
    );

    h.recover("dana", RecoveryKey::parse(&shown).unwrap(), &pass("3"))
        .await
        .unwrap_or_else(|r| panic!("{}", r.error()));
    assert_eq!(
        h.whoami(&tokens.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    let (login, keys) = h.login("dana", &pass("3")).await;
    assert_same_vault_key(&account, &keys.unlock(&login.key_bundle).unwrap());
    assert!(
        h.login_as("dana", &pass("2"), Device::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn user_add_refuses_bad_input_and_creates_nothing() {
    let h = harness(RegistrationPolicy::Closed).await;
    let (pool, setup) = (&h.state.pool, &h.state.auth.setup);
    user::create(pool, setup, "root", &pass("1"), true)
        .await
        .unwrap();
    let (login, _) = h.login("root", &pass("1")).await;
    assert!(login.session.is_admin);

    assert!(matches!(
        user::create(pool, setup, "al ice", &pass("1"), false).await,
        Err(user::UserError::InvalidUsername)
    ));
    assert!(matches!(
        user::create(pool, setup, "ROOT", &pass("2"), false).await,
        Err(user::UserError::UsernameTaken)
    ));
    assert!(matches!(
        user::create(pool, setup, "short", "elevenchars", false).await,
        Err(user::UserError::PassphraseTooShort)
    ));
    assert_eq!(h.count("users").await, 1);
    h.login("root", &pass("1")).await;
}
