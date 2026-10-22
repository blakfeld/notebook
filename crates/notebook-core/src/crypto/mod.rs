//! End-to-end encryption for vaults.
//!
//! # Key hierarchy
//!
//! - Each account has one random [`VaultKey`].
//! - Each note has its own random [`NoteKey`], stored only wrapped by the
//!   vault key ([`VaultKey::wrap_note_key`]). Note payloads, CRDT updates,
//!   and snapshots are encrypted with the note key ([`NoteKey::encrypt`]).
//! - The vault key is stored on the server in a [`KeyBundle`], wrapped twice:
//!   once under a key derived from an [`AuthSecret`] (the OPAQUE export key,
//!   or a passphrase via [`derive_auth_secret`]) and once under a key derived
//!   from the [`RecoveryKey`]. Either one alone unlocks the vault.
//!
//! # Ciphertext envelope
//!
//! Every ciphertext uses one versioned layout:
//!
//! ```text
//! offset  size  field
//! 0       1     version   (1)
//! 1       1     algorithm (1 = XChaCha20-Poly1305)
//! 2       24    nonce
//! 26      n+16  ciphertext || Poly1305 tag
//! ```
//!
//! The header, the vault ID, the note ID, and the payload kind are all
//! authenticated, so a ciphertext only decrypts in the exact context it was
//! written for. Every authentication failure is the same
//! [`CryptoError::Decrypt`].
//!
//! Keys, nonces, and salts come from the platform's secure random source
//! (`crypto.getRandomValues` in the browser). Key material is zeroized on drop
//! and never appears in `Debug` output.

mod bundle;
mod envelope;
mod kdf;
mod keys;
mod recovery;
mod secret;

use std::fmt;
use std::str::FromStr;

use ulid::Ulid;

pub use bundle::KeyBundle;
pub use envelope::{ALGORITHM_XCHACHA20POLY1305, ENVELOPE_OVERHEAD, ENVELOPE_VERSION};
pub use kdf::{KdfVersion, Salt, derive_auth_secret};
pub use keys::{NoteKey, PayloadKind, VaultKey};
pub use recovery::RecoveryKey;
pub use secret::{AuthSecret, MIN_AUTH_SECRET_LEN};

/// Errors from the vault cryptography.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// The platform's secure random source failed.
    Random,
    /// Authentication failed: wrong key, wrong context, or tampered data.
    /// Deliberately carries no detail.
    Decrypt,
    /// The ciphertext uses an envelope version this build doesn't know.
    UnsupportedVersion(u8),
    /// The ciphertext uses an algorithm this build doesn't know.
    UnsupportedAlgorithm(u8),
    /// The bytes are too short or otherwise not a ciphertext or key bundle.
    Malformed,
    /// An auth secret shorter than [`MIN_AUTH_SECRET_LEN`] bytes.
    SecretTooShort,
    /// Recovery key text with the wrong length, characters, or checksum.
    InvalidRecoveryKey,
    /// A passphrase-derivation parameter version this build doesn't know.
    UnsupportedKdfVersion(u8),
    /// Passphrase derivation failed.
    Kdf,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Random => f.write_str("secure random source failed"),
            Self::Decrypt => f.write_str("decryption failed"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported ciphertext version {v}"),
            Self::UnsupportedAlgorithm(a) => write!(f, "unsupported ciphertext algorithm {a}"),
            Self::Malformed => f.write_str("malformed ciphertext or key bundle"),
            Self::SecretTooShort => write!(
                f,
                "auth secret must be at least {MIN_AUTH_SECRET_LEN} bytes"
            ),
            Self::InvalidRecoveryKey => f.write_str("invalid recovery key"),
            Self::UnsupportedKdfVersion(v) => write!(f, "unsupported key derivation version {v}"),
            Self::Kdf => f.write_str("key derivation failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Identifier of a vault: a ULID the client chooses when the vault is created,
/// so the key bundle can be bound to it before anything reaches the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VaultId(Ulid);

impl VaultId {
    /// Builds an identifier from a creation time in Unix milliseconds and 80
    /// bits of caller-supplied randomness (higher bits are ignored).
    pub fn from_parts(timestamp_ms: u64, random: u128) -> Self {
        Self(Ulid::from_parts(timestamp_ms, random))
    }

    /// The 16-byte binary ULID, used to bind ciphertexts to their vault and
    /// as the stored form.
    pub fn to_bytes(self) -> [u8; 16] {
        self.0.to_bytes()
    }

    /// Rebuilds an identifier from its [`to_bytes`](Self::to_bytes) form.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(Ulid::from_bytes(bytes))
    }
}

impl fmt::Display for VaultId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Error returned when text is not a valid vault identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseVaultIdError;

impl fmt::Display for ParseVaultIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid vault identifier")
    }
}

impl std::error::Error for ParseVaultIdError {}

impl FromStr for VaultId {
    type Err = ParseVaultIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s)
            .map(Self)
            .map_err(|_| ParseVaultIdError)
    }
}

/// Fills `dest` from the platform's secure random source.
pub(crate) fn fill_random(dest: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(dest).map_err(|_| CryptoError::Random)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_id_round_trips_through_text() {
        let id = VaultId::from_parts(1_700_000_000_000, 0xdead_beef);
        let text = id.to_string();
        assert_eq!(text.len(), 26);
        assert_eq!(text.parse::<VaultId>(), Ok(id));
    }

    #[test]
    fn vault_id_round_trips_through_bytes() {
        let id = VaultId::from_parts(1_700_000_000_000, 0xdead_beef);
        assert_eq!(VaultId::from_bytes(id.to_bytes()), id);
    }

    #[test]
    fn invalid_vault_id_text_is_rejected() {
        for text in [
            "",
            "not-a-ulid",
            "01ARZ3NDEKTSV4RRFFQ69G5FA",
            "01ARZ3NDEKTSV4RRFFQ69G5FAVX",
        ] {
            assert_eq!(text.parse::<VaultId>(), Err(ParseVaultIdError), "{text:?}");
        }
    }

    #[test]
    fn vault_id_bytes_are_big_endian_ulid() {
        let id = VaultId::from_parts(0x0102_0304_0506, 0x0708_090a_0b0c_0d0e_0f10);
        assert_eq!(
            id.to_bytes(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        assert_eq!(
            id.to_string(),
            VaultId::from_bytes(id.to_bytes()).to_string()
        );
    }

    #[test]
    fn vault_ids_order_by_creation_time() {
        let earlier = VaultId::from_parts(1_000, u128::MAX);
        let later = VaultId::from_parts(2_000, 0);
        assert!(earlier < later);
        assert!(earlier.to_string() < later.to_string());
    }

    #[test]
    fn parse_vault_id_error_is_a_std_error() {
        let err = "nope".parse::<VaultId>().unwrap_err();
        assert_eq!(err.to_string(), "invalid vault identifier");
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn error_messages() {
        let cases = [
            (CryptoError::Random, "secure random source failed"),
            (CryptoError::Decrypt, "decryption failed"),
            (
                CryptoError::UnsupportedVersion(7),
                "unsupported ciphertext version 7",
            ),
            (
                CryptoError::UnsupportedAlgorithm(9),
                "unsupported ciphertext algorithm 9",
            ),
            (CryptoError::Malformed, "malformed ciphertext or key bundle"),
            (
                CryptoError::SecretTooShort,
                "auth secret must be at least 32 bytes",
            ),
            (CryptoError::InvalidRecoveryKey, "invalid recovery key"),
            (
                CryptoError::UnsupportedKdfVersion(3),
                "unsupported key derivation version 3",
            ),
            (CryptoError::Kdf, "key derivation failed"),
        ];
        for (err, message) in cases {
            assert_eq!(err.to_string(), message);
            let _: &dyn std::error::Error = &err;
        }
    }

    #[test]
    fn random_fill_handles_empty_and_large_buffers() {
        fill_random(&mut []).unwrap();
        let mut big = vec![0u8; 4096];
        fill_random(&mut big).unwrap();
        assert!(big.iter().any(|&b| b != 0));
    }

    #[test]
    fn random_fill_produces_different_bytes() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        fill_random(&mut a).unwrap();
        fill_random(&mut b).unwrap();
        assert_ne!(a, b);
    }
}
