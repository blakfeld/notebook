//! The vault key wrapped under the user's passphrase on one device, as the
//! server-backed version stored it for a locked app. Kept only to upgrade
//! those installs.
//!
//! Layout: `kdf_version (1) || salt (16) || envelope`. The envelope seals the
//! vault key under a key derived from the passphrase with Argon2id
//! ([`derive_auth_secret`]) and then HKDF with its own label, bound to the
//! vault ID, so it can't be confused with any other wrap of the vault key.

use std::fmt;

use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use super::envelope::{self, Context};
use super::keys::SymmetricKey;
use super::{CryptoError, KdfVersion, Salt, VaultId, VaultKey, derive_auth_secret};

/// Envelope kind for the vault key under a device-local passphrase key.
const KIND_VAULT_KEY_LOCAL: u8 = 34;
const SALT_LEN: usize = 16;

/// A device-local passphrase wrap of the vault key. Not secret by itself,
/// but only as strong as the passphrase, so keep it in the OS credential
/// store rather than in a file.
#[derive(Clone, PartialEq, Eq)]
pub struct LocalUnlock {
    vault: VaultId,
    version: KdfVersion,
    salt: Salt,
    sealed: Vec<u8>,
}

impl LocalUnlock {
    /// Wraps `vault_key` under `passphrase` with a fresh salt and the current
    /// Argon2id parameters. Slow by design: run it off the UI thread.
    pub fn seal(
        vault: VaultId,
        vault_key: &VaultKey,
        passphrase: &str,
    ) -> Result<Self, CryptoError> {
        let version = KdfVersion::CURRENT;
        let salt = Salt::generate()?;
        let key = derive_key(vault, passphrase, &salt, version)?;
        let sealed = envelope::seal(key.as_bytes(), &context(vault), vault_key.0.as_bytes())?;
        Ok(Self {
            vault,
            version,
            salt,
            sealed,
        })
    }

    /// Unwraps the vault key. A wrong passphrase is [`CryptoError::Decrypt`].
    /// Slow by design: run it off the UI thread.
    pub fn open(&self, passphrase: &str) -> Result<VaultKey, CryptoError> {
        let key = derive_key(self.vault, passphrase, &self.salt, self.version)?;
        let bytes = envelope::open(key.as_bytes(), &context(self.vault), &self.sealed)?;
        SymmetricKey::from_slice(&bytes).map(VaultKey)
    }

    pub fn vault_id(&self) -> VaultId {
        self.vault
    }

    /// The stored form. The vault ID isn't included; it is authenticated, so
    /// [`from_bytes`](Self::from_bytes) must be given the same one.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + SALT_LEN + self.sealed.len());
        out.push(self.version.to_u8());
        out.extend_from_slice(self.salt.as_bytes());
        out.extend_from_slice(&self.sealed);
        out
    }

    pub fn from_bytes(vault: VaultId, bytes: &[u8]) -> Result<Self, CryptoError> {
        let (&version, rest) = bytes.split_first().ok_or(CryptoError::Malformed)?;
        let version = KdfVersion::from_u8(version)?;
        if rest.len() < SALT_LEN + envelope::ENVELOPE_OVERHEAD {
            return Err(CryptoError::Malformed);
        }
        let (salt, sealed) = rest.split_at(SALT_LEN);
        let salt = Salt::from_bytes(salt.try_into().map_err(|_| CryptoError::Malformed)?);
        Ok(Self {
            vault,
            version,
            salt,
            sealed: sealed.to_vec(),
        })
    }
}

impl fmt::Debug for LocalUnlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LocalUnlock(..)")
    }
}

fn context(vault: VaultId) -> Context {
    Context {
        vault,
        kind: KIND_VAULT_KEY_LOCAL,
        note: None,
    }
}

/// Argon2id over the NFC-normalized passphrase (as the auth flows normalize
/// it), then HKDF to a wrapping key for `vault`.
fn derive_key(
    vault: VaultId,
    passphrase: &str,
    salt: &Salt,
    version: KdfVersion,
) -> Result<SymmetricKey, CryptoError> {
    let mut normalized = Zeroizing::new(String::with_capacity(passphrase.len() * 3));
    normalized.extend(passphrase.nfc());
    let secret = derive_auth_secret(normalized.as_bytes(), salt, version)?;
    Ok(secret.local_wrapping_key(vault))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> VaultId {
        VaultId::from_parts(1_000, 1)
    }

    fn same_key(a: &VaultKey, b: &VaultKey) -> bool {
        *a.to_secret_bytes() == *b.to_secret_bytes()
    }

    #[test]
    fn round_trips_through_bytes() {
        let key = VaultKey::generate().unwrap();
        let local = LocalUnlock::seal(vault(), &key, "correct horse battery").unwrap();
        let restored = LocalUnlock::from_bytes(vault(), &local.to_bytes()).unwrap();
        assert_eq!(restored, local);
        assert!(same_key(
            &restored.open("correct horse battery").unwrap(),
            &key
        ));
    }

    #[test]
    fn passphrase_is_normalized() {
        let key = VaultKey::generate().unwrap();
        // "é" precomposed, then as "e" + combining acute.
        let local = LocalUnlock::seal(vault(), &key, "caf\u{e9} passphrase").unwrap();
        assert!(same_key(
            &local.open("cafe\u{301} passphrase").unwrap(),
            &key
        ));
    }

    #[test]
    fn wrong_passphrase_fails() {
        let key = VaultKey::generate().unwrap();
        let local = LocalUnlock::seal(vault(), &key, "correct horse battery").unwrap();
        assert_eq!(
            local.open("wrong horse battery").unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn bound_to_its_vault() {
        let key = VaultKey::generate().unwrap();
        let bytes = LocalUnlock::seal(vault(), &key, "correct horse battery")
            .unwrap()
            .to_bytes();
        let other = LocalUnlock::from_bytes(VaultId::from_parts(1_000, 2), &bytes).unwrap();
        assert_eq!(
            other.open("correct horse battery").unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn corrupted_blobs_are_rejected() {
        let key = VaultKey::generate().unwrap();
        let bytes = LocalUnlock::seal(vault(), &key, "correct horse battery")
            .unwrap()
            .to_bytes();

        assert_eq!(
            LocalUnlock::from_bytes(vault(), &[]).unwrap_err(),
            CryptoError::Malformed
        );
        assert_eq!(
            LocalUnlock::from_bytes(vault(), &bytes[..20]).unwrap_err(),
            CryptoError::Malformed
        );
        let mut unknown = bytes.clone();
        unknown[0] = 9;
        assert_eq!(
            LocalUnlock::from_bytes(vault(), &unknown).unwrap_err(),
            CryptoError::UnsupportedKdfVersion(9)
        );
        let mut tampered = bytes.clone();
        *tampered.last_mut().unwrap() ^= 1;
        let tampered = LocalUnlock::from_bytes(vault(), &tampered).unwrap();
        assert_eq!(
            tampered.open("correct horse battery").unwrap_err(),
            CryptoError::Decrypt
        );
        let mut salted = bytes;
        salted[1] ^= 1;
        let salted = LocalUnlock::from_bytes(vault(), &salted).unwrap();
        assert_eq!(
            salted.open("correct horse battery").unwrap_err(),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn vault_key_bytes_round_trip() {
        let key = VaultKey::generate().unwrap();
        let restored = VaultKey::from_secret_bytes(&*key.to_secret_bytes()).unwrap();
        assert!(same_key(&key, &restored));
        assert_eq!(
            VaultKey::from_secret_bytes(&[0; 31]).unwrap_err(),
            CryptoError::Malformed
        );
    }
}
