//! The recovery key: a human-transcribable code that unlocks the vault on its
//! own.
//!
//! It is 20 bytes: 16 random bytes plus a 4-byte checksum (the start of
//! `SHA-256("notebook/v1/recovery" || random)`). That is exactly 32 Crockford
//! base32 characters, shown as 8 hyphen-separated groups of 4.

use std::fmt;

use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use super::{CryptoError, VaultId, fill_random};
use super::{keys::SymmetricKey, secret::recovery_wrapping_key};

const RANDOM_LEN: usize = 16;
const CHECKSUM_LEN: usize = 4;
const ENCODED_LEN: usize = 32;
const GROUP_LEN: usize = 4;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CHECKSUM_DOMAIN: &[u8] = b"notebook/v1/recovery";
const INFO_VERIFIER: &[u8] = b"notebook/v1/recovery-verifier";

/// Length of a [`RecoveryKey::verifier`] in bytes.
pub const RECOVERY_VERIFIER_LEN: usize = 32;

/// A recovery key generated at vault creation.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RecoveryKey([u8; RANDOM_LEN]);

impl RecoveryKey {
    /// Generates a new random recovery key.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut key = Self([0; RANDOM_LEN]);
        fill_random(&mut key.0)?;
        Ok(key)
    }

    /// The key as the user should write it down, e.g.
    /// `7Q2M-K9XD-...` (8 groups of 4).
    pub fn to_display(&self) -> Zeroizing<String> {
        let mut bytes = Zeroizing::new([0u8; RANDOM_LEN + CHECKSUM_LEN]);
        bytes[..RANDOM_LEN].copy_from_slice(&self.0);
        bytes[RANDOM_LEN..].copy_from_slice(&checksum(&self.0));

        let mut out = Zeroizing::new(String::with_capacity(ENCODED_LEN + ENCODED_LEN / GROUP_LEN));
        for i in 0..ENCODED_LEN {
            if i > 0 && i % GROUP_LEN == 0 {
                out.push('-');
            }
            out.push(ALPHABET[read_5_bits(&*bytes, i) as usize] as char);
        }
        out
    }

    /// Parses a recovery key as typed by a person. Case, spaces, and hyphens
    /// are ignored, and `I`/`L` read as `1` and `O` as `0`. The wrong length,
    /// an unknown character, or a checksum mismatch is
    /// [`CryptoError::InvalidRecoveryKey`].
    pub fn parse(text: &str) -> Result<Self, CryptoError> {
        let mut values = Zeroizing::new([0u8; ENCODED_LEN]);
        let mut n = 0;
        for c in text.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            if n == ENCODED_LEN {
                return Err(CryptoError::InvalidRecoveryKey);
            }
            values[n] = decode_char(c).ok_or(CryptoError::InvalidRecoveryKey)?;
            n += 1;
        }
        if n != ENCODED_LEN {
            return Err(CryptoError::InvalidRecoveryKey);
        }

        let mut bytes = Zeroizing::new([0u8; RANDOM_LEN + CHECKSUM_LEN]);
        for (i, &v) in values.iter().enumerate() {
            write_5_bits(&mut *bytes, i, v);
        }
        let key = Self(bytes[..RANDOM_LEN].try_into().expect("16 bytes"));
        if checksum(&key.0) != bytes[RANDOM_LEN..] {
            return Err(CryptoError::InvalidRecoveryKey);
        }
        Ok(key)
    }

    pub(crate) fn wrapping_key(&self, vault: VaultId) -> SymmetricKey {
        recovery_wrapping_key(&self.0, vault)
    }

    /// A value the server stores (hashed) to check that a client holds this
    /// recovery key. It is an HKDF-SHA256 output under its own info string,
    /// independent of the wrapping key, so it can't unwrap anything. No salt:
    /// the 16 random bytes are unique, and a recovering client doesn't know
    /// its vault ID yet.
    pub fn verifier(&self) -> [u8; RECOVERY_VERIFIER_LEN] {
        let mut out = [0u8; RECOVERY_VERIFIER_LEN];
        Hkdf::<Sha256>::new(None, &self.0)
            .expand(INFO_VERIFIER, &mut out)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        out
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryKey(..)")
    }
}

fn checksum(random: &[u8; RANDOM_LEN]) -> [u8; CHECKSUM_LEN] {
    let digest = Sha256::new()
        .chain_update(CHECKSUM_DOMAIN)
        .chain_update(random)
        .finalize();
    digest[..CHECKSUM_LEN].try_into().expect("4 bytes")
}

fn decode_char(c: char) -> Option<u8> {
    let c = match c.to_ascii_uppercase() {
        'I' | 'L' => '1',
        'O' => '0',
        c => c,
    };
    ALPHABET
        .iter()
        .position(|&a| a as char == c)
        .map(|i| i as u8)
}

/// Reads the `index`-th 5-bit group, most significant bit first.
fn read_5_bits(bytes: &[u8], index: usize) -> u8 {
    (0..5).fold(0, |acc, b| {
        let bit = index * 5 + b;
        (acc << 1) | ((bytes[bit / 8] >> (7 - bit % 8)) & 1)
    })
}

fn write_5_bits(bytes: &mut [u8], index: usize, value: u8) {
    for b in 0..5 {
        let bit = index * 5 + b;
        if (value >> (4 - b)) & 1 == 1 {
            bytes[bit / 8] |= 1 << (7 - bit % 8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_format() {
        let shown = RecoveryKey::generate().unwrap().to_display();
        let groups: Vec<&str> = shown.split('-').collect();
        assert_eq!(groups.len(), 8, "{}", *shown);
        for group in groups {
            assert_eq!(group.len(), 4);
            assert!(group.bytes().all(|b| ALPHABET.contains(&b)));
        }
    }

    #[test]
    fn generated_keys_differ() {
        let a = RecoveryKey::generate().unwrap().to_display();
        let b = RecoveryKey::generate().unwrap().to_display();
        assert_ne!(*a, *b);
    }

    #[test]
    fn display_round_trips() {
        let key = RecoveryKey::generate().unwrap();
        let parsed = RecoveryKey::parse(&key.to_display()).unwrap();
        assert_eq!(parsed.0, key.0);
    }

    #[test]
    fn known_encoding() {
        let key = RecoveryKey([0; RANDOM_LEN]);
        let shown = key.to_display();
        assert!(shown.starts_with("0000-0000-0000-0000-0000-0000-"));
        assert_eq!(RecoveryKey::parse(&shown).unwrap().0, [0; RANDOM_LEN]);
    }

    #[test]
    fn sloppy_but_correct_entry_parses() {
        let key = RecoveryKey([0; RANDOM_LEN]);
        let shown = key.to_display();
        let sloppy = shown.to_lowercase().replace('-', " ").replace('0', "o");
        assert_eq!(RecoveryKey::parse(&sloppy).unwrap().0, key.0);

        let key = RecoveryKey::generate().unwrap();
        let sloppy = format!("  {}  ", key.to_display().replace('1', "l").to_lowercase());
        assert_eq!(RecoveryKey::parse(&sloppy).unwrap().0, key.0);
    }

    #[test]
    fn every_single_character_typo_is_detected() {
        let shown = RecoveryKey::generate().unwrap().to_display();
        for (i, original) in shown.char_indices() {
            if original == '-' {
                continue;
            }
            for &replacement in ALPHABET {
                let replacement = replacement as char;
                if replacement == original {
                    continue;
                }
                let mut typo = shown.to_string();
                typo.replace_range(i..i + 1, &replacement.to_string());
                assert_eq!(
                    RecoveryKey::parse(&typo).unwrap_err(),
                    CryptoError::InvalidRecoveryKey,
                    "{typo}"
                );
            }
        }
    }

    #[test]
    fn wrong_length_and_bad_characters_are_rejected() {
        let shown = RecoveryKey::generate().unwrap().to_display();
        let short = &shown[..shown.len() - 1];
        let long = format!("{}0", *shown);
        let bad_char = shown.replacen(|c: char| c.is_ascii_alphanumeric(), "U", 1);
        for text in ["", short, &long, &bad_char] {
            assert_eq!(
                RecoveryKey::parse(text).unwrap_err(),
                CryptoError::InvalidRecoveryKey,
                "{text:?}"
            );
        }
    }

    #[test]
    fn debug_output_is_redacted() {
        let key = RecoveryKey::generate().unwrap();
        assert_eq!(format!("{key:?}"), "RecoveryKey(..)");
    }

    /// Cross-checked against an independent Python implementation of the
    /// encoding and checksum.
    #[test]
    fn fixed_encodings() {
        let cases = [
            ([0u8; RANDOM_LEN], "0000-0000-0000-0000-0000-0000-00DC-5W25"),
            (
                std::array::from_fn(|i| i as u8),
                "000G-40R4-0M30-E209-185G-R38E-1YSS-08BQ",
            ),
        ];
        for (random, text) in cases {
            assert_eq!(*RecoveryKey(random).to_display(), text);
            assert_eq!(RecoveryKey::parse(text).unwrap().0, random);
        }
    }

    #[test]
    fn display_is_uppercase_and_parse_accepts_any_grouping() {
        let key = RecoveryKey::generate().unwrap();
        let shown = key.to_display();
        assert_eq!(*shown, shown.to_uppercase());
        let compact: String = shown.chars().filter(|&c| c != '-').collect();
        assert_eq!(RecoveryKey::parse(&compact).unwrap().0, key.0);
        let spread: String = compact.chars().flat_map(|c| [c, '-', '\t']).collect();
        assert_eq!(RecoveryKey::parse(&spread).unwrap().0, key.0);
    }

    #[test]
    fn lookalike_letters_decode_as_digits() {
        for (c, digit) in [('I', 1), ('i', 1), ('L', 1), ('l', 1), ('O', 0), ('o', 0)] {
            assert_eq!(decode_char(c), Some(digit), "{c}");
        }
        for (i, &c) in ALPHABET.iter().enumerate() {
            assert_eq!(decode_char(c as char), Some(i as u8));
            assert_eq!(decode_char(c.to_ascii_lowercase() as char), Some(i as u8));
        }
        for c in ['U', 'u', '_', '+', 'é', '０'] {
            assert_eq!(decode_char(c), None, "{c}");
        }
    }

    #[test]
    fn five_bit_groups_round_trip() {
        let mut bytes = [0u8; RANDOM_LEN + CHECKSUM_LEN];
        for i in 0..ENCODED_LEN {
            write_5_bits(&mut bytes, i, (i as u8 * 7) % 32);
        }
        for i in 0..ENCODED_LEN {
            assert_eq!(read_5_bits(&bytes, i), (i as u8 * 7) % 32);
        }
        assert_eq!(read_5_bits(&[0b1010_1100, 0], 0), 0b10101);
        assert_eq!(read_5_bits(&[0b1010_1100, 0b0100_0000], 1), 0b10001);
    }

    #[test]
    fn checksum_is_domain_separated_sha256_prefix() {
        let random = [0u8; RANDOM_LEN];
        let digest = Sha256::digest([CHECKSUM_DOMAIN, &random].concat());
        assert_eq!(checksum(&random), digest[..CHECKSUM_LEN]);
        assert_ne!(
            checksum(&random)[..],
            Sha256::digest(random)[..CHECKSUM_LEN]
        );
    }

    #[test]
    fn swapped_characters_are_detected() {
        let shown = RecoveryKey::generate().unwrap().to_display();
        let chars: Vec<char> = shown.chars().filter(|&c| c != '-').collect();
        for i in 0..chars.len() - 1 {
            if chars[i] == chars[i + 1] {
                continue;
            }
            let mut swapped = chars.clone();
            swapped.swap(i, i + 1);
            let text: String = swapped.into_iter().collect();
            assert_eq!(
                RecoveryKey::parse(&text).unwrap_err(),
                CryptoError::InvalidRecoveryKey,
                "{text}"
            );
        }
    }

    #[test]
    fn non_ascii_input_is_rejected() {
        let shown = RecoveryKey::generate().unwrap().to_display();
        let text = shown.replacen('-', "é", 1);
        assert_eq!(
            RecoveryKey::parse(&text).unwrap_err(),
            CryptoError::InvalidRecoveryKey
        );
    }

    #[test]
    fn wrapping_key_depends_on_key_and_vault() {
        let v1 = VaultId::from_parts(1_000, 1);
        let v2 = VaultId::from_parts(1_000, 2);
        let a = RecoveryKey([1; RANDOM_LEN]);
        let b = RecoveryKey([2; RANDOM_LEN]);
        assert_eq!(a.wrapping_key(v1).as_bytes(), a.wrapping_key(v1).as_bytes());
        assert_ne!(a.wrapping_key(v1).as_bytes(), a.wrapping_key(v2).as_bytes());
        assert_ne!(a.wrapping_key(v1).as_bytes(), b.wrapping_key(v1).as_bytes());
    }

    #[test]
    fn verifier_is_deterministic_across_display_round_trip() {
        let key = RecoveryKey::generate().unwrap();
        let typed = key.to_display().to_lowercase();
        assert_eq!(
            RecoveryKey::parse(&typed).unwrap().verifier(),
            key.verifier()
        );
    }

    #[test]
    fn different_keys_give_different_verifiers() {
        let a = RecoveryKey::generate().unwrap();
        let b = RecoveryKey::generate().unwrap();
        assert_ne!(a.verifier(), b.verifier());
    }

    #[test]
    fn verifier_does_not_unlock_the_bundle() {
        use crate::crypto::{AuthSecret, KeyBundle, VaultKey};
        let vault = VaultId::from_parts(1_000, 1);
        let recovery = RecoveryKey::generate().unwrap();
        let bundle = KeyBundle::create(
            vault,
            &VaultKey::generate().unwrap(),
            &AuthSecret::from_bytes(&[1; 32]).unwrap(),
            &recovery,
        )
        .unwrap();
        let as_auth = AuthSecret::from_bytes(&recovery.verifier()).unwrap();
        assert_eq!(
            bundle.unlock_with_auth(&as_auth).unwrap_err(),
            CryptoError::Decrypt
        );
        assert_ne!(
            recovery.verifier(),
            *recovery.wrapping_key(vault).as_bytes()
        );
    }

    /// Cross-checked against an independent HKDF-SHA256 implementation.
    #[test]
    fn fixed_verifier_vectors() {
        let hex = |b: [u8; 32]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(RecoveryKey(std::array::from_fn(|i| i as u8)).verifier()),
            "94c96491f38f126d3d23d55a95048278d812cefb06ff6cfce27213a22d17a851"
        );
        assert_eq!(
            hex(RecoveryKey([0; RANDOM_LEN]).verifier()),
            "e3b6871a9999794f21926e3e71ab7e7998d414fa7cfe16e3c4d6bab30cd065b2"
        );
    }

    #[test]
    fn parsed_key_unlocks_the_bundle() {
        use crate::crypto::{AuthSecret, KeyBundle, VaultKey};
        let vault = VaultId::from_parts(1_000, 1);
        let recovery = RecoveryKey::generate().unwrap();
        let bundle = KeyBundle::create(
            vault,
            &VaultKey::generate().unwrap(),
            &AuthSecret::from_bytes(&[1; 32]).unwrap(),
            &recovery,
        )
        .unwrap();
        let typed = recovery.to_display().to_lowercase().replace('-', " ");
        let parsed = RecoveryKey::parse(&typed).unwrap();
        assert!(bundle.unlock_with_recovery(&parsed).is_ok());
    }
}
