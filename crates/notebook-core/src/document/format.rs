//! Plaintext format and encryption of updates and snapshots.

use std::fmt;

use zeroize::Zeroizing;

use crate::crypto::{CryptoError, NoteKey, PayloadKind, VaultId};
use crate::note::NoteId;

/// Document format 1: a Yrs/Yjs update in v1 encoding.
pub const FORMAT_V1: u8 = 1;

/// Errors from reading or changing a note document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentError {
    /// Encryption or decryption failed, including ciphertexts for another
    /// note, vault, or kind, and tampered data.
    Crypto(CryptoError),
    /// The plaintext declares a document format this build doesn't know.
    UnsupportedFormat(u8),
    /// The plaintext is not a valid document update.
    Malformed,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crypto(err) => err.fmt(f),
            Self::UnsupportedFormat(v) => write!(f, "unsupported document format {v}"),
            Self::Malformed => f.write_str("malformed document update"),
        }
    }
}

impl std::error::Error for DocumentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Crypto(err) => Some(err),
            _ => None,
        }
    }
}

impl From<CryptoError> for DocumentError {
    fn from(err: CryptoError) -> Self {
        Self::Crypto(err)
    }
}

/// An encrypted CRDT update, ready for storage or sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedUpdate(Vec<u8>);

impl EncryptedUpdate {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// An encrypted snapshot of a document's full state, ready for storage or
/// sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedSnapshot(Vec<u8>);

impl EncryptedSnapshot {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// Prefixes a format-1 payload with its format byte.
pub(crate) fn encode_plaintext(payload: &[u8]) -> Zeroizing<Vec<u8>> {
    let mut plaintext = Zeroizing::new(Vec::with_capacity(payload.len() + 1));
    plaintext.push(FORMAT_V1);
    plaintext.extend_from_slice(payload);
    plaintext
}

/// Checks the format byte and returns the format-1 payload after it.
pub(crate) fn decode_plaintext(plaintext: &[u8]) -> Result<&[u8], DocumentError> {
    match plaintext.split_first() {
        None => Err(DocumentError::Malformed),
        Some((&FORMAT_V1, payload)) => Ok(payload),
        Some((&format, _)) => Err(DocumentError::UnsupportedFormat(format)),
    }
}

pub(crate) fn encrypt_update(
    key: &NoteKey,
    vault: VaultId,
    note: NoteId,
    payload: &[u8],
) -> Result<EncryptedUpdate, DocumentError> {
    let plaintext = encode_plaintext(payload);
    Ok(EncryptedUpdate(key.encrypt(
        vault,
        note,
        PayloadKind::CrdtUpdate,
        &plaintext,
    )?))
}

pub(crate) fn encrypt_snapshot(
    key: &NoteKey,
    vault: VaultId,
    note: NoteId,
    payload: &[u8],
) -> Result<EncryptedSnapshot, DocumentError> {
    let plaintext = encode_plaintext(payload);
    Ok(EncryptedSnapshot(key.encrypt(
        vault,
        note,
        PayloadKind::CrdtSnapshot,
        &plaintext,
    )?))
}

/// Decrypts `ciphertext` as `kind` data of `note`, returning the plaintext
/// with its format byte still in place.
pub(crate) fn decrypt(
    key: &NoteKey,
    vault: VaultId,
    note: NoteId,
    kind: PayloadKind,
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, DocumentError> {
    Ok(key.decrypt(vault, note, kind, ciphertext)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (VaultId, NoteId) {
        (VaultId::from_parts(1_000, 1), NoteId::from_parts(1_000, 2))
    }

    #[test]
    fn plaintext_round_trips() {
        let plaintext = encode_plaintext(b"payload");
        assert_eq!(plaintext[0], FORMAT_V1);
        assert_eq!(decode_plaintext(&plaintext), Ok(&b"payload"[..]));
        assert_eq!(decode_plaintext(&[FORMAT_V1]), Ok(&b""[..]));
    }

    #[test]
    fn empty_plaintext_is_malformed() {
        assert_eq!(decode_plaintext(&[]), Err(DocumentError::Malformed));
    }

    #[test]
    fn unknown_format_is_unsupported() {
        for format in [0, 2, 255] {
            assert_eq!(
                decode_plaintext(&[format, 1, 2, 3]),
                Err(DocumentError::UnsupportedFormat(format))
            );
        }
    }

    #[test]
    fn error_messages() {
        let cases = [
            (
                DocumentError::Crypto(CryptoError::Decrypt),
                "decryption failed",
            ),
            (
                DocumentError::UnsupportedFormat(7),
                "unsupported document format 7",
            ),
            (DocumentError::Malformed, "malformed document update"),
        ];
        for (err, message) in cases {
            assert_eq!(err.to_string(), message);
            let _: &dyn std::error::Error = &err;
        }
        assert!(std::error::Error::source(&DocumentError::Crypto(CryptoError::Decrypt)).is_some());
    }

    #[test]
    fn update_round_trips() {
        let (vault, note) = ids();
        let key = NoteKey::generate().unwrap();
        let update = encrypt_update(&key, vault, note, b"edit").unwrap();
        let plaintext = decrypt(
            &key,
            vault,
            note,
            PayloadKind::CrdtUpdate,
            update.as_bytes(),
        )
        .unwrap();
        assert_eq!(decode_plaintext(&plaintext), Ok(&b"edit"[..]));
        assert_eq!(update.clone().into_bytes(), update.as_bytes());
    }

    #[test]
    fn snapshot_round_trips() {
        let (vault, note) = ids();
        let key = NoteKey::generate().unwrap();
        let snapshot = encrypt_snapshot(&key, vault, note, b"state").unwrap();
        let plaintext = decrypt(
            &key,
            vault,
            note,
            PayloadKind::CrdtSnapshot,
            snapshot.as_bytes(),
        )
        .unwrap();
        assert_eq!(decode_plaintext(&plaintext), Ok(&b"state"[..]));
        assert_eq!(snapshot.clone().into_bytes(), snapshot.as_bytes());
    }

    #[test]
    fn update_does_not_decrypt_as_snapshot() {
        let (vault, note) = ids();
        let key = NoteKey::generate().unwrap();
        let update = encrypt_update(&key, vault, note, b"edit").unwrap();
        assert_eq!(
            decrypt(
                &key,
                vault,
                note,
                PayloadKind::CrdtSnapshot,
                update.as_bytes()
            ),
            Err(DocumentError::Crypto(CryptoError::Decrypt))
        );
    }

    #[test]
    fn snapshot_does_not_decrypt_as_update() {
        let (vault, note) = ids();
        let key = NoteKey::generate().unwrap();
        let snapshot = encrypt_snapshot(&key, vault, note, b"state").unwrap();
        assert_eq!(
            decrypt(
                &key,
                vault,
                note,
                PayloadKind::CrdtUpdate,
                snapshot.as_bytes()
            ),
            Err(DocumentError::Crypto(CryptoError::Decrypt))
        );
    }
}
