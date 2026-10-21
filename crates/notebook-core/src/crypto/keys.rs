//! Vault and note keys, and encryption of note data.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use super::envelope::{self, Context};
use super::{CryptoError, VaultId, fill_random};
use crate::note::NoteId;

/// Envelope kind for a note key wrapped by the vault key.
pub(crate) const KIND_WRAPPED_NOTE_KEY: u8 = 16;

/// A 256-bit key that is zeroized on drop and never printed.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct SymmetricKey([u8; 32]);

impl SymmetricKey {
    pub(crate) fn generate() -> Result<Self, CryptoError> {
        let mut key = Self([0; 32]);
        fill_random(&mut key.0)?;
        Ok(key)
    }

    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Builds a key from a decrypted 32-byte buffer.
    pub(crate) fn from_slice(bytes: &[u8]) -> Result<Self, CryptoError> {
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| CryptoError::Malformed)?;
        Ok(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The kind of note data being encrypted. It is authenticated with the
/// ciphertext, so an update can't be passed off as a snapshot or vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayloadKind {
    /// Note content or metadata outside the CRDT.
    NotePayload,
    /// One CRDT update.
    CrdtUpdate,
    /// A CRDT snapshot covering a note's history up to some point.
    CrdtSnapshot,
}

impl PayloadKind {
    fn to_u8(self) -> u8 {
        match self {
            Self::NotePayload => 1,
            Self::CrdtUpdate => 2,
            Self::CrdtSnapshot => 3,
        }
    }
}

/// The account's root key. It wraps every note key and is itself stored only
/// inside a [`KeyBundle`](super::KeyBundle).
pub struct VaultKey(pub(crate) SymmetricKey);

impl VaultKey {
    /// Generates a new random vault key.
    pub fn generate() -> Result<Self, CryptoError> {
        SymmetricKey::generate().map(Self)
    }

    /// Encrypts `note_key` for storage, bound to `vault` and `note`.
    pub fn wrap_note_key(
        &self,
        vault: VaultId,
        note: NoteId,
        note_key: &NoteKey,
    ) -> Result<Vec<u8>, CryptoError> {
        envelope::seal(
            self.0.as_bytes(),
            &wrap_context(vault, note),
            note_key.0.as_bytes(),
        )
    }

    /// Decrypts a note key produced by [`wrap_note_key`](Self::wrap_note_key)
    /// for the same vault and note.
    pub fn unwrap_note_key(
        &self,
        vault: VaultId,
        note: NoteId,
        wrapped: &[u8],
    ) -> Result<NoteKey, CryptoError> {
        let bytes = envelope::open(self.0.as_bytes(), &wrap_context(vault, note), wrapped)?;
        SymmetricKey::from_slice(&bytes).map(NoteKey)
    }
}

impl fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultKey(..)")
    }
}

/// A per-note key. It encrypts the note's payloads, CRDT updates, and
/// snapshots, and is stored only wrapped by the [`VaultKey`].
pub struct NoteKey(SymmetricKey);

impl NoteKey {
    /// Generates a new random note key.
    pub fn generate() -> Result<Self, CryptoError> {
        SymmetricKey::generate().map(Self)
    }

    /// Encrypts `plaintext` as `kind` data of `note` in `vault`, with a fresh
    /// random nonce.
    pub fn encrypt(
        &self,
        vault: VaultId,
        note: NoteId,
        kind: PayloadKind,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        envelope::seal(
            self.0.as_bytes(),
            &data_context(vault, note, kind),
            plaintext,
        )
    }

    /// Decrypts a ciphertext produced by [`encrypt`](Self::encrypt) with the
    /// same vault, note, and kind. Any mismatch, tampering, or wrong key is
    /// [`CryptoError::Decrypt`].
    pub fn decrypt(
        &self,
        vault: VaultId,
        note: NoteId,
        kind: PayloadKind,
        ciphertext: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        envelope::open(
            self.0.as_bytes(),
            &data_context(vault, note, kind),
            ciphertext,
        )
    }
}

impl fmt::Debug for NoteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NoteKey(..)")
    }
}

fn data_context(vault: VaultId, note: NoteId, kind: PayloadKind) -> Context {
    Context {
        vault,
        kind: kind.to_u8(),
        note: Some(note),
    }
}

fn wrap_context(vault: VaultId, note: NoteId) -> Context {
    Context {
        vault,
        kind: KIND_WRAPPED_NOTE_KEY,
        note: Some(note),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: VaultId = vault(1);

    const fn vault(n: u8) -> VaultId {
        VaultId(ulid::Ulid(n as u128))
    }

    fn note(n: u128) -> NoteId {
        NoteId::from_parts(1_000, n)
    }

    #[test]
    fn generated_keys_differ() {
        let (a, b) = (VaultKey::generate().unwrap(), VaultKey::generate().unwrap());
        assert_ne!(a.0.as_bytes(), b.0.as_bytes());
        let (a, b) = (NoteKey::generate().unwrap(), NoteKey::generate().unwrap());
        assert_ne!(a.0.as_bytes(), b.0.as_bytes());
    }

    #[test]
    fn note_data_round_trips() {
        let key = NoteKey::generate().unwrap();
        for kind in [
            PayloadKind::NotePayload,
            PayloadKind::CrdtUpdate,
            PayloadKind::CrdtSnapshot,
        ] {
            for plaintext in [&b""[..], b"# Groceries\n#home"] {
                let sealed = key.encrypt(V, note(1), kind, plaintext).unwrap();
                assert_eq!(&*key.decrypt(V, note(1), kind, &sealed).unwrap(), plaintext);
            }
        }
    }

    #[test]
    fn same_plaintext_encrypts_differently() {
        let key = NoteKey::generate().unwrap();
        let a = key
            .encrypt(V, note(1), PayloadKind::CrdtUpdate, b"x")
            .unwrap();
        let b = key
            .encrypt(V, note(1), PayloadKind::CrdtUpdate, b"x")
            .unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn ciphertext_hides_plaintext() {
        let key = NoteKey::generate().unwrap();
        let marker = b"SECRET-MARKER-1234567890";
        let sealed = key
            .encrypt(V, note(1), PayloadKind::NotePayload, marker)
            .unwrap();
        assert!(!sealed.windows(marker.len()).any(|w| w == marker));
    }

    #[test]
    fn ciphertext_is_bound_to_its_context() {
        let key = NoteKey::generate().unwrap();
        let sealed = key
            .encrypt(V, note(1), PayloadKind::CrdtUpdate, b"edit")
            .unwrap();
        let wrong_note = key.decrypt(V, note(2), PayloadKind::CrdtUpdate, &sealed);
        let wrong_vault = key.decrypt(vault(2), note(1), PayloadKind::CrdtUpdate, &sealed);
        let wrong_kind = key.decrypt(V, note(1), PayloadKind::CrdtSnapshot, &sealed);
        for result in [wrong_note, wrong_vault, wrong_kind] {
            assert_eq!(result, Err(CryptoError::Decrypt));
        }
    }

    #[test]
    fn tampering_and_wrong_key_give_the_same_error() {
        let key = NoteKey::generate().unwrap();
        let sealed = key
            .encrypt(V, note(1), PayloadKind::NotePayload, b"hi")
            .unwrap();

        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        let tampered = key.decrypt(V, note(1), PayloadKind::NotePayload, &tampered);

        let other = NoteKey::generate().unwrap();
        let wrong_key = other.decrypt(V, note(1), PayloadKind::NotePayload, &sealed);

        assert_eq!(tampered, Err(CryptoError::Decrypt));
        assert_eq!(wrong_key, Err(CryptoError::Decrypt));
    }

    #[test]
    fn note_key_wraps_and_unwraps() {
        let vault_key = VaultKey::generate().unwrap();
        let note_key = NoteKey::generate().unwrap();
        let sealed = note_key
            .encrypt(V, note(1), PayloadKind::CrdtSnapshot, b"doc")
            .unwrap();

        let wrapped = vault_key.wrap_note_key(V, note(1), &note_key).unwrap();
        let unwrapped = vault_key.unwrap_note_key(V, note(1), &wrapped).unwrap();
        let plaintext = unwrapped
            .decrypt(V, note(1), PayloadKind::CrdtSnapshot, &sealed)
            .unwrap();
        assert_eq!(&*plaintext, b"doc");
    }

    #[test]
    fn wrapped_note_key_does_not_move() {
        let vault_key = VaultKey::generate().unwrap();
        let wrapped = vault_key
            .wrap_note_key(V, note(1), &NoteKey::generate().unwrap())
            .unwrap();

        let other_note = vault_key.unwrap_note_key(V, note(2), &wrapped);
        let other_vault = vault_key.unwrap_note_key(vault(2), note(1), &wrapped);
        let other_key = VaultKey::generate()
            .unwrap()
            .unwrap_note_key(V, note(1), &wrapped);
        for result in [other_note, other_vault, other_key] {
            assert!(matches!(result, Err(CryptoError::Decrypt)));
        }
    }

    #[test]
    fn wrapped_note_key_is_not_note_data() {
        let vault_key = VaultKey::generate().unwrap();
        let note_key = NoteKey::generate().unwrap();
        let wrapped = vault_key.wrap_note_key(V, note(1), &note_key).unwrap();
        // Same key, same note, but a data kind: the kind byte must differ.
        let as_vault_note_key = NoteKey(SymmetricKey::from_bytes(*vault_key.0.as_bytes()));
        let result = as_vault_note_key.decrypt(V, note(1), PayloadKind::NotePayload, &wrapped);
        assert_eq!(result, Err(CryptoError::Decrypt));
    }

    #[test]
    fn debug_output_is_redacted() {
        let vault_key = VaultKey::generate().unwrap();
        let note_key = NoteKey::generate().unwrap();
        assert_eq!(format!("{vault_key:?}"), "VaultKey(..)");
        assert_eq!(format!("{note_key:?}"), "NoteKey(..)");
    }
}
