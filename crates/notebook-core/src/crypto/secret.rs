//! Auth secrets and derivation of the keys that wrap the vault key.

use std::fmt;

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::keys::SymmetricKey;
use super::{CryptoError, VaultId};

/// Minimum length of an [`AuthSecret`] in bytes.
pub const MIN_AUTH_SECRET_LEN: usize = 32;

const INFO_AUTH: &[u8] = b"notebook/v1/kek/auth";
const INFO_RECOVERY: &[u8] = b"notebook/v1/kek/recovery";

/// A high-entropy secret from the auth layer that unlocks the key bundle:
/// the OPAQUE export key, or the output of
/// [`derive_auth_secret`](super::derive_auth_secret).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AuthSecret(Vec<u8>);

impl AuthSecret {
    /// Copies `bytes` into a new secret. Fewer than
    /// [`MIN_AUTH_SECRET_LEN`] bytes is [`CryptoError::SecretTooShort`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        if bytes.len() < MIN_AUTH_SECRET_LEN {
            return Err(CryptoError::SecretTooShort);
        }
        Ok(Self(bytes.to_vec()))
    }

    pub(crate) fn wrapping_key(&self, vault: VaultId) -> SymmetricKey {
        derive_wrapping_key(&self.0, vault, INFO_AUTH)
    }
}

impl fmt::Debug for AuthSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthSecret(..)")
    }
}

/// The wrapping key for the recovery entry of a bundle.
pub(crate) fn recovery_wrapping_key(secret: &[u8], vault: VaultId) -> SymmetricKey {
    derive_wrapping_key(secret, vault, INFO_RECOVERY)
}

/// HKDF-SHA256 with the vault ID as salt, so one secret gives unrelated keys
/// for different vaults and the auth and recovery keys never coincide.
fn derive_wrapping_key(secret: &[u8], vault: VaultId, info: &[u8]) -> SymmetricKey {
    let mut okm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&vault.to_bytes()), secret)
        .expand(info, &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    let key = SymmetricKey::from_bytes(okm);
    okm.zeroize();
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_secret_is_rejected() {
        assert_eq!(
            AuthSecret::from_bytes(&[1; 31]).unwrap_err(),
            CryptoError::SecretTooShort
        );
        assert!(AuthSecret::from_bytes(&[1; 32]).is_ok());
        assert!(AuthSecret::from_bytes(&[1; 64]).is_ok());
    }

    #[test]
    fn wrapping_key_depends_on_vault_and_purpose() {
        let secret = AuthSecret::from_bytes(&[5; 64]).unwrap();
        let v1 = VaultId::from_parts(1_000, 1);
        let v2 = VaultId::from_parts(1_000, 2);
        let a = secret.wrapping_key(v1);
        let b = secret.wrapping_key(v2);
        let c = recovery_wrapping_key(&[5; 64], v1);
        assert_ne!(a.as_bytes(), b.as_bytes());
        assert_ne!(a.as_bytes(), c.as_bytes());
        assert_eq!(a.as_bytes(), secret.wrapping_key(v1).as_bytes());
    }

    #[test]
    fn debug_output_is_redacted() {
        let secret = AuthSecret::from_bytes(&[0xab; 32]).unwrap();
        assert_eq!(format!("{secret:?}"), "AuthSecret(..)");
    }
}
