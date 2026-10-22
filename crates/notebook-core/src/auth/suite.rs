//! The OPAQUE cipher suite shared by client and server, and the random source
//! handed to `opaque-ke`.

use opaque_ke::errors::InternalError;
use opaque_ke::generic_array::{ArrayLength, GenericArray};
use opaque_ke::ksf::Ksf;
use rand_core::{CryptoRng, RngCore};

use crate::crypto::{fill_random, stretch};

/// ristretto255 for the OPRF and key exchange, TripleDH with SHA-512, and
/// Argon2id key stretching with the vault-crypto V1 parameters.
pub struct Suite;

impl opaque_ke::CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, sha2_010::Sha512>;
    type Ksf = Argon2Ksf;
}

/// Argon2id stretching through [`crate::crypto`]'s vetted parameters
/// (64 MiB, t = 3, p = 1). Runs on the client, once per OPAQUE finish.
#[derive(Debug, Default)]
pub struct Argon2Ksf;

impl Ksf for Argon2Ksf {
    fn hash<L: ArrayLength<u8>>(
        &self,
        input: GenericArray<u8, L>,
    ) -> Result<GenericArray<u8, L>, InternalError> {
        let mut out = GenericArray::default();
        stretch(&input, &mut out).map_err(|_| InternalError::KsfError)?;
        Ok(out)
    }
}

/// The platform's secure random source (`getrandom`), behind the `rand_core`
/// 0.6 traits that `opaque-ke` takes.
#[derive(Debug, Default, Clone, Copy)]
pub struct SecureRng;

impl RngCore for SecureRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    /// Panics if the platform's random source fails, like `rand`'s `OsRng`:
    /// `opaque-ke` has no way to report it, and continuing would be unsafe.
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        fill_random(dest).expect("secure random source failed");
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        fill_random(dest).map_err(|_| {
            rand_core::Error::from(
                core::num::NonZeroU32::new(rand_core::Error::CUSTOM_START).unwrap(),
            )
        })
    }
}

impl CryptoRng for SecureRng {}

#[cfg(test)]
pub(crate) mod tests {
    use opaque_ke::{
        ClientLogin, ClientLoginFinishParameters, ClientRegistration,
        ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
        CredentialResponse, RegistrationRequest, RegistrationResponse, RegistrationUpload,
        ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup,
    };
    use sha2::{Digest, Sha256};

    use super::*;

    /// A deterministic RNG for transcript tests: SHA-256 of a counter.
    pub(crate) struct TestRng(u64);

    impl TestRng {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed << 32)
        }
    }

    impl RngCore for TestRng {
        fn next_u32(&mut self) -> u32 {
            rand_core::impls::next_u32_via_fill(self)
        }
        fn next_u64(&mut self) -> u64 {
            rand_core::impls::next_u64_via_fill(self)
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            for chunk in dest.chunks_mut(32) {
                self.0 += 1;
                let block = Sha256::digest(self.0.to_le_bytes());
                chunk.copy_from_slice(&block[..chunk.len()]);
            }
        }
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
            self.fill_bytes(dest);
            Ok(())
        }
    }

    impl CryptoRng for TestRng {}

    struct Transcript {
        upload: Vec<u8>,
        response: Vec<u8>,
        finalization: Vec<u8>,
        export_key: Vec<u8>,
    }

    fn run(rng: &mut (impl RngCore + CryptoRng), password: &[u8]) -> Transcript {
        let setup = ServerSetup::<Suite>::new(rng);
        let id = b"alice";

        let start = ClientRegistration::<Suite>::start(rng, password).unwrap();
        let request = RegistrationRequest::deserialize(&start.message.serialize()).unwrap();
        let response = ServerRegistration::start(&setup, request, id).unwrap();
        let response = RegistrationResponse::deserialize(&response.message.serialize()).unwrap();
        let finish = start
            .state
            .finish(
                rng,
                password,
                response,
                ClientRegistrationFinishParameters::default(),
            )
            .unwrap();
        let upload = finish.message.serialize().to_vec();
        let record = ServerRegistration::finish(RegistrationUpload::deserialize(&upload).unwrap());

        let login = ClientLogin::<Suite>::start(rng, password).unwrap();
        let request = CredentialRequest::deserialize(&login.message.serialize()).unwrap();
        let server = ServerLogin::start(
            rng,
            &setup,
            Some(record),
            request,
            id,
            ServerLoginParameters::default(),
        )
        .unwrap();
        let response_bytes = server.message.serialize().to_vec();
        let client = login
            .state
            .finish(
                rng,
                password,
                CredentialResponse::deserialize(&response_bytes).unwrap(),
                ClientLoginFinishParameters::default(),
            )
            .unwrap();
        let finalization = client.message.serialize().to_vec();
        let done = server
            .state
            .finish(
                CredentialFinalization::deserialize(&finalization).unwrap(),
                ServerLoginParameters::default(),
            )
            .unwrap();
        assert_eq!(done.session_key, client.session_key);
        assert_eq!(client.export_key, finish.export_key);

        Transcript {
            upload,
            response: response_bytes,
            finalization,
            export_key: client.export_key.to_vec(),
        }
    }

    #[test]
    fn registration_and_login_complete_in_process() {
        let t = run(&mut SecureRng, b"correct horse battery");
        assert_eq!(t.export_key.len(), 64);
    }

    #[test]
    fn wrong_password_fails_login() {
        let mut rng = SecureRng;
        let setup = ServerSetup::<Suite>::new(&mut rng);
        let start = ClientRegistration::<Suite>::start(&mut rng, b"right").unwrap();
        let response = ServerRegistration::start(&setup, start.message, b"bob").unwrap();
        let finish = start
            .state
            .finish(
                &mut rng,
                b"right",
                response.message,
                ClientRegistrationFinishParameters::default(),
            )
            .unwrap();
        let record = ServerRegistration::finish(finish.message);

        let login = ClientLogin::<Suite>::start(&mut rng, b"wrong").unwrap();
        let server = ServerLogin::start(
            &mut rng,
            &setup,
            Some(record),
            login.message,
            b"bob",
            ServerLoginParameters::default(),
        )
        .unwrap();
        let result = login.state.finish(
            &mut rng,
            b"wrong",
            server.message,
            ClientLoginFinishParameters::default(),
        );
        assert!(matches!(
            result,
            Err(opaque_ke::errors::ProtocolError::InvalidLoginError)
        ));
    }

    /// A recorded transcript under a fixed RNG. If a dependency bump changes
    /// these bytes, stored OPAQUE records or the wire format changed with it:
    /// existing users could no longer log in.
    #[test]
    fn recorded_transcript() {
        let t = run(&mut TestRng::new(7), b"correct horse battery");
        let hash = |bytes: &[u8]| -> String {
            Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        assert_eq!(
            [
                hash(&t.upload),
                hash(&t.response),
                hash(&t.finalization),
                hash(&t.export_key),
            ],
            RECORDED
        );
    }

    const RECORDED: [&str; 4] = [
        "7bfda5da74513b1f6f675f21fadc79af72bfd0e5278af2e4e7b7af0e229cd54d",
        "509d930eeb4bb73666c33adefe0d921cfd9def0e887245e35e7f84a1db723589",
        "a04a0bdced4ada31044b460820b0b63f1d5127ec7eb2fb2f20837b7de6147aac",
        "b2febe8a9e26cba5000c7ecadf83136dc2798387291b07f9e474bd58b93f0a12",
    ];
}
