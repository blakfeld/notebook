//! The versioned ciphertext envelope shared by every encrypted value.
//!
//! See the [module docs](super) for the byte layout. The associated data is
//! `"notebook/v1/aad" || version || algorithm || vault_id || kind || note_id`,
//! all fixed-length, with an all-zero note ID for vault-level kinds.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use super::{CryptoError, VaultId, fill_random};
use crate::note::NoteId;

/// The current envelope format version.
pub const ENVELOPE_VERSION: u8 = 1;
/// Algorithm identifier for XChaCha20-Poly1305.
pub const ALGORITHM_XCHACHA20POLY1305: u8 = 1;

const HEADER_LEN: usize = 2;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
/// Bytes an envelope adds to the plaintext: header, nonce, and tag.
pub const ENVELOPE_OVERHEAD: usize = HEADER_LEN + NONCE_LEN + TAG_LEN;

const AAD_DOMAIN: &[u8] = b"notebook/v1/aad";

/// What a ciphertext is and where it belongs. All of it is authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Context {
    pub vault: VaultId,
    pub kind: u8,
    pub note: Option<NoteId>,
}

impl Context {
    fn aad(&self, version: u8, algorithm: u8) -> Vec<u8> {
        let mut aad = Vec::with_capacity(AAD_DOMAIN.len() + 2 + 16 + 1 + 16);
        aad.extend_from_slice(AAD_DOMAIN);
        aad.push(version);
        aad.push(algorithm);
        aad.extend_from_slice(&self.vault.to_bytes());
        aad.push(self.kind);
        aad.extend_from_slice(&self.note.map_or([0; 16], NoteId::to_bytes));
        aad
    }
}

/// Encrypts `plaintext` under `key` with a fresh random nonce.
pub(crate) fn seal(
    key: &[u8; 32],
    ctx: &Context,
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce)?;
    seal_with_nonce(key, ctx, &nonce, plaintext)
}

/// Encrypts with a caller-chosen nonce. Only [`seal`] and test vectors may
/// call this: reusing a nonce under one key breaks authenticity.
pub(crate) fn seal_with_nonce(
    key: &[u8; 32],
    ctx: &Context,
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let aad = ctx.aad(ENVELOPE_VERSION, ALGORITHM_XCHACHA20POLY1305);
    let sealed = XChaCha20Poly1305::new(key.into())
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Malformed)?;

    let mut out = Vec::with_capacity(HEADER_LEN + NONCE_LEN + sealed.len());
    out.push(ENVELOPE_VERSION);
    out.push(ALGORITHM_XCHACHA20POLY1305);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Checks the header and decrypts an envelope produced by [`seal`].
pub(crate) fn open(
    key: &[u8; 32],
    ctx: &Context,
    envelope: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if envelope.len() < ENVELOPE_OVERHEAD {
        return Err(CryptoError::Malformed);
    }
    let (version, algorithm) = (envelope[0], envelope[1]);
    if version != ENVELOPE_VERSION {
        return Err(CryptoError::UnsupportedVersion(version));
    }
    if algorithm != ALGORITHM_XCHACHA20POLY1305 {
        return Err(CryptoError::UnsupportedAlgorithm(algorithm));
    }
    let (nonce, sealed) = envelope[HEADER_LEN..].split_at(NONCE_LEN);
    let nonce = XNonce::try_from(nonce).map_err(|_| CryptoError::Malformed)?;
    let aad = ctx.aad(version, algorithm);
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            &nonce,
            Payload {
                msg: sealed,
                aad: &aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7; 32];

    fn ctx() -> Context {
        Context {
            vault: VaultId::from_parts(1_000, 1),
            kind: 2,
            note: Some(NoteId::from_parts(1_000, 2)),
        }
    }

    #[test]
    fn round_trip() {
        for plaintext in [&b""[..], b"hello", &[0xff; 1000]] {
            let sealed = seal(&KEY, &ctx(), plaintext).unwrap();
            assert_eq!(&*open(&KEY, &ctx(), &sealed).unwrap(), plaintext);
        }
    }

    #[test]
    fn header_layout_and_overhead() {
        let sealed = seal(&KEY, &ctx(), b"hello").unwrap();
        assert_eq!(sealed[0], 1);
        assert_eq!(sealed[1], 1);
        assert_eq!(sealed.len(), 5 + 42);
        assert_eq!(ENVELOPE_OVERHEAD, 42);
    }

    #[test]
    fn same_plaintext_encrypts_differently() {
        let a = seal(&KEY, &ctx(), b"hello").unwrap();
        let b = seal(&KEY, &ctx(), b"hello").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn unknown_version_and_algorithm_are_reported() {
        let sealed = seal(&KEY, &ctx(), b"hello").unwrap();

        let mut bad = sealed.clone();
        bad[0] = 9;
        assert_eq!(
            open(&KEY, &ctx(), &bad),
            Err(CryptoError::UnsupportedVersion(9))
        );

        let mut bad = sealed;
        bad[1] = 9;
        assert_eq!(
            open(&KEY, &ctx(), &bad),
            Err(CryptoError::UnsupportedAlgorithm(9))
        );
    }

    #[test]
    fn every_flipped_byte_is_detected() {
        let sealed = seal(&KEY, &ctx(), b"hello world").unwrap();
        for i in 0..sealed.len() {
            let mut bad = sealed.clone();
            bad[i] ^= 0x01;
            assert!(open(&KEY, &ctx(), &bad).is_err(), "byte {i}");
        }
    }

    #[test]
    fn every_truncation_is_rejected() {
        let sealed = seal(&KEY, &ctx(), b"hello world").unwrap();
        for len in 0..sealed.len() {
            assert!(open(&KEY, &ctx(), &sealed[..len]).is_err(), "length {len}");
        }
    }

    #[test]
    fn every_context_field_is_bound() {
        let sealed = seal(&KEY, &ctx(), b"hello").unwrap();
        let others = [
            Context {
                vault: VaultId::from_parts(1_000, 9),
                ..ctx()
            },
            Context { kind: 3, ..ctx() },
            Context {
                note: Some(NoteId::from_parts(1_000, 9)),
                ..ctx()
            },
            Context {
                note: None,
                ..ctx()
            },
        ];
        for other in others {
            assert_eq!(open(&KEY, &other, &sealed), Err(CryptoError::Decrypt));
        }
    }

    #[test]
    fn wrong_key_is_a_generic_decrypt_error() {
        let sealed = seal(&KEY, &ctx(), b"hello").unwrap();
        assert_eq!(open(&[8; 32], &ctx(), &sealed), Err(CryptoError::Decrypt));
    }

    #[test]
    fn short_input_is_malformed() {
        for len in [0, 1, 2, ENVELOPE_OVERHEAD - 1] {
            let mut bytes = vec![0u8; len];
            if len >= 2 {
                bytes[0] = ENVELOPE_VERSION;
                bytes[1] = ALGORITHM_XCHACHA20POLY1305;
            }
            assert_eq!(
                open(&KEY, &ctx(), &bytes),
                Err(CryptoError::Malformed),
                "{len}"
            );
        }
    }

    #[test]
    fn minimum_length_envelope_is_an_empty_plaintext() {
        let sealed = seal(&KEY, &ctx(), b"").unwrap();
        assert_eq!(sealed.len(), ENVELOPE_OVERHEAD);
        assert!(open(&KEY, &ctx(), &sealed).unwrap().is_empty());

        let mut forged = sealed;
        forged[ENVELOPE_OVERHEAD - 1] ^= 1;
        assert_eq!(open(&KEY, &ctx(), &forged), Err(CryptoError::Decrypt));
    }

    #[test]
    fn version_is_checked_before_algorithm() {
        let mut bad = seal(&KEY, &ctx(), b"x").unwrap();
        bad[0] = 0;
        bad[1] = 0;
        assert_eq!(
            open(&KEY, &ctx(), &bad),
            Err(CryptoError::UnsupportedVersion(0))
        );
    }

    #[test]
    fn nonce_is_stored_after_the_header() {
        let nonce = [0x5a; NONCE_LEN];
        let sealed = seal_with_nonce(&KEY, &ctx(), &nonce, b"x").unwrap();
        assert_eq!(&sealed[HEADER_LEN..HEADER_LEN + NONCE_LEN], &nonce);
    }

    #[test]
    fn same_nonce_and_context_is_deterministic() {
        let nonce = [1; NONCE_LEN];
        let a = seal_with_nonce(&KEY, &ctx(), &nonce, b"x").unwrap();
        let b = seal_with_nonce(&KEY, &ctx(), &nonce, b"x").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn aad_layout() {
        let ctx = Context {
            vault: VaultId::from_bytes([0x11; 16]),
            kind: 0x22,
            note: Some(NoteId::from_parts(
                0x0102_0304_0506,
                0x0708_090a_0b0c_0d0e_0f10,
            )),
        };
        let mut expected = b"notebook/v1/aad".to_vec();
        expected.extend([1, 1]);
        expected.extend([0x11; 16]);
        expected.push(0x22);
        expected.extend(1..=16u8);
        assert_eq!(ctx.aad(1, 1), expected);

        let vault_level = Context { note: None, ..ctx };
        assert_eq!(&vault_level.aad(1, 1)[expected.len() - 16..], &[0; 16]);
        assert_eq!(vault_level.aad(1, 1).len(), expected.len());
    }

    #[test]
    fn large_plaintext_round_trips() {
        let plaintext: Vec<u8> = (0..1 << 20).map(|i| (i % 251) as u8).collect();
        let sealed = seal(&KEY, &ctx(), &plaintext).unwrap();
        assert_eq!(sealed.len(), plaintext.len() + ENVELOPE_OVERHEAD);
        assert_eq!(*open(&KEY, &ctx(), &sealed).unwrap(), plaintext);
    }

    /// Pins the exact bytes so every target and dependency version agrees.
    #[test]
    fn fixed_test_vector() {
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let nonce: [u8; 24] = std::array::from_fn(|i| 0x40 + i as u8);
        let ctx = Context {
            vault: VaultId::from_bytes([0x11; 16]),
            kind: 1,
            note: Some(NoteId::from_parts(0, 0)),
        };
        let sealed = seal_with_nonce(&key, &ctx, &nonce, b"notebook").unwrap();
        assert_eq!(hex(&sealed), TEST_VECTOR);
        assert_eq!(&*open(&key, &ctx, &sealed).unwrap(), b"notebook");
    }

    /// Cross-checked against libsodium's
    /// `crypto_aead_xchacha20poly1305_ietf_encrypt` with the same AAD.
    const TEST_VECTOR: &str = "0101404142434445464748494a4b4c4d4e4f5051525354555657\
                               ba567115b28f167dbead05ebefb592976cd600860196d909";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
