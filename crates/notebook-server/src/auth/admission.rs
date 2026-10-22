//! Who may register over HTTP. Closed until invite codes gate registration;
//! accounts are created with `notebook-server user add` meanwhile.

use notebook_core::auth::api::RegisterFinishRequest;
use sqlx::SqliteConnection;

use super::error::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationPolicy {
    /// Both registration steps answer `403 registration_closed` before
    /// looking at the request, so nothing about usernames is revealed.
    Closed,
    /// Anyone may register. Test builds only: there is deliberately no
    /// config key that opens public sign-up.
    #[cfg(test)]
    Open,
}

impl RegistrationPolicy {
    /// Checked first by both registration steps.
    pub fn check(self) -> Result<(), ApiError> {
        match self {
            Self::Closed => Err(ApiError::REGISTRATION_CLOSED),
            #[cfg(test)]
            Self::Open => Ok(()),
        }
    }

    /// Runs inside the registration finish transaction (opened with
    /// `BEGIN IMMEDIATE`) before the account is created, so an admission
    /// check (such as consuming an invite) commits or rolls back with it.
    pub async fn admit(
        self,
        _tx: &mut SqliteConnection,
        _request: &RegisterFinishRequest,
    ) -> Result<(), ApiError> {
        self.check()
    }
}
