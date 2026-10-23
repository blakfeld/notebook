//! Command errors with stable codes the frontend maps to messages.
//!
//! Messages are for logs and developer tools; the UI shows its own text per
//! code. Neither ever contains a secret.

use std::fmt;

use notebook_core::auth::AuthError;
use notebook_core::auth::api::ErrorCode;
use notebook_core::auth::http::AuthClientError;
use notebook_core::storage::StorageError;
use notebook_core::sync::{ReplicaError, SyncError};
use notebook_core::workspace::{ServerUrlError, WorkspaceError};
use serde::Serialize;

use crate::credentials::CredentialError;

/// The stable error codes. `bad_credentials` is the only code for both an
/// unknown user and a wrong passphrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    InvalidServerUrl,
    InvalidUsername,
    PassphraseTooShort,
    PassphraseMismatch,
    BadCredentials,
    Unreachable,
    RateLimited,
    InvalidInvite,
    UsernameTaken,
    RecoveryKeyRequired,
    RecoveryKeyMalformed,
    RecoveryKeyWrong,
    WrongPassphrase,
    /// The session ended; re-enter the passphrase to sign in again.
    SignInRequired,
    Offline,
    PendingChanges,
    InvalidTagName,
    Trashed,
    NotTrashed,
    Locked,
    NotFound,
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

    pub fn locked() -> Self {
        Self::new(Code::Locked, "the vault is locked")
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

impl From<ServerUrlError> for AppError {
    fn from(err: ServerUrlError) -> Self {
        Self::new(Code::InvalidServerUrl, err.to_string())
    }
}

impl From<AuthClientError> for AppError {
    fn from(err: AuthClientError) -> Self {
        let message = err.to_string();
        let code = match err {
            AuthClientError::Network | AuthClientError::Server(_) => Code::Unreachable,
            AuthClientError::RateLimited { .. } => Code::RateLimited,
            AuthClientError::Rejected(code) => match code {
                ErrorCode::InvalidCredentials => Code::BadCredentials,
                ErrorCode::InvalidUsername => Code::InvalidUsername,
                ErrorCode::UsernameTaken => Code::UsernameTaken,
                ErrorCode::InvalidInvite => Code::InvalidInvite,
                ErrorCode::Unavailable => Code::RateLimited,
                ErrorCode::InvalidToken | ErrorCode::SessionEnded => Code::SignInRequired,
                ErrorCode::InvalidRequest | ErrorCode::NotFound | ErrorCode::Internal => {
                    Code::Internal
                }
            },
            AuthClientError::Auth(err) => match err {
                AuthError::InvalidCredentials => Code::BadCredentials,
                AuthError::InvalidUsername => Code::InvalidUsername,
                AuthError::PassphraseTooShort => Code::PassphraseTooShort,
                AuthError::Protocol => Code::Unreachable,
                AuthError::Crypto(_) => Code::Internal,
            },
        };
        Self::new(code, message)
    }
}

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
            WorkspaceError::Sync(SyncError::Offline | SyncError::SignInRequired) => Code::Offline,
            WorkspaceError::Sync(SyncError::RateLimited) => Code::RateLimited,
            WorkspaceError::Replica(ReplicaError::EditorSessionNotFound(_)) => Code::NotFound,
            WorkspaceError::Replica(_) | WorkspaceError::Sync(_) => Code::Internal,
        };
        Self::new(code, message)
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

    #[test]
    fn serializes_with_a_snake_case_code() {
        let json = serde_json::to_value(AppError::new(Code::BadCredentials, "x")).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"code": "bad_credentials", "message": "x"})
        );
    }

    #[test]
    fn unknown_user_and_wrong_passphrase_share_a_code() {
        let a: AppError = AuthClientError::Auth(AuthError::InvalidCredentials).into();
        let b: AppError = AuthClientError::Rejected(ErrorCode::InvalidCredentials).into();
        assert_eq!(a.code, Code::BadCredentials);
        assert_eq!(b.code, Code::BadCredentials);
    }

    #[test]
    fn network_and_rate_limits_are_distinct() {
        assert_eq!(
            AppError::from(AuthClientError::Network).code,
            Code::Unreachable
        );
        assert_eq!(
            AppError::from(AuthClientError::RateLimited { retry_after: None }).code,
            Code::RateLimited
        );
    }
}
