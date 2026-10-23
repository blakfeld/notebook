//! The host side of the note editor bridge (`web/src/editor/bridge.ts`):
//! attach, push local updates, receive what the editor lacks, detach. Every
//! edit goes through the workspace and replica, so it is stored
//! like any other, and edits to trashed notes are refused. Changes made
//! outside the editor (a tag rename) reach it as `editor-remote-update`.
//!
//! Updates cross IPC as base64 (Yjs v1 updates); sessions as small numbers
//! that are never reused within a run.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use notebook_core::document::EditorUpdate;
use notebook_core::replica::EditorSessionId;
use notebook_core::workspace::Changed;
use serde::Serialize;

use crate::error::{AppError, AppResult, Code};
use crate::events::{self, Emitter};
use crate::state::{AppState, OpenVault, now};

#[derive(Default)]
pub struct Editors {
    sessions: BTreeMap<u64, EditorSessionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenedDto {
    pub session: u64,
    /// The note's full state, base64.
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteUpdateDto {
    pub session: u64,
    pub update: String,
}

fn decode(update: &str) -> AppResult<EditorUpdate> {
    STANDARD
        .decode(update)
        .map(EditorUpdate::from_bytes)
        .map_err(|_| AppError::internal("malformed editor update"))
}

fn session_of(u: &OpenVault, session: u64) -> AppResult<EditorSessionId> {
    u.editors
        .sessions
        .get(&session)
        .copied()
        .ok_or_else(|| AppError::new(Code::NotFound, "no such editor session"))
}

impl AppState {
    pub fn editor_open(&self, note: &str) -> AppResult<OpenedDto> {
        let note = crate::dto::parse_note_id(note)?;
        let session = self.next_editor_id();
        self.with_vault(|u| {
            let (id, state) = u.workspace.editor_attach(note)?;
            u.editors.sessions.insert(session, id);
            Ok(OpenedDto {
                session,
                state: STANDARD.encode(state.as_bytes()),
            })
        })
    }

    pub fn editor_push(&self, session: u64, update: &str) -> AppResult<()> {
        let update = decode(update)?;
        self.with_vault(|u| {
            let id = session_of(u, session)?;
            Ok(u.workspace.apply_editor_update(id, &update, now())?)
        })
    }

    pub fn editor_close(&self, session: u64) -> AppResult<()> {
        self.with_vault(|u| {
            if let Some(id) = u.editors.sessions.remove(&session) {
                u.workspace.editor_detach(id);
            }
            Ok(())
        })
    }
}

/// Sends every editor attached to a changed note what it lacks, and forgets
/// editors whose note was deleted (the workspace already dropped them).
pub(crate) fn forward_changes(emitter: &dyn Emitter, u: &mut OpenVault, changed: &Changed) {
    let workspace = &u.workspace;
    u.editors
        .sessions
        .retain(|_, id| workspace.editor_note(*id).is_some());
    let attached = u.workspace.editors_of(&changed.notes);
    let sessions: Vec<(u64, EditorSessionId)> = u
        .editors
        .sessions
        .iter()
        .filter(|(_, id)| attached.contains(id))
        .map(|(n, id)| (*n, *id))
        .collect();
    for (session, id) in sessions {
        match u.workspace.editor_pending(id) {
            Ok(Some(update)) => events::emit(
                emitter,
                events::EDITOR_REMOTE_UPDATE,
                &RemoteUpdateDto {
                    session,
                    update: STANDARD.encode(update.as_bytes()),
                },
            ),
            Ok(None) => {}
            // The note was deleted; its editor gets nothing more.
            Err(_) => {
                u.editors.sessions.remove(&session);
            }
        }
    }
}
