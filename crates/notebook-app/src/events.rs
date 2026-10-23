//! Events emitted to the frontend, behind a trait so state logic can be
//! tested without a webview.

#[cfg(test)]
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter as _, Runtime};

pub const APP_PHASE_CHANGED: &str = "app-phase-changed";
pub const VAULT_CHANGED: &str = "vault-changed";
/// The backup status changed: a [`BackupStatusDto`](crate::dto::BackupStatusDto).
pub const BACKUP_STATUS_CHANGED: &str = "backup-status-changed";
/// Changes an open editor lacks: `{session, update}` (base64 Yjs update).
pub const EDITOR_REMOTE_UPDATE: &str = "editor-remote-update";

pub trait Emitter: Send + Sync {
    fn emit(&self, event: &'static str, payload: Value);
}

/// Serializes `payload` and emits it. Payload types always serialize.
pub fn emit(emitter: &dyn Emitter, event: &'static str, payload: &impl Serialize) {
    emitter.emit(
        event,
        serde_json::to_value(payload).expect("event payloads serialize"),
    );
}

/// Emits to every webview of the app.
pub struct TauriEmitter<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> Emitter for TauriEmitter<R> {
    fn emit(&self, event: &'static str, payload: Value) {
        if let Err(err) = self.0.emit(event, payload) {
            tracing::warn!(event, %err, "failed to emit event");
        }
    }
}

/// Keeps every event, for tests.
#[cfg(test)]
#[derive(Default)]
pub struct Recorder(Mutex<Vec<(&'static str, Value)>>);

#[cfg(test)]
impl Recorder {
    pub fn take(&self) -> Vec<(&'static str, Value)> {
        std::mem::take(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

#[cfg(test)]
impl Emitter for Recorder {
    fn emit(&self, event: &'static str, payload: Value) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((event, payload));
    }
}
