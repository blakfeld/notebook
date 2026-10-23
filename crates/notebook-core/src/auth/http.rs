//! The `/auth` routes over HTTP: each flow in [`client`](super) driven to
//! completion against a server, and [`AuthSession`], the engine's
//! [`SessionSource`].
//!
//! [`AuthHttp`] is the platform boundary: `NativeAuthHttp` with the
//! `transport-native` feature, `WasmAuthHttp` with `transport-wasm`.
//!
//! # Refresh tokens
//!
//! The server rotates the refresh token on every use and revokes the
//! session if an old one is presented again, so two refreshes must never
//! race. [`SessionTokens`] is the one copy of the tokens, shared by the host
//! and the engine, and serializes every refresh: a caller that waited for
//! another's refresh uses its result instead of refreshing again.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::api::{
    ErrorBody, ErrorCode, KeyBundleResponse, LoginFinishResponse, LoginStartResponse,
    PassphraseStartResponse, RecoverStartResponse, RefreshOutcome, RefreshRequest,
    RegisterStartResponse, Session, Token, Tokens,
};
use super::{AuthError, Device, Login, PassphraseChange, Recovery, Registration};
use crate::crypto::{KeyBundle, RecoveryKey, VaultId, VaultKey};
use crate::sync::{AccessToken, RefreshError, SessionSource, Shared, Timer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

/// One `/auth` request. `body` is JSON. `Debug` omits the bearer token and
/// the body, which can hold a refresh token or registration secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    /// From the server root, such as `/auth/login/start`.
    pub path: String,
    pub bearer: Option<Token>,
    pub body: Option<Vec<u8>>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// The parsed `Retry-After` header, if any.
    pub retry_after: Option<Duration>,
}

/// Sends `/auth` requests to one server.
pub trait AuthHttp {
    /// Any response, whatever its status. `Err` means none arrived
    /// (connection refused, timeout, DNS); the message is for logs.
    fn send(&self, request: HttpRequest) -> impl Future<Output = Result<HttpResponse, String>>;
}

/// Why an `/auth` call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthClientError {
    /// No response from the server.
    Network,
    /// `429`: wait at least `retry_after` (when the server said).
    RateLimited { retry_after: Option<Duration> },
    /// The server refused the request with this code.
    Rejected(ErrorCode),
    /// Any other status, or a body that doesn't parse.
    Server(u16),
    /// Refused or failed on this device before or after the round trip:
    /// invalid input, a wrong passphrase, or a bundle that won't unlock.
    Auth(AuthError),
}

impl fmt::Display for AuthClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network => f.write_str("the server can't be reached"),
            Self::RateLimited { .. } => f.write_str("rate limited by the server"),
            Self::Rejected(code) => write!(f, "the server refused the request ({code:?})"),
            Self::Server(status) => write!(f, "unexpected server response (HTTP {status})"),
            Self::Auth(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for AuthClientError {}

impl From<AuthError> for AuthClientError {
    fn from(err: AuthError) -> Self {
        Self::Auth(err)
    }
}

/// A signed-in device: its new session and the unlocked vault.
#[derive(Debug)]
pub struct SignedIn {
    pub session: Session,
    pub vault_id: VaultId,
    pub vault_key: VaultKey,
    /// The key bundle as the server now has it.
    pub key_bundle: Vec<u8>,
}

/// The `/auth` routes on one server.
#[derive(Debug, Clone)]
pub struct AuthClient<H> {
    http: H,
}

impl<H: AuthHttp> AuthClient<H> {
    pub fn new(http: H) -> Self {
        Self { http }
    }

    /// Creates an account with a recovery key the user has already seen.
    /// `now_ms` (Unix milliseconds) dates the new vault's ID.
    pub async fn register(
        &self,
        username: &str,
        passphrase: &str,
        invite_code: &str,
        recovery_key: RecoveryKey,
        device: Device,
        now_ms: u64,
    ) -> Result<SignedIn, AuthClientError> {
        let (registration, start) = Registration::start(username, passphrase, invite_code)?;
        let response: RegisterStartResponse =
            self.post("/auth/register/start", None, &start).await?;
        let (finish, account) =
            registration.finish_with_recovery_key(&response, device, now_ms, recovery_key)?;
        let session: Session = self.post("/auth/register/finish", None, &finish).await?;
        Ok(SignedIn {
            session,
            vault_id: account.vault_id,
            vault_key: account.vault_key,
            key_bundle: finish.key_bundle,
        })
    }

    /// Signs in. An unknown username and a wrong passphrase are both
    /// [`AuthError::InvalidCredentials`].
    pub async fn login(
        &self,
        username: &str,
        passphrase: &str,
        device: Device,
    ) -> Result<SignedIn, AuthClientError> {
        let (login, start) = Login::start(username, passphrase)?;
        let response: LoginStartResponse = self.post("/auth/login/start", None, &start).await?;
        let (finish, keys) = login.finish(&response, device)?;
        let response: LoginFinishResponse = self.post("/auth/login/finish", None, &finish).await?;
        let vault_key = keys.unlock(&response.key_bundle)?;
        Ok(SignedIn {
            session: response.session,
            vault_id: bundle_vault(&response.key_bundle)?,
            vault_key,
            key_bundle: response.key_bundle,
        })
    }

    /// Replaces the passphrase using the recovery key, and signs in.
    pub async fn recover(
        &self,
        username: &str,
        recovery_key: RecoveryKey,
        new_passphrase: &str,
        device: Device,
    ) -> Result<SignedIn, AuthClientError> {
        let (recovery, start) = Recovery::start(username, recovery_key, new_passphrase)?;
        let response: RecoverStartResponse = self.post("/auth/recover/start", None, &start).await?;
        let (finish, keys) = recovery.finish(&response, device)?;
        let session: Session = self.post("/auth/recover/finish", None, &finish).await?;
        let vault_key = keys.unlock(&finish.key_bundle)?;
        Ok(SignedIn {
            session,
            vault_id: bundle_vault(&finish.key_bundle)?,
            vault_key,
            key_bundle: finish.key_bundle,
        })
    }

    /// Changes the passphrase of `access`'s account. Returns the new key
    /// bundle. A wrong current passphrase is
    /// [`AuthError::InvalidCredentials`], and nothing changes.
    pub async fn change_passphrase(
        &self,
        access: &Token,
        old_passphrase: &str,
        new_passphrase: &str,
    ) -> Result<Vec<u8>, AuthClientError> {
        let (change, start) = PassphraseChange::start(old_passphrase, new_passphrase)?;
        let current = self.key_bundle(access).await?;
        let response: PassphraseStartResponse = self
            .post("/auth/passphrase/start", Some(access), &start)
            .await?;
        let (finish, _keys) = change.finish(&response, &current)?;
        self.post_empty("/auth/passphrase/finish", Some(access), &finish)
            .await?;
        Ok(finish.key_bundle)
    }

    /// The account's current key bundle.
    pub async fn key_bundle(&self, access: &Token) -> Result<Vec<u8>, AuthClientError> {
        let response: KeyBundleResponse = self
            .call(HttpRequest {
                method: Method::Get,
                path: "/auth/key-bundle".into(),
                bearer: Some(access.clone()),
                body: None,
            })
            .await
            .and_then(|r| decode(&r))?;
        Ok(response.key_bundle)
    }

    /// Exchanges a refresh token. No response is
    /// [`RefreshOutcome::Transient`].
    pub async fn refresh(&self, refresh_token: &Token) -> RefreshOutcome {
        let body = json(&RefreshRequest {
            refresh_token: refresh_token.clone(),
        });
        let request = HttpRequest {
            method: Method::Post,
            path: "/auth/refresh".into(),
            bearer: None,
            body: Some(body),
        };
        match self.http.send(request).await {
            Ok(response) => RefreshOutcome::classify(Some(response.status), &response.body),
            Err(_) => RefreshOutcome::classify(None, &[]),
        }
    }

    /// Revokes another session of the same account, such as the one this
    /// device used before signing in again.
    pub async fn revoke_session(
        &self,
        access: &Token,
        session_id: &str,
    ) -> Result<(), AuthClientError> {
        // Session ids are server-issued and URL-safe; refuse anything else
        // rather than build an odd path.
        if session_id.is_empty()
            || !session_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(AuthClientError::Rejected(ErrorCode::NotFound));
        }
        self.call(HttpRequest {
            method: Method::Delete,
            path: format!("/auth/sessions/{session_id}"),
            bearer: Some(access.clone()),
            body: None,
        })
        .await
        .map(drop)
    }

    /// Revokes `access`'s session.
    pub async fn logout(&self, access: &Token) -> Result<(), AuthClientError> {
        self.call(HttpRequest {
            method: Method::Post,
            path: "/auth/logout".into(),
            bearer: Some(access.clone()),
            body: None,
        })
        .await
        .map(drop)
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &'static str,
        bearer: Option<&Token>,
        body: &impl Serialize,
    ) -> Result<T, AuthClientError> {
        decode(&self.post_empty(path, bearer, body).await?)
    }

    async fn post_empty(
        &self,
        path: &'static str,
        bearer: Option<&Token>,
        body: &impl Serialize,
    ) -> Result<HttpResponse, AuthClientError> {
        self.call(HttpRequest {
            method: Method::Post,
            path: path.into(),
            bearer: bearer.cloned(),
            body: Some(json(body)),
        })
        .await
    }

    /// Sends `request` and returns a `2xx` response.
    async fn call(&self, request: HttpRequest) -> Result<HttpResponse, AuthClientError> {
        let response = self
            .http
            .send(request)
            .await
            .map_err(|_| AuthClientError::Network)?;
        match response.status {
            200..=299 => Ok(response),
            429 => Err(AuthClientError::RateLimited {
                retry_after: response.retry_after,
            }),
            status => Err(match serde_json::from_slice::<ErrorBody>(&response.body) {
                Ok(body) => AuthClientError::Rejected(body.error),
                Err(_) => AuthClientError::Server(status),
            }),
        }
    }
}

fn json(body: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(body).expect("auth request bodies always serialize")
}

fn decode<T: DeserializeOwned>(response: &HttpResponse) -> Result<T, AuthClientError> {
    serde_json::from_slice(&response.body).map_err(|_| AuthClientError::Server(response.status))
}

fn bundle_vault(bundle: &[u8]) -> Result<VaultId, AuthClientError> {
    KeyBundle::from_bytes(bundle)
        .map(|b| b.vault_id())
        .map_err(|e| AuthClientError::Auth(e.into()))
}

/// What the host hears after a successful refresh: first the rotated refresh
/// token alone, to store at once, then again with the server's current key
/// bundle once it has been fetched (to notice a passphrase changed on
/// another device).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub refresh_token: Token,
    pub key_bundle: Option<Vec<u8>>,
}

#[cfg(not(target_arch = "wasm32"))]
type OnRefresh = std::sync::Arc<dyn Fn(&Refreshed) + Send + Sync>;
#[cfg(target_arch = "wasm32")]
type OnRefresh = std::rc::Rc<dyn Fn(&Refreshed)>;

#[derive(Default)]
struct TokenState {
    /// `expires_at` is on the engine's clock; a token refreshed by the host
    /// is stored as already expired, so the engine refreshes before using
    /// it.
    access: Option<AccessToken>,
    refresh: Option<Token>,
    /// Bumped by every change, so a waiter can tell someone else refreshed.
    generation: u64,
}

/// The device's tokens, shared by the host and the engine. Cheap to clone.
#[derive(Clone)]
pub struct SessionTokens {
    state: Shared<TokenState>,
    refreshing: std::sync::Arc<futures::lock::Mutex<()>>,
    on_refresh: Option<OnRefresh>,
}

impl SessionTokens {
    /// Tokens starting from a stored refresh token (or none: signed out).
    /// `on_refresh` runs after every successful refresh, on whichever side
    /// refreshed; it should store the new refresh token.
    pub fn new(refresh: Option<Token>, on_refresh: Option<OnRefresh>) -> Self {
        Self {
            state: Shared::new(TokenState {
                refresh,
                ..TokenState::default()
            }),
            refreshing: Default::default(),
            on_refresh,
        }
    }

    /// Installs a new session after signing in (again). The access token is
    /// used by the host; the engine refreshes before it syncs.
    pub fn install(&self, session: &Session) {
        let mut state = self.state.lock();
        state.access = Some(AccessToken {
            token: session.access_token.clone(),
            expires_at: 0,
        });
        state.refresh = Some(session.refresh_token.clone());
        state.generation += 1;
    }

    /// Forgets both tokens (signing out).
    pub fn clear(&self) {
        let mut state = self.state.lock();
        state.access = None;
        state.refresh = None;
        state.generation += 1;
    }

    pub fn refresh_token(&self) -> Option<Token> {
        self.state.lock().refresh.clone()
    }

    /// Runs `op` with the current access token, refreshing once and retrying
    /// if there is none or the server says it is no longer valid. A session
    /// that has ended is [`ErrorCode::SessionEnded`].
    pub async fn authed<H: AuthHttp, R, Fut>(
        &self,
        client: &AuthClient<H>,
        op: impl Fn(Token) -> Fut,
    ) -> Result<R, AuthClientError>
    where
        Fut: Future<Output = Result<R, AuthClientError>>,
    {
        let (access, generation) = {
            let state = self.state.lock();
            (
                state.access.as_ref().map(|a| a.token.clone()),
                state.generation,
            )
        };
        if let Some(token) = access {
            match op(token).await {
                Err(AuthClientError::Rejected(ErrorCode::InvalidToken)) => {}
                other => return other,
            }
        }
        self.refresh(client, generation, |_| 0)
            .await
            .map_err(|e| match e {
                RefreshError::SessionEnded => AuthClientError::Rejected(ErrorCode::SessionEnded),
                RefreshError::Unavailable => AuthClientError::Network,
            })?;
        let token = self.state.lock().access.as_ref().map(|a| a.token.clone());
        match token {
            Some(token) => op(token).await,
            None => Err(AuthClientError::Rejected(ErrorCode::SessionEnded)),
        }
    }

    /// Refreshes unless someone else did since `seen` (a generation read
    /// before deciding to refresh). `expires_at` turns the server's
    /// `access_expires_in` (seconds) into the stored expiry.
    async fn refresh<H: AuthHttp>(
        &self,
        client: &AuthClient<H>,
        seen: u64,
        expires_at: impl Fn(u64) -> u64,
    ) -> Result<(), RefreshError> {
        let _guard = self.refreshing.lock().await;
        let refresh = {
            let state = self.state.lock();
            if state.generation != seen {
                return match state.access {
                    Some(_) => Ok(()),
                    None => Err(RefreshError::SessionEnded),
                };
            }
            state.refresh.clone().ok_or(RefreshError::SessionEnded)?
        };
        match client.refresh(&refresh).await {
            RefreshOutcome::Refreshed(Tokens {
                access_token,
                access_expires_in,
                refresh_token,
            }) => {
                {
                    let mut state = self.state.lock();
                    state.access = Some(AccessToken {
                        token: access_token.clone(),
                        expires_at: expires_at(access_expires_in),
                    });
                    state.refresh = Some(refresh_token.clone());
                    state.generation += 1;
                }
                if let Some(on_refresh) = &self.on_refresh {
                    // Store the rotated token before anything else can fail
                    // or be interrupted: presenting the old one again would
                    // end the session.
                    on_refresh(&Refreshed {
                        refresh_token: refresh_token.clone(),
                        key_bundle: None,
                    });
                    if let Ok(key_bundle) = client.key_bundle(&access_token).await {
                        on_refresh(&Refreshed {
                            refresh_token,
                            key_bundle: Some(key_bundle),
                        });
                    }
                }
                Ok(())
            }
            RefreshOutcome::SessionEnded => {
                let mut state = self.state.lock();
                state.access = None;
                state.refresh = None;
                state.generation += 1;
                Err(RefreshError::SessionEnded)
            }
            RefreshOutcome::Transient => Err(RefreshError::Unavailable),
        }
    }
}

/// The engine's [`SessionSource`]: the shared [`SessionTokens`], refreshed
/// through this side's own client and dated on the engine's clock.
pub struct AuthSession<H, M> {
    client: AuthClient<H>,
    tokens: SessionTokens,
    timer: M,
}

impl<H: AuthHttp, M: Timer> AuthSession<H, M> {
    /// `timer` must read the same clock as the engine's.
    pub fn new(client: AuthClient<H>, tokens: SessionTokens, timer: M) -> Self {
        Self {
            client,
            tokens,
            timer,
        }
    }
}

/// Treat an access token as expired this long before the server does.
const EXPIRY_MARGIN_MS: u64 = 30_000;

impl<H: AuthHttp, M: Timer> SessionSource for AuthSession<H, M> {
    /// With only a refresh token (after a relaunch or an offline unlock),
    /// an already-expired placeholder, so the engine refreshes before its
    /// first request rather than treating the device as signed out.
    fn access_token(&self) -> Option<AccessToken> {
        let state = self.tokens.state.lock();
        match (&state.access, &state.refresh) {
            (Some(access), _) => Some(access.clone()),
            (None, Some(_)) => Some(AccessToken {
                token: Token(String::new()),
                expires_at: 0,
            }),
            (None, None) => None,
        }
    }

    async fn refresh(&self) -> Result<(), RefreshError> {
        let seen = self.tokens.state.lock().generation;
        let now = self.timer.now();
        self.tokens
            .refresh(&self.client, seen, |expires_in| {
                (now + expires_in.saturating_mul(1000)).saturating_sub(EXPIRY_MARGIN_MS)
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use futures::executor::block_on;

    use super::*;

    /// Canned responses, in order, and every request sent.
    #[derive(Clone, Default)]
    struct Canned {
        responses: Rc<RefCell<VecDeque<Result<HttpResponse, String>>>>,
        sent: Rc<RefCell<Vec<HttpRequest>>>,
    }

    impl Canned {
        fn reply(&self, status: u16, body: &str) -> &Self {
            self.responses.borrow_mut().push_back(Ok(HttpResponse {
                status,
                body: body.as_bytes().to_vec(),
                retry_after: None,
            }));
            self
        }

        fn fail(&self) -> &Self {
            self.responses.borrow_mut().push_back(Err("refused".into()));
            self
        }

        fn paths(&self) -> Vec<(Method, String, bool)> {
            self.sent
                .borrow()
                .iter()
                .map(|r| (r.method, r.path.clone(), r.bearer.is_some()))
                .collect()
        }
    }

    impl AuthHttp for Canned {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, String> {
            self.sent.borrow_mut().push(request);
            self.responses
                .borrow_mut()
                .pop_front()
                .unwrap_or(Err("no canned response".into()))
        }
    }

    struct Clock(u64);

    impl Timer for Clock {
        fn now(&self) -> u64 {
            self.0
        }

        async fn sleep(&self, _: Duration) {}
    }

    fn token(s: &str) -> Token {
        Token(s.into())
    }

    fn tokens_json(access: &str, refresh: &str) -> String {
        format!(
            r#"{{"access_token":"{access}","access_expires_in":900,"refresh_token":"{refresh}"}}"#
        )
    }

    #[test]
    fn errors_are_mapped() {
        let http = Canned::default();
        let client = AuthClient::new(http.clone());
        http.fail();
        assert_eq!(
            block_on(client.logout(&token("a"))),
            Err(AuthClientError::Network)
        );
        http.responses.borrow_mut().push_back(Ok(HttpResponse {
            status: 429,
            body: Vec::new(),
            retry_after: Some(Duration::from_secs(3)),
        }));
        assert_eq!(
            block_on(client.logout(&token("a"))),
            Err(AuthClientError::RateLimited {
                retry_after: Some(Duration::from_secs(3))
            })
        );
        http.reply(403, r#"{"error":"invalid_invite"}"#);
        assert_eq!(
            block_on(client.logout(&token("a"))),
            Err(AuthClientError::Rejected(ErrorCode::InvalidInvite))
        );
        http.reply(500, "<html>");
        assert_eq!(
            block_on(client.logout(&token("a"))),
            Err(AuthClientError::Server(500))
        );
        http.reply(200, "not json");
        assert_eq!(
            block_on(client.key_bundle(&token("a"))),
            Err(AuthClientError::Server(200))
        );
    }

    #[test]
    fn local_refusals_send_nothing() {
        let http = Canned::default();
        let client = AuthClient::new(http.clone());
        let device = Device::default();
        let key = RecoveryKey::generate().unwrap();
        assert_eq!(
            block_on(client.register("alice", "short", "", key, device.clone(), 0)).unwrap_err(),
            AuthClientError::Auth(AuthError::PassphraseTooShort)
        );
        assert_eq!(
            block_on(client.login("no spaces allowed", "whatever", device)).unwrap_err(),
            AuthClientError::Auth(AuthError::InvalidUsername)
        );
        assert!(http.sent.borrow().is_empty());
    }

    #[test]
    fn request_paths() {
        let http = Canned::default();
        let client = AuthClient::new(http.clone());
        let device = Device::default();
        let passphrase = "correct horse battery";

        // Each flow stops at its first (refused) response; the path and
        // whether it carried a token are what's checked.
        let refused = r#"{"error":"invalid_request"}"#;
        for _ in 0..5 {
            http.reply(400, refused);
        }
        let key = RecoveryKey::generate().unwrap();
        let _ = block_on(client.register("alice", passphrase, "code", key, device.clone(), 0));
        let _ = block_on(client.login("alice", passphrase, device.clone()));
        let key = RecoveryKey::generate().unwrap();
        let _ = block_on(client.recover("alice", key, passphrase, device));
        let _ = block_on(client.change_passphrase(&token("a"), passphrase, passphrase));
        let _ = block_on(client.logout(&token("a")));
        http.reply(204, "");
        let _ = block_on(client.revoke_session(&token("a"), "01ABC"));
        assert_eq!(
            block_on(client.revoke_session(&token("a"), "../logout")),
            Err(AuthClientError::Rejected(ErrorCode::NotFound))
        );
        http.reply(200, &tokens_json("a2", "r2"));
        let _ = block_on(client.refresh(&token("r")));

        assert_eq!(
            http.paths(),
            [
                (Method::Post, "/auth/register/start", false),
                (Method::Post, "/auth/login/start", false),
                (Method::Post, "/auth/recover/start", false),
                (Method::Get, "/auth/key-bundle", true),
                (Method::Post, "/auth/logout", true),
                (Method::Delete, "/auth/sessions/01ABC", true),
                (Method::Post, "/auth/refresh", false),
            ]
            .map(|(m, p, b)| (m, p.to_owned(), b))
        );
        let refresh = &http.sent.borrow()[6];
        assert_eq!(
            refresh.body.as_deref(),
            Some(br#"{"refresh_token":"r"}"#.as_slice())
        );
    }

    #[test]
    fn engine_refresh_rotates_and_reports() {
        let http = Canned::default();
        http.reply(200, &tokens_json("a2", "r2"));
        http.reply(200, r#"{"key_bundle":"AQID"}"#);
        let seen = Rc::new(RefCell::new(None));
        let tokens = SessionTokens::new(Some(token("r1")), {
            // `OnRefresh` is `Send` on native, so record through a channel.
            let (tx, rx) = std::sync::mpsc::channel();
            *seen.borrow_mut() = Some(rx);
            Some(std::sync::Arc::new(move |r: &Refreshed| {
                tx.send(r.clone()).unwrap();
            }))
        });
        let session = AuthSession::new(AuthClient::new(http.clone()), tokens.clone(), Clock(5_000));

        // Only a refresh token: expired, so the engine refreshes first.
        assert_eq!(session.access_token().map(|t| t.expires_at), Some(0));
        block_on(session.refresh()).unwrap();
        assert_eq!(
            session.access_token(),
            Some(AccessToken {
                token: token("a2"),
                expires_at: 5_000 + 900_000 - EXPIRY_MARGIN_MS,
            })
        );
        assert_eq!(tokens.refresh_token(), Some(token("r2")));
        let rx = seen.borrow();
        let rx = rx.as_ref().unwrap();
        // The token is reported before the bundle fetch, then with it.
        assert_eq!(
            rx.try_recv().unwrap(),
            Refreshed {
                refresh_token: token("r2"),
                key_bundle: None,
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            Refreshed {
                refresh_token: token("r2"),
                key_bundle: Some(vec![1, 2, 3]),
            }
        );
    }

    #[test]
    fn ended_session_maps_to_session_ended() {
        let http = Canned::default();
        http.reply(401, r#"{"error":"session_ended"}"#);
        let tokens = SessionTokens::new(Some(token("r1")), None);
        let session = AuthSession::new(AuthClient::new(http.clone()), tokens.clone(), Clock(0));
        assert_eq!(block_on(session.refresh()), Err(RefreshError::SessionEnded));
        assert_eq!(tokens.refresh_token(), None);
        assert_eq!(session.access_token(), None);
        // With no refresh token left, nothing more is sent.
        assert_eq!(block_on(session.refresh()), Err(RefreshError::SessionEnded));
        assert_eq!(http.sent.borrow().len(), 1);
    }

    #[test]
    fn transient_failures_keep_the_token() {
        let http = Canned::default();
        http.fail();
        http.reply(503, "");
        let tokens = SessionTokens::new(Some(token("r1")), None);
        let session = AuthSession::new(AuthClient::new(http.clone()), tokens.clone(), Clock(0));
        assert_eq!(block_on(session.refresh()), Err(RefreshError::Unavailable));
        assert_eq!(block_on(session.refresh()), Err(RefreshError::Unavailable));
        assert_eq!(tokens.refresh_token(), Some(token("r1")));
    }

    #[test]
    fn a_refresh_someone_else_made_is_reused() {
        let http = Canned::default();
        let client = AuthClient::new(http.clone());
        let tokens = SessionTokens::new(Some(token("r1")), None);
        let seen = tokens.state.lock().generation;
        tokens.install(&Session {
            session_id: "s".into(),
            access_token: token("a9"),
            access_expires_in: 900,
            refresh_token: token("r9"),
            username: "alice".into(),
            is_admin: false,
        });
        block_on(tokens.refresh(&client, seen, |_| 0)).unwrap();
        assert!(http.sent.borrow().is_empty());
    }

    #[test]
    fn authed_refreshes_once_on_an_invalid_token() {
        let http = Canned::default();
        http.reply(401, r#"{"error":"invalid_token"}"#);
        http.reply(200, &tokens_json("a2", "r2"));
        http.reply(204, "");
        let client = AuthClient::new(http.clone());
        let tokens = SessionTokens::new(None, None);
        tokens.install(&Session {
            session_id: "s".into(),
            access_token: token("a1"),
            access_expires_in: 900,
            refresh_token: token("r1"),
            username: "alice".into(),
            is_admin: false,
        });
        block_on(tokens.authed(&client, |t| {
            let client = &client;
            async move { client.logout(&t).await }
        }))
        .unwrap();
        let sent = http.sent.borrow();
        let bearers: Vec<_> = sent.iter().map(|r| r.bearer.clone()).collect();
        assert_eq!(bearers, [Some(token("a1")), None, Some(token("a2"))]);
    }
}
