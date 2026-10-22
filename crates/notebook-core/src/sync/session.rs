//! Access tokens for sync requests, supplied by the host.

use std::future::Future;

use crate::auth::api::Token;

/// The current access token and when it expires, on the engine's
/// [`Timer`](super::Timer) clock (the host converts the server's
/// `access_expires_in` with `timer.now()` when it stores a token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessToken {
    pub token: Token,
    pub expires_at: u64,
}

/// Why a refresh failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshError {
    /// The session was revoked or expired: the user must sign in again.
    SessionEnded,
    /// The refresh couldn't complete (network, `429`, `5xx`); try again later.
    Unavailable,
}

/// Where the engine gets access tokens. The host owns the refresh token and
/// its storage; only the engine task calls [`refresh`](Self::refresh), so it
/// is never called concurrently.
pub trait SessionSource {
    /// The current access token, or `None` when signed out.
    fn access_token(&self) -> Option<AccessToken>;

    /// Exchanges the refresh token for a new access token, which
    /// [`access_token`](Self::access_token) returns afterwards.
    fn refresh(&self) -> impl Future<Output = Result<(), RefreshError>>;
}
