//! `/auth` handler errors: a status and `{"error": "<code>"}`, never internal
//! details.

use axum::Json;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use notebook_core::auth::api::{ErrorBody, ErrorCode};
use serde::de::DeserializeOwned;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiError(pub ErrorCode);

impl ApiError {
    pub const INVALID_REQUEST: Self = Self(ErrorCode::InvalidRequest);
    pub const INVALID_USERNAME: Self = Self(ErrorCode::InvalidUsername);
    pub const INVALID_CREDENTIALS: Self = Self(ErrorCode::InvalidCredentials);
    pub const INVALID_TOKEN: Self = Self(ErrorCode::InvalidToken);
    pub const SESSION_ENDED: Self = Self(ErrorCode::SessionEnded);
    pub const USERNAME_TAKEN: Self = Self(ErrorCode::UsernameTaken);
    pub const INVALID_INVITE: Self = Self(ErrorCode::InvalidInvite);
    pub const NOT_FOUND: Self = Self(ErrorCode::NotFound);
    pub const UNAVAILABLE: Self = Self(ErrorCode::Unavailable);
    pub const INTERNAL: Self = Self(ErrorCode::Internal);

    pub fn status(self) -> StatusCode {
        match self.0 {
            ErrorCode::InvalidRequest | ErrorCode::InvalidUsername => StatusCode::BAD_REQUEST,
            ErrorCode::InvalidCredentials | ErrorCode::InvalidToken | ErrorCode::SessionEnded => {
                StatusCode::UNAUTHORIZED
            }
            ErrorCode::UsernameTaken => StatusCode::CONFLICT,
            ErrorCode::InvalidInvite => StatusCode::FORBIDDEN,
            ErrorCode::NotFound => StatusCode::NOT_FOUND,
            ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status(), Json(ErrorBody { error: self.0 })).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("auth database error: {e}");
        Self::INTERNAL
    }
}

/// `Json<T>` whose rejections (bad content type, syntax, missing fields,
/// malformed base64) are `400 invalid_request` like every other auth error.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(_) => Err(ApiError::INVALID_REQUEST),
        }
    }
}
