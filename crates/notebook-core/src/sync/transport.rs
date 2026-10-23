//! The network boundary: the `/sync/v1` endpoints and the live channel,
//! spoken in [`crate::protocol`] types. Native and wasm hosts each have one
//! implementation; tests use the in-memory fake.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use crate::auth::api::Token;
use crate::note::NoteId;
use crate::protocol::{
    ChangesResponse, ClientFrame, Entry, ErrorBody, ErrorCode, OpId, PushOutcome, PushRequest,
    PushResponse, ServerFrame,
};
use crate::storage::UpdateId;

impl From<OpId> for UpdateId {
    fn from(id: OpId) -> Self {
        Self(id.to_bytes())
    }
}

impl From<UpdateId> for OpId {
    fn from(id: UpdateId) -> Self {
        Self::from_bytes(id.0)
    }
}

/// Why a sync request failed. Every `sync-protocol` error code has a
/// variant, so the engine handles a rejection the same way whether it came
/// as an HTTP status or a live `push_result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// No response: connection refused or dropped, timeout, DNS. The message
    /// is for logs.
    Network(String),
    /// `401 invalid_token`: refresh the session and retry.
    InvalidToken,
    /// `429`: wait at least `retry_after` (when the server said) before the
    /// next request.
    RateLimited { retry_after: Option<Duration> },
    /// `400 bad_request`.
    BadRequest,
    /// `404 unknown_note`.
    UnknownNote,
    /// `409 op_id_conflict`.
    OpIdConflict,
    /// `409 note_key_conflict`.
    NoteKeyConflict,
    /// `409 snapshot_stale`.
    SnapshotStale,
    /// `409 cursor_ahead`: the server was restored from a backup.
    CursorAhead,
    /// `410 note_purged`.
    NotePurged,
    /// `413`: an update is over the server's limit, or the request body is.
    UpdateTooLarge,
    /// `422 missing_note_key`.
    MissingNoteKey,
    /// `507`: the vault's storage quota is full.
    QuotaExceeded,
    /// Any other status, including `500 internal` and unreadable bodies.
    Server(u16),
}

impl TransportError {
    /// Maps an HTTP error response. `retry_after` is the parsed
    /// `Retry-After` header, if any.
    pub fn from_response(status: u16, body: &[u8], retry_after: Option<Duration>) -> Self {
        match status {
            401 => Self::InvalidToken,
            // The body limit's 413 isn't a sync error body; both mean the
            // request was too big.
            413 => Self::UpdateTooLarge,
            429 => Self::RateLimited { retry_after },
            507 => Self::QuotaExceeded,
            _ => match serde_json::from_slice::<ErrorBody>(body) {
                Ok(ErrorBody { error }) => Self::from_code(error, status),
                Err(_) => Self::Server(status),
            },
        }
    }

    /// Maps an error code from a body or a rejected live push. `status` is
    /// used for codes without a variant of their own.
    pub fn from_code(code: ErrorCode, status: u16) -> Self {
        match code {
            ErrorCode::BadRequest => Self::BadRequest,
            ErrorCode::InvalidToken => Self::InvalidToken,
            ErrorCode::UnknownNote => Self::UnknownNote,
            ErrorCode::OpIdConflict => Self::OpIdConflict,
            ErrorCode::NoteKeyConflict => Self::NoteKeyConflict,
            ErrorCode::SnapshotStale => Self::SnapshotStale,
            ErrorCode::CursorAhead => Self::CursorAhead,
            ErrorCode::NotePurged => Self::NotePurged,
            ErrorCode::UpdateTooLarge => Self::UpdateTooLarge,
            ErrorCode::MissingNoteKey => Self::MissingNoteKey,
            ErrorCode::RateLimited => Self::RateLimited { retry_after: None },
            ErrorCode::QuotaExceeded => Self::QuotaExceeded,
            ErrorCode::Internal => Self::Server(status.max(500)),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(msg) => write!(f, "network error: {msg}"),
            Self::InvalidToken => f.write_str("access token rejected"),
            Self::RateLimited { .. } => f.write_str("rate limited"),
            Self::BadRequest => f.write_str("bad request"),
            Self::UnknownNote => f.write_str("unknown note"),
            Self::OpIdConflict => f.write_str("update ID conflict"),
            Self::NoteKeyConflict => f.write_str("note key conflict"),
            Self::SnapshotStale => f.write_str("snapshot is stale"),
            Self::CursorAhead => f.write_str("cursor is ahead of the server"),
            Self::NotePurged => f.write_str("note was purged"),
            Self::UpdateTooLarge => f.write_str("update too large"),
            Self::MissingNoteKey => f.write_str("missing note key"),
            Self::QuotaExceeded => f.write_str("storage quota exceeded"),
            Self::Server(status) => write!(f, "server error {status}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// The `/sync/v1` HTTP endpoints, plus opening the live channel.
///
/// Methods take `&self` and return futures without `Send` bounds: the
/// engine is one task, on one thread.
pub trait Transport {
    type Live: LiveChannel;

    /// `POST push`.
    fn push(
        &self,
        token: &Token,
        request: &PushRequest,
    ) -> impl Future<Output = Result<PushResponse, TransportError>>;

    /// `GET changes?after=`.
    fn changes(
        &self,
        token: &Token,
        after: u64,
    ) -> impl Future<Output = Result<ChangesResponse, TransportError>>;

    /// `PUT notes/{note}/snapshot?covers=` with the raw ciphertext.
    fn upload_snapshot(
        &self,
        token: &Token,
        note: NoteId,
        covers: u64,
        data: &[u8],
    ) -> impl Future<Output = Result<(), TransportError>>;

    /// `DELETE notes/{note}`.
    fn purge(
        &self,
        token: &Token,
        note: NoteId,
    ) -> impl Future<Output = Result<(), TransportError>>;

    /// Opens `GET live`. The caller sends the `auth` frame.
    fn connect_live(&self) -> impl Future<Output = Result<Self::Live, TransportError>>;
}

/// What the live channel delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    /// Authenticated and subscribed at `head`.
    Ready {
        head: u64,
    },
    Entry(Entry),
    /// The result of the `push` frame with `request_id`.
    PushResult {
        request_id: String,
        result: Result<PushResponse, TransportError>,
    },
    /// The server closed the socket with this code (see
    /// [`CLOSE_AUTH`](crate::protocol::CLOSE_AUTH) and friends), or `None`
    /// without one.
    Closed(Option<u16>),
    /// The socket failed. It is unusable afterwards.
    Error(TransportError),
}

// Helpers shared by the native and wasm transports.

/// The sync API's HTTP base and live channel URL for a server's public URL
/// (`https://host[:port]`; a path is ignored).
#[cfg_attr(
    not(any(feature = "transport-native", feature = "transport-wasm")),
    allow(dead_code)
)]
pub(crate) fn urls(server: &str) -> Result<(String, String), TransportError> {
    let invalid = || TransportError::Network(format!("invalid server URL {server:?}"));
    let (scheme, rest) = server.split_once("://").ok_or_else(invalid)?;
    let ws_scheme = match scheme {
        "https" => "wss",
        "http" => "ws",
        _ => return Err(invalid()),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err(invalid());
    }
    // Tokens travel in every request, so cleartext is only for a server on
    // this machine.
    if scheme == "http" && !is_loopback(authority) {
        return Err(TransportError::Network(format!(
            "server URL {server:?} must use https"
        )));
    }
    Ok((
        format!("{scheme}://{authority}/sync/v1"),
        format!("{ws_scheme}://{authority}/sync/v1/live"),
    ))
}

/// Whether `authority` (`host[:port]`) names this machine.
pub(crate) fn is_loopback(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `Retry-After` in seconds (the server never sends an HTTP date).
#[cfg_attr(
    not(any(feature = "transport-native", feature = "transport-wasm")),
    allow(dead_code)
)]
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse().ok().map(Duration::from_secs)
}

/// Decodes a `2xx` JSON body; an unreadable one counts as a server error.
#[cfg_attr(
    not(any(feature = "transport-native", feature = "transport-wasm")),
    allow(dead_code)
)]
pub(crate) fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, TransportError> {
    serde_json::from_slice(body).map_err(|_| TransportError::Server(200))
}

/// Maps a server frame to a live event.
#[cfg_attr(
    not(any(feature = "transport-native", feature = "transport-wasm")),
    allow(dead_code)
)]
pub(crate) fn live_event(frame: ServerFrame) -> LiveEvent {
    match frame {
        ServerFrame::Ready { head } => LiveEvent::Ready { head },
        ServerFrame::Entry(entry) => LiveEvent::Entry(entry),
        ServerFrame::PushResult {
            request_id,
            outcome,
        } => LiveEvent::PushResult {
            request_id,
            result: match outcome {
                PushOutcome::Accepted(response) => Ok(response),
                PushOutcome::Rejected(body) => Err(TransportError::from_code(body.error, 500)),
            },
        },
    }
}

/// An open live channel.
pub trait LiveChannel {
    /// The next event. Must be cancel-safe: the engine drops this future
    /// whenever another event wins, and nothing may be lost when it does.
    /// After [`LiveEvent::Closed`] or [`LiveEvent::Error`] it keeps
    /// returning one of those.
    fn next(&mut self) -> impl Future<Output = LiveEvent>;

    /// Sends an `auth` or `push` frame.
    fn send(&mut self, frame: &ClientFrame) -> impl Future<Output = Result<(), TransportError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_id_round_trips_through_update_id() {
        let op = OpId::from_parts(1_700_000_000_000, 0x1234_5678_9abc_def0_1234);
        let update = UpdateId::from(op);
        assert_eq!(update.0, op.to_bytes());
        assert_eq!(OpId::from(update), op);
    }

    #[test]
    fn maps_statuses_and_codes() {
        let body = |code: &str| format!(r#"{{"error":"{code}"}}"#).into_bytes();
        let cases = [
            (401, body("invalid_token"), TransportError::InvalidToken),
            (400, body("bad_request"), TransportError::BadRequest),
            (404, body("unknown_note"), TransportError::UnknownNote),
            (409, body("op_id_conflict"), TransportError::OpIdConflict),
            (
                409,
                body("note_key_conflict"),
                TransportError::NoteKeyConflict,
            ),
            (409, body("snapshot_stale"), TransportError::SnapshotStale),
            (409, body("cursor_ahead"), TransportError::CursorAhead),
            (410, body("note_purged"), TransportError::NotePurged),
            (
                413,
                body("update_too_large"),
                TransportError::UpdateTooLarge,
            ),
            (
                413,
                b"length limit exceeded".to_vec(),
                TransportError::UpdateTooLarge,
            ),
            (
                422,
                body("missing_note_key"),
                TransportError::MissingNoteKey,
            ),
            (507, body("quota_exceeded"), TransportError::QuotaExceeded),
            (507, Vec::new(), TransportError::QuotaExceeded),
            (500, body("internal"), TransportError::Server(500)),
            (502, b"<html>".to_vec(), TransportError::Server(502)),
        ];
        for (status, body, want) in cases {
            assert_eq!(TransportError::from_response(status, &body, None), want);
        }
        assert_eq!(
            TransportError::from_response(429, b"", Some(Duration::from_secs(30))),
            TransportError::RateLimited {
                retry_after: Some(Duration::from_secs(30))
            }
        );
    }
}
