//! The recovery entry: the vault key wrapped under the recovery key alone,
//! stored beside a local vault and inside its backups.
//!
//! ```text
//! offset  size  field
//! 0       1     format version (1)
//! 1       16    vault id
//! 17      74    envelope (kind 33, the vault key under the recovery key)
//! ```
//!
//! The envelope is the same wrap the server-backed version's key bundle
//! used for its recovery half. It is not secret on its own: it opens only
//! with the 128-bit recovery key.

use super::envelope::{self, Context};
use super::keys::SymmetricKey;
use super::{CryptoError, RecoveryKey, VaultId, VaultKey};

// Envelope kind 32 was the server key bundle's auth wrap of the vault key;
// it stays reserved so no new payload ever reuses it.
const KIND_VAULT_KEY_RECOVERY: u8 = 33;

const ENTRY_VERSION: u8 = 1;
const HEADER_LEN: usize = 1 + 16;
const ENVELOPE_LEN: usize = 32 + super::ENVELOPE_OVERHEAD;

/// The vault key wrapped under the [`RecoveryKey`], bound to its vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryEntry {
    vault: VaultId,
    envelope: Vec<u8>,
}

impl RecoveryEntry {
    /// Wraps `vault_key` for `vault` under `recovery`.
    pub fn create(
        vault: VaultId,
        vault_key: &VaultKey,
        recovery: &RecoveryKey,
    ) -> Result<Self, CryptoError> {
        Ok(Self {
            vault,
            envelope: wrap(
                &recovery.wrapping_key(vault),
                vault,
                KIND_VAULT_KEY_RECOVERY,
                vault_key,
            )?,
        })
    }

    /// Recovers the vault key. A wrong recovery key, or an entry moved to
    /// another vault ID, is [`CryptoError::Decrypt`].
    pub fn open(&self, recovery: &RecoveryKey) -> Result<VaultKey, CryptoError> {
        unwrap(
            &recovery.wrapping_key(self.vault),
            self.vault,
            KIND_VAULT_KEY_RECOVERY,
            &self.envelope,
        )
    }

    /// The vault this entry belongs to.
    pub fn vault_id(&self) -> VaultId {
        self.vault
    }

    /// Serializes the entry for storage.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.envelope.len());
        out.push(ENTRY_VERSION);
        out.extend_from_slice(&self.vault.to_bytes());
        out.extend_from_slice(&self.envelope);
        out
    }

    /// Parses bytes produced by [`to_bytes`](Self::to_bytes). The wrong
    /// length is [`CryptoError::Malformed`]; an unknown format version is
    /// [`CryptoError::UnsupportedVersion`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CryptoError> {
        let (&version, rest) = bytes.split_first().ok_or(CryptoError::Malformed)?;
        if version != ENTRY_VERSION {
            return Err(CryptoError::UnsupportedVersion(version));
        }
        if rest.len() != 16 + ENVELOPE_LEN {
            return Err(CryptoError::Malformed);
        }
        let (vault, envelope) = rest.split_at(16);
        Ok(Self {
            vault: VaultId::from_bytes(vault.try_into().expect("16 bytes")),
            envelope: envelope.to_vec(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::envelope::{self, Context};
    use crate::crypto::keys::SymmetricKey;
    use crate::crypto::{NoteKey, PayloadKind};
    use crate::note::NoteId;

    struct Fixture {
        vault: VaultId,
        vault_key: VaultKey,
        recovery: RecoveryKey,
        entry: RecoveryEntry,
    }

    fn fixture() -> Fixture {
        let vault = VaultId::from_parts(1_000, 1);
        let vault_key = VaultKey::generate().unwrap();
        let recovery = RecoveryKey::generate().unwrap();
        let entry = RecoveryEntry::create(vault, &vault_key, &recovery).unwrap();
        Fixture {
            vault,
            vault_key,
            recovery,
            entry,
        }
    }

    /// Checks that `opened` unwraps a note key the original vault key wrapped.
    fn assert_same_vault_key(f: &Fixture, opened: &VaultKey) {
        let note = NoteId::from_parts(1_000, 7);
        let note_key = NoteKey::generate().unwrap();
        let wrapped = f.vault_key.wrap_note_key(f.vault, note, &note_key).unwrap();
        let sealed = note_key
            .encrypt(f.vault, note, PayloadKind::NotePayload, b"x")
            .unwrap();
        let unwrapped = opened.unwrap_note_key(f.vault, note, &wrapped).unwrap();
        let plaintext = unwrapped
            .decrypt(f.vault, note, PayloadKind::NotePayload, &sealed)
            .unwrap();
        assert_eq!(&*plaintext, b"x");
    }

    #[test]
    fn opens_with_the_recovery_key() {
        let f = fixture();
        assert_same_vault_key(&f, &f.entry.open(&f.recovery).unwrap());
    }

    #[test]
    fn wrong_recovery_key_fails() {
        let f = fixture();
        let other = RecoveryKey::generate().unwrap();
        assert_eq!(f.entry.open(&other).unwrap_err(), CryptoError::Decrypt);
    }

    #[test]
    fn bound_to_its_vault() {
        let f = fixture();
        let moved = RecoveryEntry {
            vault: VaultId::from_parts(1_000, 2),
            ..f.entry.clone()
        };
        assert_eq!(moved.open(&f.recovery).unwrap_err(), CryptoError::Decrypt);

        // Same envelope bytes, re-serialized under another vault ID.
        let mut bytes = f.entry.to_bytes();
        bytes[1..17].copy_from_slice(&VaultId::from_parts(1_000, 2).to_bytes());
        let parsed = RecoveryEntry::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.open(&f.recovery).unwrap_err(), CryptoError::Decrypt);
    }

    #[test]
    fn serialization_round_trips() {
        let f = fixture();
        let bytes = f.entry.to_bytes();
        assert_eq!(bytes.len(), HEADER_LEN + ENVELOPE_LEN);
        assert_eq!(bytes[0], ENTRY_VERSION);
        assert_eq!(&bytes[1..17], &f.vault.to_bytes());
        let parsed = RecoveryEntry::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, f.entry);
        assert_eq!(parsed.vault_id(), f.vault);
        assert_same_vault_key(&f, &parsed.open(&f.recovery).unwrap());
    }

    #[test]
    fn malformed_bytes_are_rejected() {
        let bytes = fixture().entry.to_bytes();
        assert_eq!(RecoveryEntry::from_bytes(&[]), Err(CryptoError::Malformed));
        for len in 1..bytes.len() {
            assert_eq!(
                RecoveryEntry::from_bytes(&bytes[..len]),
                Err(CryptoError::Malformed),
                "length {len}"
            );
        }

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            RecoveryEntry::from_bytes(&trailing),
            Err(CryptoError::Malformed)
        );

        let mut wrong_version = bytes;
        wrong_version[0] = 2;
        assert_eq!(
            RecoveryEntry::from_bytes(&wrong_version),
            Err(CryptoError::UnsupportedVersion(2))
        );

        assert!(RecoveryEntry::from_bytes(&[0xff; 200]).is_err());
        let mut garbage = vec![ENTRY_VERSION];
        garbage.extend([0xa5; 16 + ENVELOPE_LEN]);
        let parsed = RecoveryEntry::from_bytes(&garbage).unwrap();
        assert!(parsed.open(&RecoveryKey::generate().unwrap()).is_err());
    }

    #[test]
    fn tampered_envelope_fails_to_open() {
        let f = fixture();
        let bytes = f.entry.to_bytes();
        for i in HEADER_LEN..bytes.len() {
            let mut bad = bytes.clone();
            bad[i] ^= 1;
            let entry = RecoveryEntry::from_bytes(&bad).unwrap();
            assert!(entry.open(&f.recovery).is_err(), "byte {i}");
        }
    }

    /// Pins the byte format: a fixed vault key, recovery key, vault ID, and
    /// nonce give these bytes, and every later build must open them.
    #[test]
    fn fixed_test_vector() {
        let vault = VaultId::from_parts(0x0102_0304_0506, 0x0708_090a_0b0c_0d0e_0f10);
        let vault_key = VaultKey(SymmetricKey::from_bytes(std::array::from_fn(|i| i as u8)));
        let recovery = RecoveryKey::from_random_for_tests([0x42; 16]);
        let nonce: [u8; 24] = std::array::from_fn(|i| 0xa0 + i as u8);
        let ctx = Context {
            vault,
            kind: KIND_VAULT_KEY_RECOVERY,
            note: None,
        };
        let envelope = envelope::seal_with_nonce(
            recovery.wrapping_key(vault).as_bytes(),
            &ctx,
            &nonce,
            &vault_key.to_secret_bytes()[..],
        )
        .unwrap();
        let entry = RecoveryEntry { vault, envelope };

        let hex: String = entry
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, EXPECTED_VECTOR);

        let bytes: Vec<u8> = (0..EXPECTED_VECTOR.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&EXPECTED_VECTOR[i..i + 2], 16).unwrap())
            .collect();
        let opened = RecoveryEntry::from_bytes(&bytes)
            .unwrap()
            .open(&recovery)
            .unwrap();
        assert_eq!(*opened.to_secret_bytes(), *vault_key.to_secret_bytes());
    }

    const EXPECTED_VECTOR: &str = concat!(
        "010102030405060708090a0b0c0d0e0f10",
        "0101a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7",
        "c2357cbaecdfba8265da29f9939d7a484b00442798c6a6c0ee44c851f349ba8a",
        "3fd3cc66e480b1184cccfe9de9db99af",
    );
}
