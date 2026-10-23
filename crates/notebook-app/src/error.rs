//! Command errors with stable codes the frontend maps to messages.
//!
//! Messages are for logs and developer tools; the UI shows its own text per
//! code. Neither ever contains a secret.

use std::fmt;

use notebook_core::export::ExportError;
use notebook_core::storage::StorageError;
use notebook_core::sync::ReplicaError;
use notebook_core::workspace::WorkspaceError;
use serde::Serialize;

use crate::credentials::CredentialError;

/// The stable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The recovery key fails its checksum: probably mistyped.
    RecoveryKeyMalformed,
    /// A well-formed recovery key that doesn't open this notebook.
    RecoveryKeyWrong,
    /// The old passphrase of an upgrading install is wrong.
    WrongPassphrase,
    InvalidTagName,
    Trashed,
    NotTrashed,
    /// The notebook isn't open, or the command doesn't fit the current phase.
    NotOpen,
    NotFound,
    /// Writing an export failed. The message says why and is shown.
    ExportFailed,
    /// Writing a backup failed. The message says why and is shown.
    BackupFailed,
    /// No backup folder has been chosen.
    BackupNoFolder,
    /// The chosen file isn't a Notebook backup.
    BackupNotABackup,
    /// The backup needs a newer version of Notebook.
    BackupTooNew,
    /// The backup is truncated or altered.
    BackupDamaged,
    /// The backup is of another notebook, so its recovery key is needed.
    RecoveryKeyRequired,
    /// The backup of the current notes before a restore failed; nothing
    /// changed. The message says why and is shown.
    SafetyBackupFailed,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppError {
    pub code: Code,
    pub message: String,
}

impl AppError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn not_open() -> Self {
        Self::new(Code::NotOpen, "the notebook is not open")
    }

    pub fn internal(message: impl fmt::Display) -> Self {
        Self::new(Code::Internal, message.to_string())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

pub type AppResult<T> = Result<T, AppError>;

impl From<WorkspaceError> for AppError {
    fn from(err: WorkspaceError) -> Self {
        let message = err.to_string();
        let code = match err {
            WorkspaceError::NotFound(_) => Code::NotFound,
            WorkspaceError::Trashed(_) => Code::Trashed,
            WorkspaceError::NotTrashed(_) => Code::NotTrashed,
            WorkspaceError::InvalidTagName(_) => Code::InvalidTagName,
            // A rename that can't be written is still a name the user has
            // to change.
            WorkspaceError::Unrepresentable(_) => Code::InvalidTagName,
            WorkspaceError::Replica(ReplicaError::EditorSessionNotFound(_)) => Code::NotFound,
            WorkspaceError::Replica(_) | WorkspaceError::Sync(_) => Code::Internal,
        };
        Self::new(code, message)
    }
}

impl From<ExportError> for AppError {
    fn from(err: ExportError) -> Self {
        Self::new(Code::ExportFailed, err.to_string())
    }
}

impl From<CredentialError> for AppError {
    fn from(err: CredentialError) -> Self {
        Self::internal(err)
    }
}

impl From<StorageError> for AppError {
    fn from(err: StorageError) -> Self {
        Self::internal(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notebook_core::note::NoteId;

    #[test]
    fn serializes_with_a_snake_case_code() {
        let json = serde_json::to_value(AppError::new(Code::RecoveryKeyWrong, "x")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"code": "recovery_key_wrong", "message": "x"})
        );
    }

    #[test]
    fn workspace_errors_map_to_codes() {
        let id = NoteId::from_parts(1, 1);
        assert_eq!(
            AppError::from(WorkspaceError::NotTrashed(id)).code,
            Code::NotTrashed
        );
        assert_eq!(
            AppError::from(WorkspaceError::Unrepresentable(id)).code,
            Code::InvalidTagName
        );
    }
}
