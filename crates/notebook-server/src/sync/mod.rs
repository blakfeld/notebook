//! The sync protocol's server side: the encrypted op log, paginated pull,
//! snapshots and compaction, purge, and the live channel. Wire types and the
//! endpoint table are in `notebook_core::protocol`.

pub mod error;
mod handlers;
pub mod hub;
mod live;
pub mod store;
#[cfg(test)]
mod tests;

use axum::Router;
use axum::routing::{delete, get, post, put};

use crate::app::AppState;

pub use error::SyncError;
pub use hub::{LiveSettings, SyncState};

/// The `/sync/v1` routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sync/v1/push", post(handlers::push))
        .route("/sync/v1/changes", get(handlers::changes))
        .route("/sync/v1/notes/{note_id}/snapshot", put(handlers::snapshot))
        .route("/sync/v1/notes/{note_id}", delete(handlers::purge))
        .route("/sync/v1/live", get(live::upgrade))
}
