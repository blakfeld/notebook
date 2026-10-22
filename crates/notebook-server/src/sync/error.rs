//! Sync errors: a status and `{"error": "<code>"}`, never payloads or
//! internal details.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use notebook_core::protocol::{ErrorBody, ErrorCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncError(pub ErrorCode);

impl SyncError {
    pub const BAD_REQUEST: Self = Self(ErrorCode::BadRequest);
    pub const UNKNOWN_NOTE: Self = Self(ErrorCode::UnknownNote);
    pub const OP_ID_CONFLICT: Self = Self(ErrorCode::OpIdConflict);
    pub const NOTE_KEY_CONFLICT: Self = Self(ErrorCode::NoteKeyConflict);
    pub const SNAPSHOT_STALE: Self = Self(ErrorCode::SnapshotStale);
    pub const CURSOR_AHEAD: Self = Self(ErrorCode::CursorAhead);
    pub const NOTE_PURGED: Self = Self(ErrorCode::NotePurged);
    pub const UPDATE_TOO_LARGE: Self = Self(ErrorCode::UpdateTooLarge);
    pub const MISSING_NOTE_KEY: Self = Self(ErrorCode::MissingNoteKey);
    pub const RATE_LIMITED: Self = Self(ErrorCode::RateLimited);
    pub const QUOTA_EXCEEDED: Self = Self(ErrorCode::QuotaExceeded);
    pub const INTERNAL: Self = Self(ErrorCode::Internal);

    pub fn status(self) -> StatusCode {
        match self.0 {
            ErrorCode::BadRequest => StatusCode::BAD_REQUEST,
            ErrorCode::InvalidToken => StatusCode::UNAUTHORIZED,
            ErrorCode::UnknownNote => StatusCode::NOT_FOUND,
            ErrorCode::OpIdConflict
            | ErrorCode::NoteKeyConflict
            | ErrorCode::SnapshotStale
            | ErrorCode::CursorAhead => StatusCode::CONFLICT,
            ErrorCode::NotePurged => StatusCode::GONE,
            ErrorCode::UpdateTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::MissingNoteKey => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::QuotaExceeded => StatusCode::INSUFFICIENT_STORAGE,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for SyncError {
    fn into_response(self) -> Response {
        (self.status(), Json(ErrorBody { error: self.0 })).into_response()
    }
}

impl From<sqlx::Error> for SyncError {
    fn from(e: sqlx::Error) -> Self {
        // sqlx errors carry SQL and constraint names, never bound values.
        tracing::error!("sync database error: {e}");
        Self::INTERNAL
    }
}
