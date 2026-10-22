//! Accounts over OPAQUE: the `/auth` routes, device sessions, the OPAQUE
//! server key, and the authenticated-request extractor ([`AuthSession`]).
//!
//! The client side of every flow is in `notebook_core::auth`; this module
//! runs `opaque-ke`'s server side with the same [`Suite`].

pub mod admission;
mod error;
mod handlers;
pub mod keyfile;
pub mod pending;
pub mod sessions;
pub mod store;
#[cfg(test)]
mod tests;

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post};
use notebook_core::auth::Suite;
use opaque_ke::ServerLogin;
use sqlx::SqlitePool;

use crate::app::AppState;
use crate::rate_limit::{self, KeyedLimiter, NameLimiter};

pub use admission::RegistrationPolicy;
pub use error::{ApiError, ApiJson};
pub use keyfile::Setup;
pub use sessions::{AuthSession, authenticate_token, is_active};

/// Longest accepted device name, in characters.
pub const MAX_DEVICE_NAME_CHARS: usize = 64;

/// Auth state shared by the `/auth` handlers.
pub struct AuthState {
    pub setup: Setup,
    pub pending: Arc<pending::PendingMap<Pending>>,
    pub policy: RegistrationPolicy,
    /// Per normalized username, for login and passphrase change.
    pub username_limiter: Arc<NameLimiter>,
    pub login_limiter: Arc<KeyedLimiter>,
    pub recover_limiter: Arc<KeyedLimiter>,
    pub register_limiter: Arc<KeyedLimiter>,
    clock: Clock,
}

/// Server state held between a handshake's start and finish.
pub enum Pending {
    /// `user` is `None` for an unknown username; finish always fails.
    Login {
        state: Box<ServerLogin<Suite>>,
        user: Option<store::User>,
    },
    /// `opaque_record` is the record the login half was checked against.
    Passphrase {
        state: Box<ServerLogin<Suite>>,
        user_id: Vec<u8>,
        session_id: Vec<u8>,
        opaque_record: Vec<u8>,
    },
    /// The user whose recovery verifier matched.
    Recover { user: store::User },
}

fn nonzero(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("non-zero")
}

impl AuthState {
    pub fn new(setup: Setup, policy: RegistrationPolicy) -> Self {
        Self {
            setup,
            pending: Arc::new(pending::PendingMap::new()),
            policy,
            username_limiter: Arc::new(NameLimiter::per_period(
                Duration::from_secs(60),
                nonzero(10),
            )),
            login_limiter: Arc::new(KeyedLimiter::per_period(
                Duration::from_secs(6),
                nonzero(10),
            )),
            recover_limiter: Arc::new(KeyedLimiter::per_period(
                Duration::from_secs(600),
                nonzero(5),
            )),
            register_limiter: Arc::new(KeyedLimiter::per_period(
                Duration::from_secs(60),
                nonzero(5),
            )),
            clock: Clock::default(),
        }
    }

    /// Unix seconds, from the system clock outside tests.
    pub fn now(&self) -> i64 {
        self.clock.now()
    }

    /// Starts the background upkeep: the pending-handshake sweep, limiter
    /// retention, and hourly expired-session cleanup.
    pub fn spawn_tasks(&self, pool: &SqlitePool) -> Vec<tokio::task::JoinHandle<()>> {
        vec![
            pending::spawn_sweep_task(&self.pending),
            rate_limit::spawn_retain_task(&self.username_limiter),
            rate_limit::spawn_retain_task(&self.login_limiter),
            rate_limit::spawn_retain_task(&self.recover_limiter),
            rate_limit::spawn_retain_task(&self.register_limiter),
            sessions::spawn_cleanup_task(pool.clone()),
        ]
    }
}

/// The `/auth` routes, with the per-address limits on the start steps.
pub fn routes(state: &AppState) -> Router<AppState> {
    let limit =
        |limiter: &Arc<KeyedLimiter>| from_fn_with_state(limiter.clone(), rate_limit::middleware);
    let auth = &state.auth;
    Router::new()
        .route(
            "/auth/register/start",
            post(handlers::register_start).route_layer(limit(&auth.register_limiter)),
        )
        .route("/auth/register/finish", post(handlers::register_finish))
        .route(
            "/auth/login/start",
            post(handlers::login_start).route_layer(limit(&auth.login_limiter)),
        )
        .route("/auth/login/finish", post(handlers::login_finish))
        .route("/auth/refresh", post(handlers::refresh))
        .route("/auth/key-bundle", get(handlers::key_bundle))
        .route("/auth/logout", post(handlers::logout))
        .route("/auth/sessions", get(handlers::list_sessions))
        .route("/auth/sessions/{id}", delete(handlers::revoke_session))
        .route("/auth/passphrase/start", post(handlers::passphrase_start))
        .route("/auth/passphrase/finish", post(handlers::passphrase_finish))
        .route(
            "/auth/recover/start",
            post(handlers::recover_start).route_layer(limit(&auth.recover_limiter)),
        )
        .route("/auth/recover/finish", post(handlers::recover_finish))
}

/// Current Unix time in seconds.
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Byte equality whose timing doesn't depend on where the inputs differ.
pub(crate) fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The system clock, or a settable one in tests.
#[derive(Clone, Default)]
struct Clock(Option<Arc<AtomicI64>>);

impl Clock {
    fn now(&self) -> i64 {
        match &self.0 {
            Some(t) => t.load(Ordering::SeqCst),
            None => unix_now(),
        }
    }
}

#[cfg(test)]
impl AuthState {
    /// Switches to a settable clock starting at `now`, returning its handle.
    pub(crate) fn fix_clock(&mut self, now: i64) -> Arc<AtomicI64> {
        let t = Arc::new(AtomicI64::new(now));
        self.clock = Clock(Some(t.clone()));
        t
    }
}
