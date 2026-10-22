//! Client side of every auth flow, as sans-I/O state machines: each `start`
//! returns the request to send, and `finish` takes the server's response and
//! returns the next request plus whatever the client keeps.

use std::fmt;

use opaque_ke::errors::ProtocolError;
use opaque_ke::{
    ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialResponse, RegistrationResponse,
};
use rand_core::RngCore;
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

use super::api::{
    LoginFinishRequest, LoginStartRequest, LoginStartResponse, PassphraseFinishRequest,
    PassphraseStartRequest, PassphraseStartResponse, RecoverFinishRequest, RecoverStartRequest,
    RecoverStartResponse, RegisterFinishRequest, RegisterStartRequest, RegisterStartResponse,
    SessionKind,
};
use super::suite::{SecureRng, Suite};
use super::{AuthError, Username};
use crate::crypto::{AuthSecret, KeyBundle, RecoveryKey, VaultId, VaultKey};

/// Minimum passphrase length, in Unicode scalar values after NFC
/// normalization, for registration, passphrase change, and recovery.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

/// How this device appears in the session list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub kind: SessionKind,
}

/// A new account's secrets, returned once by [`Registration::finish`]. Show
/// the recovery key to the user and then drop it.
#[derive(Debug)]
pub struct NewAccount {
    pub vault_id: VaultId,
    pub vault_key: VaultKey,
    pub recovery_key: RecoveryKey,
    pub keys: LoginKeys,
}

/// The OPAQUE export key from a login, as the [`AuthSecret`] that unwraps
/// the key bundle. Zeroized on drop.
pub struct LoginKeys {
    auth: AuthSecret,
}

impl LoginKeys {
    /// Unwraps the vault key from key bundle bytes the server returned.
    pub fn unlock(&self, key_bundle: &[u8]) -> Result<VaultKey, AuthError> {
        Ok(KeyBundle::from_bytes(key_bundle)?.unlock_with_auth(&self.auth)?)
    }
}

impl fmt::Debug for LoginKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LoginKeys(..)")
    }
}

/// Registration: OPAQUE registration plus a fresh vault, vault key, recovery
/// key, and key bundle, all generated here.
pub struct Registration {
    username: Username,
    passphrase: Zeroizing<String>,
    invite_code: String,
    state: ClientRegistration<Suite>,
}

impl Registration {
    /// Refuses an invalid username or a passphrase shorter than
    /// [`MIN_PASSPHRASE_CHARS`] before producing any message. `invite_code`
    /// is sent as typed in both requests; the server normalizes it.
    pub fn start(
        username: &str,
        passphrase: &str,
        invite_code: &str,
    ) -> Result<(Self, RegisterStartRequest), AuthError> {
        let username = Username::parse(username)?;
        let passphrase = new_passphrase(passphrase)?;
        let start = ClientRegistration::<Suite>::start(&mut SecureRng, passphrase.as_bytes())
            .map_err(|_| AuthError::Protocol)?;
        let request = RegisterStartRequest {
            username: username.to_string(),
            registration_request: start.message.serialize().to_vec(),
            invite_code: invite_code.to_owned(),
        };
        Ok((
            Self {
                username,
                passphrase,
                invite_code: invite_code.to_owned(),
                state: start.state,
            },
            request,
        ))
    }

    /// `now_ms` (Unix milliseconds) dates the new vault's ID.
    pub fn finish(
        self,
        response: &RegisterStartResponse,
        device: Device,
        now_ms: u64,
    ) -> Result<(RegisterFinishRequest, NewAccount), AuthError> {
        let (upload, auth) = finish_registration(
            self.state,
            &self.passphrase,
            &response.registration_response,
        )?;
        let mut random = [0u8; 16];
        SecureRng.fill_bytes(&mut random);
        let vault_id = VaultId::from_parts(now_ms, u128::from_le_bytes(random));
        let vault_key = VaultKey::generate()?;
        let recovery_key = RecoveryKey::generate()?;
        let bundle = KeyBundle::create(vault_id, &vault_key, &auth, &recovery_key)?;
        let request = RegisterFinishRequest {
            username: self.username.to_string(),
            invite_code: self.invite_code,
            registration_upload: upload,
            key_bundle: bundle.to_bytes(),
            recovery_verifier: recovery_key.verifier().to_vec(),
            device_name: device.name,
            session_kind: device.kind,
        };
        let account = NewAccount {
            vault_id,
            vault_key,
            recovery_key,
            keys: LoginKeys { auth },
        };
        Ok((request, account))
    }
}

/// Login: an OPAQUE login whose export key unlocks the returned key bundle.
pub struct Login {
    passphrase: Zeroizing<String>,
    state: ClientLogin<Suite>,
}

impl Login {
    pub fn start(username: &str, passphrase: &str) -> Result<(Self, LoginStartRequest), AuthError> {
        let username = Username::parse(username)?;
        let passphrase = normalize(passphrase);
        let start = ClientLogin::<Suite>::start(&mut SecureRng, passphrase.as_bytes())
            .map_err(|_| AuthError::Protocol)?;
        let request = LoginStartRequest {
            username: username.to_string(),
            credential_request: start.message.serialize().to_vec(),
        };
        Ok((
            Self {
                passphrase,
                state: start.state,
            },
            request,
        ))
    }

    /// A wrong passphrase (or unknown username) is
    /// [`AuthError::InvalidCredentials`], detected here before anything more
    /// is sent.
    pub fn finish(
        self,
        response: &LoginStartResponse,
        device: Device,
    ) -> Result<(LoginFinishRequest, LoginKeys), AuthError> {
        let (finalization, auth) =
            finish_login(self.state, &self.passphrase, &response.credential_response)?;
        let request = LoginFinishRequest {
            handshake_id: response.handshake_id.clone(),
            credential_finalization: finalization,
            device_name: device.name,
            session_kind: device.kind,
        };
        Ok((request, LoginKeys { auth }))
    }
}

/// Passphrase change: a login with the current passphrase and a registration
/// with the new one in a single round trip, re-wrapping only the bundle's
/// auth entry.
pub struct PassphraseChange {
    old: Zeroizing<String>,
    new: Zeroizing<String>,
    login: ClientLogin<Suite>,
    registration: ClientRegistration<Suite>,
}

impl PassphraseChange {
    /// The account is the one the session's access token belongs to.
    pub fn start(
        old_passphrase: &str,
        new_passphrase: &str,
    ) -> Result<(Self, PassphraseStartRequest), AuthError> {
        let new = self::new_passphrase(new_passphrase)?;
        let old = normalize(old_passphrase);
        let login = ClientLogin::<Suite>::start(&mut SecureRng, old.as_bytes())
            .map_err(|_| AuthError::Protocol)?;
        let registration = ClientRegistration::<Suite>::start(&mut SecureRng, new.as_bytes())
            .map_err(|_| AuthError::Protocol)?;
        let request = PassphraseStartRequest {
            credential_request: login.message.serialize().to_vec(),
            registration_request: registration.message.serialize().to_vec(),
        };
        Ok((
            Self {
                old,
                new,
                login: login.state,
                registration: registration.state,
            },
            request,
        ))
    }

    /// `current_bundle` is the key bundle as last fetched from the server.
    /// Returns the keys for the new passphrase.
    pub fn finish(
        self,
        response: &PassphraseStartResponse,
        current_bundle: &[u8],
    ) -> Result<(PassphraseFinishRequest, LoginKeys), AuthError> {
        let (finalization, old_auth) =
            finish_login(self.login, &self.old, &response.credential_response)?;
        let (upload, new_auth) = finish_registration(
            self.registration,
            &self.new,
            &response.registration_response,
        )?;
        let mut bundle = KeyBundle::from_bytes(current_bundle)?;
        bundle.change_auth(&old_auth, &new_auth)?;
        let request = PassphraseFinishRequest {
            handshake_id: response.handshake_id.clone(),
            credential_finalization: finalization,
            registration_upload: upload,
            key_bundle: bundle.to_bytes(),
        };
        Ok((request, LoginKeys { auth: new_auth }))
    }
}

/// Recovery: prove possession of the recovery key, then re-register with a
/// new passphrase and re-wrap the bundle's auth entry under it.
pub struct Recovery {
    recovery_key: RecoveryKey,
    new: Zeroizing<String>,
    state: ClientRegistration<Suite>,
}

impl Recovery {
    pub fn start(
        username: &str,
        recovery_key: RecoveryKey,
        new_passphrase: &str,
    ) -> Result<(Self, RecoverStartRequest), AuthError> {
        let username = Username::parse(username)?;
        let new = self::new_passphrase(new_passphrase)?;
        let start = ClientRegistration::<Suite>::start(&mut SecureRng, new.as_bytes())
            .map_err(|_| AuthError::Protocol)?;
        let request = RecoverStartRequest {
            username: username.to_string(),
            recovery_verifier: recovery_key.verifier().to_vec(),
            registration_request: start.message.serialize().to_vec(),
        };
        Ok((
            Self {
                recovery_key,
                new,
                state: start.state,
            },
            request,
        ))
    }

    /// Returns the keys for the new passphrase. A bundle the recovery key
    /// doesn't unlock is an error, and nothing is sent.
    pub fn finish(
        self,
        response: &RecoverStartResponse,
        device: Device,
    ) -> Result<(RecoverFinishRequest, LoginKeys), AuthError> {
        let (upload, new_auth) =
            finish_registration(self.state, &self.new, &response.registration_response)?;
        let mut bundle = KeyBundle::from_bytes(&response.key_bundle)?;
        bundle.recover_auth(&self.recovery_key, &new_auth)?;
        let request = RecoverFinishRequest {
            handshake_id: response.handshake_id.clone(),
            registration_upload: upload,
            key_bundle: bundle.to_bytes(),
            device_name: device.name,
            session_kind: device.kind,
        };
        Ok((request, LoginKeys { auth: new_auth }))
    }
}

macro_rules! redacted_debug {
    ($($ty:ident),*) => {$(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "(..)"))
            }
        }
    )*};
}

redacted_debug!(Registration, Login, PassphraseChange, Recovery);

/// NFC-normalizes a passphrase so the same text typed on different platforms
/// gives the same OPAQUE input. The buffer is zeroized on drop.
fn normalize(passphrase: &str) -> Zeroizing<String> {
    // NFC is at most 3x longer in UTF-8; reserving up front avoids leaving
    // reallocated copies behind.
    let mut out = Zeroizing::new(String::with_capacity(passphrase.len() * 3));
    out.extend(passphrase.nfc());
    out
}

fn new_passphrase(passphrase: &str) -> Result<Zeroizing<String>, AuthError> {
    let passphrase = normalize(passphrase);
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(AuthError::PassphraseTooShort);
    }
    Ok(passphrase)
}

fn export_key_secret(mut export_key: impl AsMut<[u8]>) -> Result<AuthSecret, AuthError> {
    let secret = AuthSecret::from_bytes(export_key.as_mut());
    export_key.as_mut().zeroize();
    Ok(secret?)
}

/// Returns the registration upload and the export key.
fn finish_registration(
    state: ClientRegistration<Suite>,
    passphrase: &str,
    response: &[u8],
) -> Result<(Vec<u8>, AuthSecret), AuthError> {
    let response =
        RegistrationResponse::<Suite>::deserialize(response).map_err(|_| AuthError::Protocol)?;
    let result = state
        .finish(
            &mut SecureRng,
            passphrase.as_bytes(),
            response,
            ClientRegistrationFinishParameters::default(),
        )
        .map_err(|_| AuthError::Protocol)?;
    let auth = export_key_secret(result.export_key)?;
    Ok((result.message.serialize().to_vec(), auth))
}

/// Returns the credential finalization and the export key.
fn finish_login(
    state: ClientLogin<Suite>,
    passphrase: &str,
    response: &[u8],
) -> Result<(Vec<u8>, AuthSecret), AuthError> {
    let response =
        CredentialResponse::<Suite>::deserialize(response).map_err(|_| AuthError::Protocol)?;
    let result = state
        .finish(
            &mut SecureRng,
            passphrase.as_bytes(),
            response,
            ClientLoginFinishParameters::default(),
        )
        .map_err(|e| match e {
            ProtocolError::InvalidLoginError => AuthError::InvalidCredentials,
            _ => AuthError::Protocol,
        })?;
    let auth = export_key_secret(result.export_key)?;
    Ok((result.message.serialize().to_vec(), auth))
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    use opaque_ke::{
        CredentialFinalization, CredentialRequest, RegistrationRequest, RegistrationUpload,
        ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup,
    };
    use serde::Serialize;

    use super::*;
    use crate::auth::api::RecoverStartResponse;
    use crate::crypto::{CryptoError, NoteKey, PayloadKind};
    use crate::note::NoteId;

    const MARKER: &str = "zebra-MARKER-4417";

    /// The server side, in process, with every request recorded as JSON.
    struct Server {
        setup: ServerSetup<Suite>,
        username: String,
        record: Option<Vec<u8>>,
        bundle: Vec<u8>,
        sent: Vec<String>,
    }

    impl Server {
        fn new() -> Self {
            Self {
                setup: ServerSetup::new(&mut SecureRng),
                username: String::new(),
                record: None,
                bundle: Vec::new(),
                sent: Vec::new(),
            }
        }

        fn log(&mut self, request: &impl Serialize) {
            self.sent.push(serde_json::to_string(request).unwrap());
        }

        fn registration_response(&self, request: &[u8]) -> Vec<u8> {
            let request = RegistrationRequest::deserialize(request).unwrap();
            ServerRegistration::<Suite>::start(&self.setup, request, self.username.as_bytes())
                .unwrap()
                .message
                .serialize()
                .to_vec()
        }

        fn register(&mut self, username: &str, passphrase: &str) -> NewAccount {
            let (reg, start) = Registration::start(username, passphrase, "").unwrap();
            self.log(&start);
            self.username = start.username.clone();
            let response = RegisterStartResponse {
                registration_response: self.registration_response(&start.registration_request),
            };
            let (finish, account) = reg.finish(&response, Device::default(), 1_000).unwrap();
            self.log(&finish);
            self.record = Some(finish.registration_upload);
            self.bundle = finish.key_bundle;
            account
        }

        fn credential_response(&self, request: &[u8]) -> (ServerLogin<Suite>, Vec<u8>) {
            let record = self
                .record
                .as_ref()
                .map(|r| ServerRegistration::finish(RegistrationUpload::deserialize(r).unwrap()));
            let start = ServerLogin::start(
                &mut SecureRng,
                &self.setup,
                record,
                CredentialRequest::deserialize(request).unwrap(),
                self.username.as_bytes(),
                ServerLoginParameters::default(),
            )
            .unwrap();
            (start.state, start.message.serialize().to_vec())
        }

        fn login(&mut self, username: &str, passphrase: &str) -> Result<LoginKeys, AuthError> {
            let (login, start) = Login::start(username, passphrase)?;
            self.log(&start);
            let (state, credential_response) = self.credential_response(&start.credential_request);
            let response = LoginStartResponse {
                handshake_id: "h".into(),
                credential_response,
            };
            let (finish, keys) = login.finish(&response, Device::default())?;
            self.log(&finish);
            state
                .finish(
                    CredentialFinalization::deserialize(&finish.credential_finalization).unwrap(),
                    ServerLoginParameters::default(),
                )
                .unwrap();
            Ok(keys)
        }

        fn change_passphrase(&mut self, old: &str, new: &str) -> PassphraseFinishRequest {
            let (change, start) = PassphraseChange::start(old, new).unwrap();
            self.log(&start);
            let (state, credential_response) = self.credential_response(&start.credential_request);
            let response = PassphraseStartResponse {
                handshake_id: "h".into(),
                credential_response,
                registration_response: self.registration_response(&start.registration_request),
            };
            let (finish, _) = change.finish(&response, &self.bundle).unwrap();
            self.log(&finish);
            state
                .finish(
                    CredentialFinalization::deserialize(&finish.credential_finalization).unwrap(),
                    ServerLoginParameters::default(),
                )
                .unwrap();
            self.record = Some(finish.registration_upload.clone());
            self.bundle = finish.key_bundle.clone();
            finish
        }

        fn recover(&mut self, recovery: RecoveryKey, new: &str) -> RecoverFinishRequest {
            let (recovery, start) = Recovery::start(&self.username.clone(), recovery, new).unwrap();
            self.log(&start);
            let response = RecoverStartResponse {
                handshake_id: "h".into(),
                key_bundle: self.bundle.clone(),
                registration_response: self.registration_response(&start.registration_request),
            };
            let (finish, _) = recovery.finish(&response, Device::default()).unwrap();
            self.log(&finish);
            self.record = Some(finish.registration_upload.clone());
            self.bundle = finish.key_bundle.clone();
            finish
        }
    }

    /// Wraps a note key under `account`'s vault key, as a client would right
    /// after registering, and returns a check that another vault key unwraps it.
    fn note_check(account: &NewAccount) -> impl Fn(&VaultKey) {
        let vault = account.vault_id;
        let note = NoteId::from_parts(1_000, 7);
        let note_key = NoteKey::generate().unwrap();
        let wrapped = account
            .vault_key
            .wrap_note_key(vault, note, &note_key)
            .unwrap();
        let sealed = note_key
            .encrypt(vault, note, PayloadKind::NotePayload, b"hello")
            .unwrap();
        move |vault_key: &VaultKey| {
            let key = vault_key.unwrap_note_key(vault, note, &wrapped).unwrap();
            let plain = key
                .decrypt(vault, note, PayloadKind::NotePayload, &sealed)
                .unwrap();
            assert_eq!(&*plain, b"hello");
        }
    }

    fn passphrase(tag: &str) -> String {
        format!("{MARKER} {tag}")
    }

    #[test]
    fn register_then_login_unlocks_the_vault() {
        let mut server = Server::new();
        let account = server.register("Alice", &passphrase("one"));
        let check = note_check(&account);
        check(&account.keys.unlock(&server.bundle).unwrap());

        let keys = server.login("ALICE", &passphrase("one")).unwrap();
        check(&keys.unlock(&server.bundle).unwrap());
    }

    #[test]
    fn wrong_passphrase_fails() {
        let mut server = Server::new();
        server.register("alice", &passphrase("one"));
        assert_eq!(
            server.login("alice", &passphrase("two")).unwrap_err(),
            AuthError::InvalidCredentials
        );
    }

    #[test]
    fn unknown_user_fails_like_a_wrong_passphrase() {
        let mut server = Server::new();
        server.username = "nobody".into();
        assert_eq!(
            server.login("nobody", &passphrase("one")).unwrap_err(),
            AuthError::InvalidCredentials
        );
    }

    #[test]
    fn short_passphrases_are_refused_before_any_message() {
        let eleven = "abcdefghijk";
        assert_eq!(eleven.chars().count(), 11);
        assert_eq!(
            Registration::start("alice", eleven, "").unwrap_err(),
            AuthError::PassphraseTooShort
        );
        assert_eq!(
            PassphraseChange::start("anything at all", eleven).unwrap_err(),
            AuthError::PassphraseTooShort
        );
        assert_eq!(
            Recovery::start("alice", RecoveryKey::generate().unwrap(), eleven).unwrap_err(),
            AuthError::PassphraseTooShort
        );
        // Twelve scalar values, more bytes: accepted.
        assert!(Registration::start("alice", "ééééééééééé1", "").is_ok());
        // Login doesn't judge length.
        assert!(Login::start("alice", "short").is_ok());
    }

    #[test]
    fn invalid_username_is_refused() {
        assert_eq!(
            Registration::start("al ice", &passphrase("one"), "").unwrap_err(),
            AuthError::InvalidUsername
        );
        assert_eq!(
            Login::start("ab", "x").unwrap_err(),
            AuthError::InvalidUsername
        );
    }

    #[test]
    fn nfc_and_nfd_passphrases_are_the_same() {
        let nfc = "caf\u{e9} au lait, s'il vous pla\u{ee}t";
        let nfd = "cafe\u{301} au lait, s'il vous plai\u{302}t";
        assert_ne!(nfc, nfd);
        let mut server = Server::new();
        server.register("alice", nfc);
        assert!(server.login("alice", nfd).is_ok());
        assert!(server.login("alice", nfc).is_ok());
    }

    #[test]
    fn requests_carry_neither_passphrase_nor_recovery_key() {
        let mut server = Server::new();
        let account = server.register("alice", &passphrase("one"));
        server.login("alice", &passphrase("one")).unwrap();
        server.change_passphrase(&passphrase("one"), &passphrase("two"));
        let shown = account.recovery_key.to_display().to_string();
        let parsed = RecoveryKey::parse(&shown).unwrap();
        server.recover(parsed, &passphrase("three"));

        let compact: String = shown.chars().filter(|&c| c != '-').collect();
        let mut needles = vec![MARKER.to_owned(), shown, compact.clone()];
        for engine in [&URL_SAFE_NO_PAD, &STANDARD] {
            needles.push(engine.encode(MARKER));
            needles.push(engine.encode(&compact));
        }
        needles.push(MARKER.bytes().map(|b| format!("{b:02x}")).collect());
        for request in &server.sent {
            let decoded: Vec<Vec<u8>> = serde_json::from_str::<serde_json::Value>(request)
                .unwrap()
                .as_object()
                .unwrap()
                .values()
                .filter_map(|v| v.as_str())
                .filter_map(|s| URL_SAFE_NO_PAD.decode(s).ok())
                .collect();
            for needle in &needles {
                assert!(!request.contains(needle.as_str()), "{needle} in {request}");
                assert!(
                    decoded
                        .iter()
                        .all(|bytes| !bytes.windows(needle.len()).any(|w| w == needle.as_bytes())),
                    "{needle} in a byte field of {request}"
                );
            }
        }
        assert_eq!(server.sent.len(), 8);
    }

    #[test]
    fn passphrase_change_keeps_the_vault_key() {
        let mut server = Server::new();
        let account = server.register("alice", &passphrase("old"));
        let check = note_check(&account);
        let original = KeyBundle::from_bytes(&server.bundle).unwrap();

        let finish = server.change_passphrase(&passphrase("old"), &passphrase("new"));
        let uploaded = KeyBundle::from_bytes(&finish.key_bundle).unwrap();
        assert!(uploaded.same_recovery_entry(&original));
        assert_eq!(uploaded.vault_id(), account.vault_id);

        let keys = server.login("alice", &passphrase("new")).unwrap();
        check(&keys.unlock(&server.bundle).unwrap());
        assert_eq!(
            server.login("alice", &passphrase("old")).unwrap_err(),
            AuthError::InvalidCredentials
        );
        check(
            &uploaded
                .unlock_with_recovery(&account.recovery_key)
                .unwrap(),
        );
    }

    #[test]
    fn passphrase_change_with_wrong_current_passphrase_fails() {
        let mut server = Server::new();
        server.register("alice", &passphrase("old"));
        let (change, start) =
            PassphraseChange::start(&passphrase("wrong"), &passphrase("new")).unwrap();
        let (_, credential_response) = server.credential_response(&start.credential_request);
        let response = PassphraseStartResponse {
            handshake_id: "h".into(),
            credential_response,
            registration_response: server.registration_response(&start.registration_request),
        };
        assert_eq!(
            change.finish(&response, &server.bundle).unwrap_err(),
            AuthError::InvalidCredentials
        );
    }

    #[test]
    fn recovery_sets_a_new_passphrase_for_the_same_vault_key() {
        let mut server = Server::new();
        let account = server.register("alice", &passphrase("old"));
        let check = note_check(&account);
        let original = KeyBundle::from_bytes(&server.bundle).unwrap();
        let shown = account.recovery_key.to_display();

        let finish = server.recover(RecoveryKey::parse(&shown).unwrap(), &passphrase("new"));
        assert!(
            KeyBundle::from_bytes(&finish.key_bundle)
                .unwrap()
                .same_recovery_entry(&original)
        );
        let keys = server.login("alice", &passphrase("new")).unwrap();
        check(&keys.unlock(&server.bundle).unwrap());
        assert!(server.login("alice", &passphrase("old")).is_err());

        // The recovery key keeps working.
        server.recover(RecoveryKey::parse(&shown).unwrap(), &passphrase("newer"));
        let keys = server.login("alice", &passphrase("newer")).unwrap();
        check(&keys.unlock(&server.bundle).unwrap());
    }

    #[test]
    fn recovery_with_the_wrong_key_sends_nothing() {
        let mut server = Server::new();
        server.register("alice", &passphrase("old"));
        let (recovery, start) = Recovery::start(
            "alice",
            RecoveryKey::generate().unwrap(),
            &passphrase("new"),
        )
        .unwrap();
        let response = RecoverStartResponse {
            handshake_id: "h".into(),
            key_bundle: server.bundle.clone(),
            registration_response: server.registration_response(&start.registration_request),
        };
        assert_eq!(
            recovery.finish(&response, Device::default()).unwrap_err(),
            AuthError::Crypto(CryptoError::Decrypt)
        );
    }

    #[test]
    fn garbage_server_messages_are_protocol_errors() {
        let (login, _) = Login::start("alice", "x").unwrap();
        let response = LoginStartResponse {
            handshake_id: "h".into(),
            credential_response: vec![1, 2, 3],
        };
        assert_eq!(
            login.finish(&response, Device::default()).unwrap_err(),
            AuthError::Protocol
        );
        let (reg, _) = Registration::start("alice", &passphrase("one"), "").unwrap();
        let response = RegisterStartResponse {
            registration_response: vec![0; 64],
        };
        assert!(reg.finish(&response, Device::default(), 0).is_err());
    }

    #[test]
    fn device_and_invite_are_carried_into_finish_requests() {
        let mut server = Server::new();
        server.username = "alice".into();
        let (reg, start) =
            Registration::start("alice", &passphrase("one"), " 7kq2m xw9pd ").unwrap();
        assert_eq!(start.invite_code, " 7kq2m xw9pd ");
        let response = RegisterStartResponse {
            registration_response: server.registration_response(&start.registration_request),
        };
        let device = Device {
            name: "Work laptop".into(),
            kind: SessionKind::Browser,
        };
        let (finish, account) = reg.finish(&response, device, 1_234).unwrap();
        assert_eq!(finish.invite_code, " 7kq2m xw9pd ");
        assert_eq!(finish.device_name, "Work laptop");
        assert_eq!(finish.session_kind, SessionKind::Browser);
        assert_eq!(finish.recovery_verifier, account.recovery_key.verifier());
        assert_eq!(
            KeyBundle::from_bytes(&finish.key_bundle)
                .unwrap()
                .vault_id(),
            account.vault_id
        );
    }

    #[test]
    fn debug_output_has_no_secrets() {
        let secret = passphrase("debug");
        let (reg, _) = Registration::start("alice", &secret, "").unwrap();
        let (login, _) = Login::start("alice", &secret).unwrap();
        let (change, _) = PassphraseChange::start(&secret, &secret).unwrap();
        let recovery_key = RecoveryKey::generate().unwrap();
        let shown = recovery_key.to_display().to_string();
        let (recovery, _) = Recovery::start("alice", recovery_key, &secret).unwrap();
        let mut server = Server::new();
        let account = server.register("bob", &secret);
        let account_shown = account.recovery_key.to_display().to_string();

        let debug = format!(
            "{reg:?} {login:?} {change:?} {recovery:?} {account:?} {:?}",
            account.keys
        );
        assert_eq!(
            format!(
                "{reg:?} {login:?} {change:?} {recovery:?} {:?}",
                account.keys
            ),
            "Registration(..) Login(..) PassphraseChange(..) Recovery(..) LoginKeys(..)"
        );
        for needle in [MARKER, &shown, &account_shown] {
            assert!(!debug.contains(needle), "{needle} in {debug}");
        }
        assert!(debug.contains("VaultKey(..)"), "{debug}");
    }
}
