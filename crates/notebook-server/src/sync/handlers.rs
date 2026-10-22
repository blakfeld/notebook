//! The `/sync/v1` HTTP handlers. Each authenticates first, so a request
//! without a valid token reads and writes nothing.

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use notebook_core::note::NoteId;
use notebook_core::protocol::{ChangesResponse, PushRequest, PushResponse};
use serde::Deserialize;

use super::{SyncError, store};
use crate::app::AppState;
use crate::auth::AuthSession;

/// A sync error, or a body over the request limit, which keeps the plain
/// `413` the body-limit layer gives every route.
pub enum Rejection {
    Sync(SyncError),
    TooLarge,
}

impl From<SyncError> for Rejection {
    fn from(e: SyncError) -> Self {
        Self::Sync(e)
    }
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        match self {
            Self::Sync(e) => e.into_response(),
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        }
    }
}

/// Body-limit rejections stay `413`; anything else malformed is
/// `400 bad_request`.
fn rejected(status: StatusCode) -> Rejection {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        Rejection::TooLarge
    } else {
        Rejection::Sync(SyncError::BAD_REQUEST)
    }
}

fn parse_note(text: &str) -> Result<NoteId, SyncError> {
    text.parse().map_err(|_| SyncError::BAD_REQUEST)
}

pub async fn push(
    State(state): State<AppState>,
    session: AuthSession,
    body: Result<Json<PushRequest>, JsonRejection>,
) -> Result<Json<PushResponse>, Rejection> {
    let Json(request) = body.map_err(|e| rejected(e.status()))?;
    let pushed = state
        .sync
        .push(
            &state.pool,
            &session.vault_id,
            &request.updates,
            state.config.sync_max_update_bytes.get(),
        )
        .await?;
    Ok(Json(PushResponse {
        seqs: pushed.seqs,
        head: pushed.head,
    }))
}

#[derive(Deserialize)]
pub struct After {
    after: u64,
}

pub async fn changes(
    State(state): State<AppState>,
    session: AuthSession,
    query: Result<Query<After>, QueryRejection>,
) -> Result<Json<ChangesResponse>, SyncError> {
    let Query(After { after }) = query.map_err(|_| SyncError::BAD_REQUEST)?;
    Ok(Json(
        store::changes(&state.pool, &session.vault_id, after).await?,
    ))
}

#[derive(Deserialize)]
pub struct Covers {
    covers: u64,
}

pub async fn snapshot(
    State(state): State<AppState>,
    session: AuthSession,
    Path(note): Path<String>,
    query: Result<Query<Covers>, QueryRejection>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, Rejection> {
    let note = parse_note(&note)?;
    let Query(Covers { covers }) = query.map_err(|_| SyncError::BAD_REQUEST)?;
    let body = body.map_err(|e| rejected(e.status()))?;
    state
        .sync
        .snapshot(&state.pool, &session.vault_id, note, covers, &body)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn purge(
    State(state): State<AppState>,
    session: AuthSession,
    Path(note): Path<String>,
) -> Result<StatusCode, SyncError> {
    let note = parse_note(&note)?;
    state
        .sync
        .purge(&state.pool, &session.vault_id, note)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
