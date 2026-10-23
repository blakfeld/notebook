//! Accounts: the client side of OPAQUE registration, login, passphrase
//! change, and recovery, plus the wire types shared with the server.
//!
//! Everything here is sans-I/O and builds for `wasm32-unknown-unknown`. The
//! server uses [`Suite`] and [`SecureRng`] with `opaque-ke`'s server types so
//! both sides agree on the cipher suite.
//!
//! The OPAQUE export key from login unwraps the [`KeyBundle`](crate::crypto::KeyBundle),
//! so one passphrase both authenticates and unlocks the vault. The server
//! never sees the passphrase or anything that unwraps the vault key.

pub mod api;
mod client;
pub mod http;
#[cfg(all(feature = "transport-native", not(target_arch = "wasm32")))]
mod http_native;
#[cfg(all(feature = "transport-wasm", target_arch = "wasm32"))]
mod http_wasm;
mod suite;
mod username;

use std::fmt;

use crate::crypto::CryptoError;

pub use client::{
    Device, Login, LoginKeys, MIN_PASSPHRASE_CHARS, NewAccount, PassphraseChange, Recovery,
    Registration,
};
#[cfg(all(feature = "transport-native", not(target_arch = "wasm32")))]
pub use http_native::NativeAuthHttp;
#[cfg(all(feature = "transport-wasm", target_arch = "wasm32"))]
pub use http_wasm::WasmAuthHttp;
pub use suite::{Argon2Ksf, SecureRng, Suite};
pub use username::Username;

/// Errors from the client auth flows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The username breaks the rules in [`Username::parse`].
    InvalidUsername,
    /// A new passphrase shorter than [`MIN_PASSPHRASE_CHARS`].
    PassphraseTooShort,
    /// Wrong passphrase, or no such account. Deliberately indistinguishable.
    InvalidCredentials,
    /// A server message that doesn't parse or fails an OPAQUE check.
    Protocol,
    /// Key bundle handling failed, e.g. a bundle that doesn't unlock.
    Crypto(CryptoError),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUsername => f.write_str(
                "usernames are 3 to 32 letters, digits, '.', '_', or '-', \
                 starting with a letter or digit",
            ),
            Self::PassphraseTooShort => write!(
                f,
                "passphrase must be at least {MIN_PASSPHRASE_CHARS} characters"
            ),
            Self::InvalidCredentials => f.write_str("wrong username or passphrase"),
            Self::Protocol => f.write_str("unexpected response from the server"),
            Self::Crypto(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<CryptoError> for AuthError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}
