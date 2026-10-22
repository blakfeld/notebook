//! Argon2id derivation of an auth secret directly from a passphrase.

use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use super::{AuthSecret, CryptoError, fill_random};

const SALT_LEN: usize = 16;
const OUTPUT_LEN: usize = 32;

/// A numbered Argon2id parameter set. Only known versions can be named, so
/// stored parameters can't be downgraded to weaker settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KdfVersion {
    /// Argon2id, 64 MiB memory, 3 iterations, parallelism 1, 32-byte output.
    V1,
}

impl KdfVersion {
    /// The newest parameter set, for new derivations.
    pub const CURRENT: Self = Self::V1;

    pub fn to_u8(self) -> u8 {
        match self {
            Self::V1 => 1,
        }
    }

    /// Looks up a stored version number. Unknown numbers are
    /// [`CryptoError::UnsupportedKdfVersion`].
    pub fn from_u8(version: u8) -> Result<Self, CryptoError> {
        match version {
            1 => Ok(Self::V1),
            v => Err(CryptoError::UnsupportedKdfVersion(v)),
        }
    }

    fn params(self) -> Params {
        self.params_with_output(Some(OUTPUT_LEN))
    }

    fn params_with_output(self, output_len: Option<usize>) -> Params {
        match self {
            Self::V1 => Params::new(64 * 1024, 3, 1, output_len).expect("V1 parameters are valid"),
        }
    }
}

/// A random salt for passphrase derivation, stored next to whatever the
/// derived secret protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Salt([u8; SALT_LEN]);

impl Salt {
    /// Generates a new random 16-byte salt.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut salt = [0; SALT_LEN];
        fill_random(&mut salt)?;
        Ok(Self(salt))
    }

    pub fn from_bytes(bytes: [u8; SALT_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; SALT_LEN] {
        &self.0
    }
}

/// Derives an [`AuthSecret`] from a passphrase with Argon2id.
///
/// Normal login doesn't use this: the OPAQUE export key is already a strong
/// secret. It exists so any path that starts from a passphrase directly uses
/// vetted, versioned parameters.
pub fn derive_auth_secret(
    passphrase: &[u8],
    salt: &Salt,
    version: KdfVersion,
) -> Result<AuthSecret, CryptoError> {
    let mut out = Zeroizing::new([0u8; OUTPUT_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, version.params())
        .hash_password_into(passphrase, &salt.0, &mut *out)
        .map_err(|_| CryptoError::Kdf)?;
    AuthSecret::from_bytes(&*out)
}

/// Stretches `input` into `out` (any length Argon2 allows) with the
/// [`KdfVersion::CURRENT`] parameters and an all-zero salt. This is the OPAQUE
/// key-stretching step: its input is already a unique, per-user OPRF output,
/// so a salt adds nothing.
pub(crate) fn stretch(input: &[u8], out: &mut [u8]) -> Result<(), CryptoError> {
    Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        KdfVersion::CURRENT.params_with_output(None),
    )
    .hash_password_into(input, &[0; SALT_LEN], out)
    .map_err(|_| CryptoError::Kdf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::VaultId;

    const SALT_A: Salt = Salt([1; SALT_LEN]);
    const SALT_B: Salt = Salt([2; SALT_LEN]);

    fn derive(passphrase: &str, salt: &Salt) -> [u8; 32] {
        let secret = derive_auth_secret(passphrase.as_bytes(), salt, KdfVersion::V1).unwrap();
        // Compare through a derived key, since the secret's bytes are private.
        *secret.wrapping_key(VaultId::from_parts(0, 0)).as_bytes()
    }

    #[test]
    fn derivation_is_deterministic_and_input_sensitive() {
        let a = derive("correct horse", &SALT_A);
        assert_eq!(a, derive("correct horse", &SALT_A));
        assert_ne!(a, derive("correct horse", &SALT_B));
        assert_ne!(a, derive("battery staple", &SALT_A));
    }

    #[test]
    fn v1_parameters() {
        let params = KdfVersion::V1.params();
        assert_eq!(params.m_cost(), 65_536);
        assert_eq!(params.t_cost(), 3);
        assert_eq!(params.p_cost(), 1);
        assert_eq!(KdfVersion::CURRENT, KdfVersion::V1);
    }

    #[test]
    fn version_numbers() {
        assert_eq!(KdfVersion::from_u8(1), Ok(KdfVersion::V1));
        assert_eq!(KdfVersion::V1.to_u8(), 1);
        for v in [0, 2, 255] {
            assert_eq!(
                KdfVersion::from_u8(v),
                Err(CryptoError::UnsupportedKdfVersion(v))
            );
        }
    }

    #[test]
    fn generated_salts_differ() {
        assert_ne!(Salt::generate().unwrap(), Salt::generate().unwrap());
    }

    #[test]
    fn salt_bytes_round_trip() {
        let bytes: [u8; SALT_LEN] = std::array::from_fn(|i| i as u8);
        let salt = Salt::from_bytes(bytes);
        assert_eq!(salt.as_bytes(), &bytes);
        assert_eq!(Salt::from_bytes(*salt.as_bytes()), salt);
        assert_eq!(Salt::generate().unwrap().as_bytes().len(), 16);
    }

    #[test]
    fn current_version_round_trips() {
        let v = KdfVersion::CURRENT;
        assert_eq!(KdfVersion::from_u8(v.to_u8()), Ok(v));
        assert_eq!(v.params().output_len(), Some(OUTPUT_LEN));
    }

    #[test]
    fn stretch_supports_64_byte_output_and_matches_derivation_at_32() {
        let mut long = [0u8; 64];
        stretch(b"input", &mut long).unwrap();
        assert_ne!(long, [0; 64]);

        // With a 32-byte output and a zero salt, stretching is the same
        // Argon2id computation as `derive_auth_secret`.
        let mut short = [0u8; 32];
        stretch(b"input", &mut short).unwrap();
        let derived = derive_auth_secret(b"input", &Salt([0; SALT_LEN]), KdfVersion::V1).unwrap();
        let expected = AuthSecret::from_bytes(&short).unwrap();
        let vault = VaultId::from_parts(0, 0);
        assert_eq!(
            derived.wrapping_key(vault).as_bytes(),
            expected.wrapping_key(vault).as_bytes()
        );
    }

    #[test]
    fn empty_and_non_utf8_passphrases_derive() {
        derive_auth_secret(b"", &SALT_A, KdfVersion::V1).unwrap();
        derive_auth_secret(&[0xff, 0xfe, 0x00], &SALT_A, KdfVersion::V1).unwrap();
    }

    #[test]
    fn derived_secret_unlocks_a_bundle() {
        use crate::crypto::{KeyBundle, RecoveryKey, VaultKey};
        let vault = VaultId::from_parts(1_000, 1);
        let salt = Salt::generate().unwrap();
        let auth = derive_auth_secret(b"correct horse", &salt, KdfVersion::CURRENT).unwrap();
        let bundle = KeyBundle::create(
            vault,
            &VaultKey::generate().unwrap(),
            &auth,
            &RecoveryKey::generate().unwrap(),
        )
        .unwrap();

        let again = derive_auth_secret(b"correct horse", &salt, KdfVersion::CURRENT).unwrap();
        assert!(bundle.unlock_with_auth(&again).is_ok());
        let wrong = derive_auth_secret(b"wrong horse", &salt, KdfVersion::CURRENT).unwrap();
        assert!(matches!(
            bundle.unlock_with_auth(&wrong),
            Err(CryptoError::Decrypt)
        ));
    }
}
