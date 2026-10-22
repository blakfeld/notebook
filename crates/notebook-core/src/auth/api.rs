//! Request and response bodies for the `/auth` routes, shared by client and
//! server. JSON; byte fields are base64url without padding; timestamps are
//! Unix seconds.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What kind of client a session belongs to. A hint that picks the refresh
/// lifetime, not a security boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Native,
    Browser,
}

/// An access or refresh token: 32 random bytes, base64url. `Debug` is
/// redacted so tokens can't reach logs by accident.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(pub String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// `Debug` omits the invite code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterStartRequest {
    pub username: String,
    #[serde(with = "b64")]
    pub registration_request: Vec<u8>,
    /// Sent as typed; the server normalizes it. A missing code reads as
    /// empty, so the server rejects it like any other invalid code.
    #[serde(default)]
    pub invite_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterStartResponse {
    #[serde(with = "b64")]
    pub registration_response: Vec<u8>,
}

/// `Debug` omits the recovery verifier, which is enough to start recovery,
/// and the invite code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterFinishRequest {
    pub username: String,
    /// As in [`RegisterStartRequest::invite_code`].
    #[serde(default)]
    pub invite_code: String,
    #[serde(with = "b64")]
    pub registration_upload: Vec<u8>,
    #[serde(with = "b64")]
    pub key_bundle: Vec<u8>,
    #[serde(with = "b64")]
    pub recovery_verifier: Vec<u8>,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub session_kind: SessionKind,
}

/// A new device session, returned by register finish, login finish
/// (flattened into [`LoginFinishResponse`]), and recover finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub access_token: Token,
    /// Seconds until the access token expires.
    pub access_expires_in: u64,
    pub refresh_token: Token,
    pub username: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginStartRequest {
    pub username: String,
    #[serde(with = "b64")]
    pub credential_request: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginStartResponse {
    pub handshake_id: String,
    #[serde(with = "b64")]
    pub credential_response: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginFinishRequest {
    pub handshake_id: String,
    #[serde(with = "b64")]
    pub credential_finalization: Vec<u8>,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub session_kind: SessionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginFinishResponse {
    #[serde(flatten)]
    pub session: Session,
    #[serde(with = "b64")]
    pub key_bundle: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: Token,
}

/// A rotated token pair from `/auth/refresh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: Token,
    pub access_expires_in: u64,
    pub refresh_token: Token,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBundleResponse {
    #[serde(with = "b64")]
    pub key_bundle: Vec<u8>,
}

/// One row of `GET /auth/sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub device_name: String,
    pub session_kind: SessionKind,
    pub created_at: i64,
    pub last_used_at: i64,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassphraseStartRequest {
    #[serde(with = "b64")]
    pub credential_request: Vec<u8>,
    #[serde(with = "b64")]
    pub registration_request: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassphraseStartResponse {
    pub handshake_id: String,
    #[serde(with = "b64")]
    pub credential_response: Vec<u8>,
    #[serde(with = "b64")]
    pub registration_response: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassphraseFinishRequest {
    pub handshake_id: String,
    #[serde(with = "b64")]
    pub credential_finalization: Vec<u8>,
    #[serde(with = "b64")]
    pub registration_upload: Vec<u8>,
    #[serde(with = "b64")]
    pub key_bundle: Vec<u8>,
}

/// `Debug` omits the recovery verifier, which is enough to start recovery.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverStartRequest {
    pub username: String,
    #[serde(with = "b64")]
    pub recovery_verifier: Vec<u8>,
    #[serde(with = "b64")]
    pub registration_request: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverStartResponse {
    pub handshake_id: String,
    #[serde(with = "b64")]
    pub key_bundle: Vec<u8>,
    #[serde(with = "b64")]
    pub registration_response: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverFinishRequest {
    pub handshake_id: String,
    #[serde(with = "b64")]
    pub registration_upload: Vec<u8>,
    #[serde(with = "b64")]
    pub key_bundle: Vec<u8>,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub session_kind: SessionKind,
}

impl fmt::Debug for RegisterStartRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterStartRequest")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for RegisterFinishRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterFinishRequest")
            .field("username", &self.username)
            .field("device_name", &self.device_name)
            .field("session_kind", &self.session_kind)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for RecoverStartRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoverStartRequest")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// The machine-readable reason in an `/auth` error body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    InvalidUsername,
    InvalidCredentials,
    InvalidToken,
    SessionEnded,
    UsernameTaken,
    /// The invite code is missing, unknown, expired, revoked, or used.
    InvalidInvite,
    /// No such session (or it belongs to someone else).
    NotFound,
    /// Too many handshakes in flight; retry later.
    Unavailable,
    /// A server-side failure; details are only in the server log.
    Internal,
}

/// Every `/auth` handler error: `{"error": "<code>"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub error: ErrorCode,
}

/// What a refresh attempt means for the stored refresh token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// New tokens; replace both stored tokens.
    Refreshed(Tokens),
    /// The session no longer exists; discard the tokens and log in again.
    SessionEnded,
    /// Anything else (no response, `429`, `5xx`, an unexpected body): keep
    /// the refresh token and retry later.
    Transient,
}

impl RefreshOutcome {
    /// Classifies a refresh response: `status` is `None` when no response
    /// arrived (network error or timeout).
    pub fn classify(status: Option<u16>, body: &[u8]) -> Self {
        match status {
            Some(200) => serde_json::from_slice(body)
                .map(Self::Refreshed)
                .unwrap_or(Self::Transient),
            Some(401) => match serde_json::from_slice::<ErrorBody>(body) {
                Ok(ErrorBody {
                    error: ErrorCode::SessionEnded,
                }) => Self::SessionEnded,
                _ => Self::Transient,
            },
            _ => Self::Transient,
        }
    }
}

/// Serde for byte fields as base64url without padding.
mod b64 {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        URL_SAFE_NO_PAD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T>(value: &T) -> String
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value);
        json
    }

    fn token(s: &str) -> Token {
        Token(s.to_owned())
    }

    fn session() -> Session {
        Session {
            session_id: "01J".into(),
            access_token: token("a"),
            access_expires_in: 900,
            refresh_token: token("r"),
            username: "alice".into(),
            is_admin: false,
        }
    }

    #[test]
    fn every_type_round_trips() {
        let bytes = vec![0xfb, 0xff, 0x00, 0x01];
        let json = round_trip(&RegisterStartRequest {
            username: "alice".into(),
            registration_request: bytes.clone(),
            invite_code: "7KQ2M-XW9PD-0RT4H-C6NBJ".into(),
        });
        // base64url, no padding.
        assert!(json.contains("\"-_8AAQ\""), "{json}");
        assert!(
            json.contains(r#""invite_code":"7KQ2M-XW9PD-0RT4H-C6NBJ""#),
            "{json}"
        );
        // A request without a code reads as an empty one.
        let start: RegisterStartRequest =
            serde_json::from_str(r#"{"username":"alice","registration_request":""}"#).unwrap();
        assert_eq!(start.invite_code, "");

        round_trip(&RegisterStartResponse {
            registration_response: bytes.clone(),
        });
        round_trip(&RegisterFinishRequest {
            username: "alice".into(),
            invite_code: "7KQ2M-XW9PD-0RT4H-C6NBJ".into(),
            registration_upload: bytes.clone(),
            key_bundle: bytes.clone(),
            recovery_verifier: bytes.clone(),
            device_name: "laptop".into(),
            session_kind: SessionKind::Browser,
        });
        round_trip(&session());
        round_trip(&LoginStartRequest {
            username: "alice".into(),
            credential_request: bytes.clone(),
        });
        round_trip(&LoginStartResponse {
            handshake_id: "h".into(),
            credential_response: bytes.clone(),
        });
        round_trip(&LoginFinishRequest {
            handshake_id: "h".into(),
            credential_finalization: bytes.clone(),
            device_name: String::new(),
            session_kind: SessionKind::Native,
        });
        let json = round_trip(&LoginFinishResponse {
            session: session(),
            key_bundle: bytes.clone(),
        });
        assert!(json.contains("\"session_id\""), "flattened: {json}");
        round_trip(&RefreshRequest {
            refresh_token: token("r"),
        });
        round_trip(&Tokens {
            access_token: token("a"),
            access_expires_in: 900,
            refresh_token: token("r"),
        });
        round_trip(&KeyBundleResponse {
            key_bundle: bytes.clone(),
        });
        round_trip(&SessionInfo {
            id: "s".into(),
            device_name: "phone".into(),
            session_kind: SessionKind::Browser,
            created_at: 1,
            last_used_at: 2,
            current: true,
        });
        round_trip(&PassphraseStartRequest {
            credential_request: bytes.clone(),
            registration_request: bytes.clone(),
        });
        round_trip(&PassphraseStartResponse {
            handshake_id: "h".into(),
            credential_response: bytes.clone(),
            registration_response: bytes.clone(),
        });
        round_trip(&PassphraseFinishRequest {
            handshake_id: "h".into(),
            credential_finalization: bytes.clone(),
            registration_upload: bytes.clone(),
            key_bundle: bytes.clone(),
        });
        round_trip(&RecoverStartRequest {
            username: "alice".into(),
            recovery_verifier: bytes.clone(),
            registration_request: bytes.clone(),
        });
        round_trip(&RecoverStartResponse {
            handshake_id: "h".into(),
            key_bundle: bytes.clone(),
            registration_response: bytes.clone(),
        });
        round_trip(&RecoverFinishRequest {
            handshake_id: "h".into(),
            registration_upload: bytes.clone(),
            key_bundle: bytes,
            device_name: "x".into(),
            session_kind: SessionKind::Native,
        });
    }

    #[test]
    fn device_name_and_kind_default() {
        let req: LoginFinishRequest =
            serde_json::from_str(r#"{"handshake_id":"h","credential_finalization":"AA"}"#).unwrap();
        assert_eq!(req.device_name, "");
        assert_eq!(req.session_kind, SessionKind::Native);
    }

    #[test]
    fn error_body_format() {
        let body = ErrorBody {
            error: ErrorCode::InvalidCredentials,
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"invalid_credentials"}"#
        );
        round_trip(&body);
        let body = ErrorBody {
            error: ErrorCode::InvalidInvite,
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"error":"invalid_invite"}"#
        );
    }

    #[test]
    fn unknown_error_code_and_bad_base64_fail_to_parse() {
        assert!(serde_json::from_str::<ErrorBody>(r#"{"error":"teapot"}"#).is_err());
        assert!(serde_json::from_str::<SessionKind>(r#""desktop""#).is_err());
        for bad in ["!!", "AA==", "A", "\u{e9}"] {
            let json = format!(r#"{{"key_bundle":"{bad}"}}"#);
            assert!(
                serde_json::from_str::<KeyBundleResponse>(&json).is_err(),
                "{bad}"
            );
        }
        assert!(serde_json::from_str::<KeyBundleResponse>(r#"{"key_bundle":7}"#).is_err());
    }

    #[test]
    fn token_debug_is_redacted() {
        let tokens = Tokens {
            access_token: token("secret-access"),
            access_expires_in: 900,
            refresh_token: token("secret-refresh"),
        };
        let debug = format!("{tokens:?} {:?}", session());
        assert!(!debug.contains("secret"), "{debug}");
    }

    #[test]
    fn recovery_verifier_and_invite_debug_is_redacted() {
        let verifier = vec![0xab; 32];
        let start = RegisterStartRequest {
            username: "alice".into(),
            registration_request: vec![],
            invite_code: "SECRET-INVITE".into(),
        };
        let register = RegisterFinishRequest {
            username: "alice".into(),
            invite_code: "SECRET-INVITE".into(),
            registration_upload: vec![],
            key_bundle: vec![],
            recovery_verifier: verifier.clone(),
            device_name: String::new(),
            session_kind: SessionKind::Native,
        };
        let recover = RecoverStartRequest {
            username: "alice".into(),
            recovery_verifier: verifier,
            registration_request: vec![],
        };
        let debug = format!("{start:?} {register:?} {recover:?}");
        assert!(debug.contains("alice"), "{debug}");
        assert!(!debug.contains("171"), "{debug}");
        assert!(!debug.contains("verifier"), "{debug}");
        assert!(!debug.contains("SECRET"), "{debug}");
    }

    #[test]
    fn refresh_classification() {
        let tokens = Tokens {
            access_token: token("a"),
            access_expires_in: 900,
            refresh_token: token("r"),
        };
        let ok = serde_json::to_vec(&tokens).unwrap();
        assert_eq!(
            RefreshOutcome::classify(Some(200), &ok),
            RefreshOutcome::Refreshed(tokens)
        );
        assert_eq!(
            RefreshOutcome::classify(Some(401), br#"{"error":"session_ended"}"#),
            RefreshOutcome::SessionEnded
        );
        for (status, body) in [
            (Some(429), &b""[..]),
            (Some(503), b""),
            (None, b""),
            (Some(500), br#"{"error":"session_ended"}"#),
            (Some(401), br#"{"error":"invalid_token"}"#),
            (Some(401), b"not json"),
            (Some(200), b"not json"),
        ] {
            assert_eq!(
                RefreshOutcome::classify(status, body),
                RefreshOutcome::Transient,
                "{status:?}"
            );
        }
    }
}
