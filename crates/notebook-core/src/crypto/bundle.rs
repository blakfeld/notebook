//! The key bundle: the vault key wrapped for server storage.
//!
//! ```text
//! offset  size  field
//! 0       1     bundle format version (1)
//! 1       16    vault id
//! 17      2     auth entry length (u16 big-endian)
//! 19      a     auth entry (envelope)
//! 19+a    2     recovery entry length (u16 big-endian)
//! 21+a    r     recovery entry (envelope)
//! ```

use super::envelope::{self, Context};
use super::keys::{SymmetricKey, VaultKey};
use super::{AuthSecret, CryptoError, RecoveryKey, VaultId};

const BUNDLE_VERSION: u8 = 1;
const KIND_VAULT_KEY_AUTH: u8 = 32;
const KIND_VAULT_KEY_RECOVERY: u8 = 33;

/// The vault key wrapped under an [`AuthSecret`] and, separately, under the
/// [`RecoveryKey`]. The server stores it as opaque bytes and can't unwrap it;
/// either secret alone recovers the [`VaultKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBundle {
    vault: VaultId,
    auth_entry: Vec<u8>,
    recovery_entry: Vec<u8>,
}

impl KeyBundle {
    /// Wraps `vault_key` for `vault` under both secrets.
    pub fn create(
        vault: VaultId,
        vault_key: &VaultKey,
        auth: &AuthSecret,
        recovery: &RecoveryKey,
    ) -> Result<Self, CryptoError> {
        Ok(Self {
            vault,
            auth_entry: wrap(
                &auth.wrapping_key(vault),
                vault,
                KIND_VAULT_KEY_AUTH,
                vault_key,
            )?,
            recovery_entry: wrap(
                &recovery.wrapping_key(vault),
                vault,
                KIND_VAULT_KEY_RECOVERY,
                vault_key,
            )?,
        })
    }

    /// The vault this bundle belongs to.
    pub fn vault_id(&self) -> VaultId {
        self.vault
    }

    /// Recovers the vault key with the auth secret.
    pub fn unlock_with_auth(&self, auth: &AuthSecret) -> Result<VaultKey, CryptoError> {
        unwrap(
            &auth.wrapping_key(self.vault),
            self.vault,
            KIND_VAULT_KEY_AUTH,
            &self.auth_entry,
        )
    }

    /// Recovers the vault key with the recovery key.
    pub fn unlock_with_recovery(&self, recovery: &RecoveryKey) -> Result<VaultKey, CryptoError> {
        unwrap(
            &recovery.wrapping_key(self.vault),
            self.vault,
            KIND_VAULT_KEY_RECOVERY,
            &self.recovery_entry,
        )
    }

    /// Replaces the auth entry after a passphrase change, unlocking with the
    /// old auth secret first so the new entry wraps the same vault key as the
    /// recovery entry. The recovery entry, the vault key, and everything it
    /// protects are unchanged; the old auth secret stops working.
    pub fn change_auth(&mut self, old: &AuthSecret, new: &AuthSecret) -> Result<(), CryptoError> {
        let vault_key = self.unlock_with_auth(old)?;
        self.rewrap_auth(&vault_key, new)
    }

    /// Replaces the auth entry after recovery, unlocking with the recovery
    /// key first. Otherwise the same as [`change_auth`](Self::change_auth).
    pub fn recover_auth(
        &mut self,
        recovery: &RecoveryKey,
        new: &AuthSecret,
    ) -> Result<(), CryptoError> {
        let vault_key = self.unlock_with_recovery(recovery)?;
        self.rewrap_auth(&vault_key, new)
    }

    /// Wraps `vault_key`, which must come from unlocking this bundle, as the
    /// new auth entry.
    fn rewrap_auth(&mut self, vault_key: &VaultKey, auth: &AuthSecret) -> Result<(), CryptoError> {
        self.auth_entry = wrap(
            &auth.wrapping_key(self.vault),
            self.vault,
            KIND_VAULT_KEY_AUTH,
            vault_key,
        )?;
        Ok(())
    }

    /// True when `other` carries a byte-identical recovery entry, compared in
    /// constant time. The server uses it to refuse a replacement bundle that
    /// would change how (or whether) the recovery key unlocks the vault.
    pub fn same_recovery_entry(&self, other: &KeyBundle) -> bool {
        let (a, b) = (&self.recovery_entry, &other.recovery_entry);
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }

    /// SHA-256 of [`to_bytes`](Self::to_bytes). A device compares it with
    /// the server's current bundle to notice a passphrase changed elsewhere.
    pub fn fingerprint(&self) -> [u8; 32] {
        use sha2::Digest;
        sha2::Sha256::digest(self.to_bytes()).into()
    }

    /// Serializes the bundle for storage.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(1 + 16 + 2 + self.auth_entry.len() + 2 + self.recovery_entry.len());
        out.push(BUNDLE_VERSION);
        out.extend_from_slice(&self.vault.to_bytes());
        for entry in [&self.auth_entry, &self.recovery_entry] {
            let len = u16::try_from(entry.len()).expect("entries are 74 bytes");
            out.extend_from_slice(&len.to_be_bytes());
            out.extend_from_slice(entry);
        }
        out
    }

    /// Parses bytes produced by [`to_bytes`](Self::to_bytes). Anything else,
    /// including trailing bytes, is [`CryptoError::Malformed`]; an unknown
    /// format version is [`CryptoError::UnsupportedVersion`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let mut reader = Reader(bytes);
        let version = reader.take(1)?[0];
        if version != BUNDLE_VERSION {
            return Err(CryptoError::UnsupportedVersion(version));
        }
        let vault = VaultId::from_bytes(reader.take(16)?.try_into().expect("16 bytes"));
        let auth_entry = reader.entry()?;
        let recovery_entry = reader.entry()?;
        if !reader.0.is_empty() {
            return Err(CryptoError::Malformed);
        }
        Ok(Self {
            vault,
            auth_entry,
            recovery_entry,
        })
    }
}

fn wrap(
    key: &SymmetricKey,
    vault: VaultId,
    kind: u8,
    vault_key: &VaultKey,
) -> Result<Vec<u8>, CryptoError> {
    let ctx = Context {
        vault,
        kind,
        note: None,
    };
    envelope::seal(key.as_bytes(), &ctx, vault_key.0.as_bytes())
}

fn unwrap(
    key: &SymmetricKey,
    vault: VaultId,
    kind: u8,
    entry: &[u8],
) -> Result<VaultKey, CryptoError> {
    let ctx = Context {
        vault,
        kind,
        note: None,
    };
    let bytes = envelope::open(key.as_bytes(), &ctx, entry)?;
    SymmetricKey::from_slice(&bytes).map(VaultKey)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CryptoError> {
        if self.0.len() < n {
            return Err(CryptoError::Malformed);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn entry(&mut self) -> Result<Vec<u8>, CryptoError> {
        let len = u16::from_be_bytes(self.take(2)?.try_into().expect("2 bytes"));
        Ok(self.take(len.into())?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{NoteKey, PayloadKind};
    use crate::note::NoteId;

    struct Fixture {
        vault: VaultId,
        vault_key: VaultKey,
        auth: AuthSecret,
        recovery: RecoveryKey,
        bundle: KeyBundle,
    }

    fn fixture() -> Fixture {
        let vault = VaultId::from_parts(1_000, 1);
        let vault_key = VaultKey::generate().unwrap();
        let auth = AuthSecret::from_bytes(&[3; 64]).unwrap();
        let recovery = RecoveryKey::generate().unwrap();
        let bundle = KeyBundle::create(vault, &vault_key, &auth, &recovery).unwrap();
        Fixture {
            vault,
            vault_key,
            auth,
            recovery,
            bundle,
        }
    }

    /// Checks that `unlocked` is the same vault key by unwrapping a note key
    /// the original wrapped and decrypting with it.
    fn assert_same_vault_key(f: &Fixture, unlocked: &VaultKey) {
        let note = NoteId::from_parts(1_000, 7);
        let note_key = NoteKey::generate().unwrap();
        let wrapped = f.vault_key.wrap_note_key(f.vault, note, &note_key).unwrap();
        let sealed = note_key
            .encrypt(f.vault, note, PayloadKind::NotePayload, b"x")
            .unwrap();
        let unwrapped = unlocked.unwrap_note_key(f.vault, note, &wrapped).unwrap();
        let plaintext = unwrapped
            .decrypt(f.vault, note, PayloadKind::NotePayload, &sealed)
            .unwrap();
        assert_eq!(&*plaintext, b"x");
    }

    #[test]
    fn unlocks_with_either_secret() {
        let f = fixture();
        assert_same_vault_key(&f, &f.bundle.unlock_with_auth(&f.auth).unwrap());
        assert_same_vault_key(&f, &f.bundle.unlock_with_recovery(&f.recovery).unwrap());
    }

    #[test]
    fn wrong_secrets_fail() {
        let f = fixture();
        let other_auth = AuthSecret::from_bytes(&[4; 64]).unwrap();
        let other_recovery = RecoveryKey::generate().unwrap();
        assert!(matches!(
            f.bundle.unlock_with_auth(&other_auth),
            Err(CryptoError::Decrypt)
        ));
        assert!(matches!(
            f.bundle.unlock_with_recovery(&other_recovery),
            Err(CryptoError::Decrypt)
        ));
    }

    #[test]
    fn entries_are_not_interchangeable() {
        let f = fixture();
        let swapped = KeyBundle {
            auth_entry: f.bundle.recovery_entry.clone(),
            recovery_entry: f.bundle.auth_entry.clone(),
            ..f.bundle.clone()
        };
        assert!(swapped.unlock_with_auth(&f.auth).is_err());
        assert!(swapped.unlock_with_recovery(&f.recovery).is_err());
    }

    #[test]
    fn serialization_round_trips() {
        let f = fixture();
        let bytes = f.bundle.to_bytes();
        assert_eq!(bytes.len(), 1 + 16 + 2 * (2 + 32 + 42));
        let parsed = KeyBundle::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, f.bundle);
        assert_eq!(parsed.vault_id(), f.vault);
        assert_same_vault_key(&f, &parsed.unlock_with_auth(&f.auth).unwrap());
    }

    #[test]
    fn malformed_bytes_are_rejected() {
        let bytes = fixture().bundle.to_bytes();
        for len in 0..bytes.len() {
            assert!(
                KeyBundle::from_bytes(&bytes[..len]).is_err(),
                "length {len}"
            );
        }

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            KeyBundle::from_bytes(&trailing),
            Err(CryptoError::Malformed)
        );

        let mut wrong_version = bytes;
        wrong_version[0] = 2;
        assert_eq!(
            KeyBundle::from_bytes(&wrong_version),
            Err(CryptoError::UnsupportedVersion(2))
        );

        assert!(KeyBundle::from_bytes(&[0xff; 200]).is_err());
    }

    #[test]
    fn bundle_is_bound_to_its_vault() {
        let f = fixture();
        let other_vault = VaultId::from_parts(1_000, 2);
        let moved = KeyBundle {
            vault: other_vault,
            ..f.bundle.clone()
        };
        assert!(moved.unlock_with_auth(&f.auth).is_err());
        assert!(moved.unlock_with_recovery(&f.recovery).is_err());
    }

    #[test]
    fn byte_layout() {
        let f = fixture();
        let bytes = f.bundle.to_bytes();
        assert_eq!(bytes[0], BUNDLE_VERSION);
        assert_eq!(&bytes[1..17], &f.vault.to_bytes());
        assert_eq!(&bytes[17..19], &[0, 74]);
        assert_eq!(&bytes[19..93], &f.bundle.auth_entry[..]);
        assert_eq!(&bytes[93..95], &[0, 74]);
        assert_eq!(&bytes[95..], &f.bundle.recovery_entry[..]);
    }

    #[test]
    fn truncations_are_malformed() {
        let bytes = fixture().bundle.to_bytes();
        for len in 0..bytes.len() {
            assert_eq!(
                KeyBundle::from_bytes(&bytes[..len]),
                Err(CryptoError::Malformed),
                "length {len}"
            );
        }
    }

    #[test]
    fn length_prefix_past_the_end_is_malformed() {
        let mut bytes = fixture().bundle.to_bytes();
        bytes[17..19].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(KeyBundle::from_bytes(&bytes), Err(CryptoError::Malformed));
    }

    #[test]
    fn parsing_accepts_any_entry_lengths() {
        let mut bytes = vec![BUNDLE_VERSION];
        bytes.extend([0x11; 16]);
        bytes.extend([0, 0, 0, 3, 1, 2, 3]);
        let bundle = KeyBundle::from_bytes(&bytes).unwrap();
        assert!(bundle.auth_entry.is_empty());
        assert_eq!(bundle.recovery_entry, [1, 2, 3]);
        assert_eq!(bundle.to_bytes(), bytes);

        let auth = AuthSecret::from_bytes(&[3; 64]).unwrap();
        assert!(matches!(
            bundle.unlock_with_auth(&auth),
            Err(CryptoError::Malformed)
        ));
    }

    #[test]
    fn tampered_entries_fail_to_unlock() {
        let f = fixture();
        let bytes = f.bundle.to_bytes();
        for i in 19..bytes.len() {
            if (93..95).contains(&i) {
                continue;
            }
            let mut bad = bytes.clone();
            bad[i] ^= 1;
            let bundle = KeyBundle::from_bytes(&bad).unwrap();
            let result = if i < 93 {
                bundle.unlock_with_auth(&f.auth)
            } else {
                bundle.unlock_with_recovery(&f.recovery)
            };
            assert!(result.is_err(), "byte {i}");
        }
    }

    #[test]
    fn unwrapped_value_of_the_wrong_length_is_malformed() {
        let f = fixture();
        let ctx = Context {
            vault: f.vault,
            kind: KIND_VAULT_KEY_AUTH,
            note: None,
        };
        let entry =
            envelope::seal(f.auth.wrapping_key(f.vault).as_bytes(), &ctx, &[0; 16]).unwrap();
        let bundle = KeyBundle {
            auth_entry: entry,
            ..f.bundle.clone()
        };
        assert!(matches!(
            bundle.unlock_with_auth(&f.auth),
            Err(CryptoError::Malformed)
        ));
    }

    #[test]
    fn creating_twice_gives_different_bytes_for_the_same_key() {
        let f = fixture();
        let again = KeyBundle::create(f.vault, &f.vault_key, &f.auth, &f.recovery).unwrap();
        assert_ne!(again.to_bytes(), f.bundle.to_bytes());
        assert_same_vault_key(&f, &again.unlock_with_auth(&f.auth).unwrap());
    }

    #[test]
    fn vault_key_kinds_differ_from_note_kinds() {
        assert_ne!(KIND_VAULT_KEY_AUTH, KIND_VAULT_KEY_RECOVERY);
        for kind in [KIND_VAULT_KEY_AUTH, KIND_VAULT_KEY_RECOVERY] {
            assert!(![1, 2, 3, crate::crypto::keys::KIND_WRAPPED_NOTE_KEY].contains(&kind));
        }
    }

    #[test]
    fn fingerprint_changes_with_the_auth_entry() {
        let mut f = fixture();
        let before = f.bundle.fingerprint();
        let parsed = KeyBundle::from_bytes(&f.bundle.to_bytes()).unwrap();
        assert_eq!(parsed.fingerprint(), before);
        let new_auth = AuthSecret::from_bytes(&[4; 64]).unwrap();
        f.bundle.change_auth(&f.auth, &new_auth).unwrap();
        assert_ne!(f.bundle.fingerprint(), before);
    }

    #[test]
    fn rewrap_survives_serialization() {
        let mut f = fixture();
        let new_auth = AuthSecret::from_bytes(&[9; 64]).unwrap();
        f.bundle.change_auth(&f.auth, &new_auth).unwrap();
        let parsed = KeyBundle::from_bytes(&f.bundle.to_bytes()).unwrap();
        assert_eq!(parsed.vault_id(), f.vault);
        assert_same_vault_key(&f, &parsed.unlock_with_auth(&new_auth).unwrap());
        assert!(parsed.unlock_with_auth(&f.auth).is_err());
    }

    #[test]
    fn passphrase_change_rewraps_only_the_auth_entry() {
        let mut f = fixture();
        let recovery_entry = f.bundle.recovery_entry.clone();
        let new_auth = AuthSecret::from_bytes(&[9; 64]).unwrap();

        f.bundle.recover_auth(&f.recovery, &new_auth).unwrap();

        assert_eq!(f.bundle.recovery_entry, recovery_entry);
        assert_same_vault_key(&f, &f.bundle.unlock_with_auth(&new_auth).unwrap());
        assert_same_vault_key(&f, &f.bundle.unlock_with_recovery(&f.recovery).unwrap());
        assert!(f.bundle.unlock_with_auth(&f.auth).is_err());
    }

    #[test]
    fn recovery_entry_survives_rewrapping() {
        let f = fixture();
        let new_auth = AuthSecret::from_bytes(&[9; 64]).unwrap();

        let mut changed = f.bundle.clone();
        changed.change_auth(&f.auth, &new_auth).unwrap();
        assert!(changed.same_recovery_entry(&f.bundle));

        let mut recovered = f.bundle.clone();
        recovered.recover_auth(&f.recovery, &new_auth).unwrap();
        assert!(recovered.same_recovery_entry(&f.bundle));
    }

    #[test]
    fn different_recovery_key_is_a_different_entry() {
        let f = fixture();
        let other = KeyBundle::create(
            f.vault,
            &f.vault_key,
            &f.auth,
            &RecoveryKey::generate().unwrap(),
        )
        .unwrap();
        assert!(!other.same_recovery_entry(&f.bundle));

        let truncated = KeyBundle {
            recovery_entry: f.bundle.recovery_entry[..10].to_vec(),
            ..f.bundle.clone()
        };
        assert!(!truncated.same_recovery_entry(&f.bundle));
    }

    #[test]
    fn rewrapping_requires_a_secret_that_unlocks() {
        let mut f = fixture();
        let before = f.bundle.clone();
        let new_auth = AuthSecret::from_bytes(&[9; 64]).unwrap();
        let wrong_auth = AuthSecret::from_bytes(&[4; 64]).unwrap();
        let wrong_recovery = RecoveryKey::generate().unwrap();
        assert_eq!(
            f.bundle.change_auth(&wrong_auth, &new_auth),
            Err(CryptoError::Decrypt)
        );
        assert_eq!(
            f.bundle.recover_auth(&wrong_recovery, &new_auth),
            Err(CryptoError::Decrypt)
        );
        assert_eq!(f.bundle, before);
    }
}
