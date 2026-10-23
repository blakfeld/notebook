//! Secrets in the OS credential store, behind a trait with an in-memory
//! implementation for tests and for systems without a secure store.
//!
//! Three entries under one service, each keyed by this install's id:
//!
//! | entry | contents | present when |
//! |---|---|---|
//! | `session` | server, username, vault id, session id, refresh token | signed in |
//! | `local-unlock` | the vault key wrapped under the passphrase, and the key bundle fingerprint | signed in, until a passphrase change elsewhere is noticed |
//! | `auto-unlock` | the vault key | signed in and not locked |
//!
//! Nothing here ever falls back to a file.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use notebook_core::auth::api::Token;
use notebook_core::crypto::{LocalUnlock, VaultId, VaultKey};
use notebook_core::workspace::ServerUrl;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The keychain service every entry lives under.
pub const SERVICE: &str = "com.blakfeld.notebook";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entry {
    Session,
    LocalUnlock,
    AutoUnlock,
}

impl Entry {
    fn name(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::LocalUnlock => "local-unlock",
            Self::AutoUnlock => "auto-unlock",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// There is no usable secure store (no backend, or it refused access).
    NoSecureStore,
    /// The store failed, or an entry didn't parse. The message has no secrets.
    Other(String),
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSecureStore => f.write_str("no secure credential store is available"),
            Self::Other(msg) => write!(f, "credential store: {msg}"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Where the entries live. Values are opaque bytes; the typed helpers below
/// encode them.
pub trait CredentialStore: Send + Sync {
    fn get(&self, entry: Entry) -> Result<Option<Zeroizing<Vec<u8>>>, CredentialError>;
    fn set(&self, entry: Entry, value: &[u8]) -> Result<(), CredentialError>;
    /// Deleting a missing entry is fine.
    fn delete(&self, entry: Entry) -> Result<(), CredentialError>;
}

/// The OS store through `keyring`: Keychain on macOS, Credential Manager on
/// Windows, Secret Service on Linux.
pub struct KeyringStore {
    account: String,
}

impl KeyringStore {
    /// `install_id` scopes the entries to this install.
    pub fn new(install_id: &str) -> Self {
        Self {
            account: install_id.to_owned(),
        }
    }

    fn entry(&self, entry: Entry) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(SERVICE, &format!("{}:{}", self.account, entry.name()))
            .map_err(store_error)
    }
}

/// Errors that mean the store can't be used at all, as opposed to one call
/// failing.
fn store_error(err: keyring::Error) -> CredentialError {
    match err {
        keyring::Error::NoDefaultStore
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::PlatformFailure(_)
        | keyring::Error::NotSupportedByStore(_) => CredentialError::NoSecureStore,
        // The other variants carry no secret material in their text.
        other => CredentialError::Other(other.to_string()),
    }
}

impl CredentialStore for KeyringStore {
    fn get(&self, entry: Entry) -> Result<Option<Zeroizing<Vec<u8>>>, CredentialError> {
        match self.entry(entry)?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(store_error(err)),
        }
    }

    fn set(&self, entry: Entry, value: &[u8]) -> Result<(), CredentialError> {
        self.entry(entry)?.set_secret(value).map_err(store_error)
    }

    fn delete(&self, entry: Entry) -> Result<(), CredentialError> {
        match self.entry(entry)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(store_error(err)),
        }
    }
}

/// Entries in memory only: for tests, and for the rest of a session on a
/// system with no secure store (so nothing survives a relaunch).
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<Entry, Zeroizing<Vec<u8>>>>,
    /// Fail every write as if there were no secure store (tests).
    pub unavailable: bool,
    /// Accept this many more writes, then fail like `unavailable` (tests).
    writes_left: Mutex<Option<usize>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }

    /// A store that stops working after `writes` writes, as a keychain
    /// that locks partway through a sign-in.
    #[cfg(test)]
    pub fn fails_after(writes: usize) -> Self {
        Self {
            writes_left: Mutex::new(Some(writes)),
            ..Self::default()
        }
    }

    fn failing(&self) -> bool {
        self.unavailable
            || self
                .writes_left
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some_and(|left| left == 0)
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<Entry, Zeroizing<Vec<u8>>>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl CredentialStore for MemoryStore {
    fn get(&self, entry: Entry) -> Result<Option<Zeroizing<Vec<u8>>>, CredentialError> {
        if self.unavailable {
            return Err(CredentialError::NoSecureStore);
        }
        Ok(self.entries().get(&entry).cloned())
    }

    fn set(&self, entry: Entry, value: &[u8]) -> Result<(), CredentialError> {
        if self.failing() {
            return Err(CredentialError::NoSecureStore);
        }
        if let Some(left) = &mut *self.writes_left.lock().unwrap_or_else(|e| e.into_inner()) {
            *left -= 1;
        }
        self.entries().insert(entry, Zeroizing::new(value.to_vec()));
        Ok(())
    }

    fn delete(&self, entry: Entry) -> Result<(), CredentialError> {
        if self.unavailable {
            return Err(CredentialError::NoSecureStore);
        }
        self.entries().remove(&entry);
        Ok(())
    }
}

/// The `session` entry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSession {
    pub server: String,
    pub username: String,
    pub vault_id: String,
    pub session_id: String,
    pub refresh_token: Token,
}

impl fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredSession")
            .field("server", &self.server)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl StoredSession {
    pub fn server(&self) -> Result<ServerUrl, CredentialError> {
        ServerUrl::parse(&self.server).map_err(|_| corrupt(Entry::Session))
    }

    pub fn vault(&self) -> Result<VaultId, CredentialError> {
        self.vault_id.parse().map_err(|_| corrupt(Entry::Session))
    }
}

/// The `local-unlock` entry.
#[derive(Serialize, Deserialize)]
struct StoredLocalUnlock {
    blob: String,
    fingerprint: String,
}

fn corrupt(entry: Entry) -> CredentialError {
    CredentialError::Other(format!("{} entry is unreadable", entry.name()))
}

pub fn load_session(store: &dyn CredentialStore) -> Result<Option<StoredSession>, CredentialError> {
    store
        .get(Entry::Session)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| corrupt(Entry::Session)))
        .transpose()
}

pub fn save_session(
    store: &dyn CredentialStore,
    session: &StoredSession,
) -> Result<(), CredentialError> {
    let bytes = Zeroizing::new(serde_json::to_vec(session).expect("sessions serialize"));
    store.set(Entry::Session, &bytes)
}

/// The local-unlock wrap and the fingerprint of the bundle it was made
/// alongside.
pub fn load_local_unlock(
    store: &dyn CredentialStore,
    vault: VaultId,
) -> Result<Option<(LocalUnlock, [u8; 32])>, CredentialError> {
    let Some(bytes) = store.get(Entry::LocalUnlock)? else {
        return Ok(None);
    };
    let stored: StoredLocalUnlock =
        serde_json::from_slice(&bytes).map_err(|_| corrupt(Entry::LocalUnlock))?;
    let blob = URL_SAFE_NO_PAD
        .decode(&stored.blob)
        .ok()
        .and_then(|b| LocalUnlock::from_bytes(vault, &b).ok())
        .ok_or_else(|| corrupt(Entry::LocalUnlock))?;
    let fingerprint = URL_SAFE_NO_PAD
        .decode(&stored.fingerprint)
        .ok()
        .and_then(|f| <[u8; 32]>::try_from(f).ok())
        .ok_or_else(|| corrupt(Entry::LocalUnlock))?;
    Ok(Some((blob, fingerprint)))
}

pub fn save_local_unlock(
    store: &dyn CredentialStore,
    local: &LocalUnlock,
    fingerprint: &[u8; 32],
) -> Result<(), CredentialError> {
    let stored = StoredLocalUnlock {
        blob: URL_SAFE_NO_PAD.encode(local.to_bytes()),
        fingerprint: URL_SAFE_NO_PAD.encode(fingerprint),
    };
    store.set(
        Entry::LocalUnlock,
        &serde_json::to_vec(&stored).expect("entries serialize"),
    )
}

pub fn load_auto_unlock(store: &dyn CredentialStore) -> Result<Option<VaultKey>, CredentialError> {
    store
        .get(Entry::AutoUnlock)?
        .map(|bytes| VaultKey::from_secret_bytes(&bytes).map_err(|_| corrupt(Entry::AutoUnlock)))
        .transpose()
}

pub fn save_auto_unlock(
    store: &dyn CredentialStore,
    key: &VaultKey,
) -> Result<(), CredentialError> {
    store.set(Entry::AutoUnlock, &*key.to_secret_bytes())
}

/// Removes every entry. Tries all three even if one fails.
pub fn delete_all(store: &dyn CredentialStore) -> Result<(), CredentialError> {
    let results = [Entry::AutoUnlock, Entry::LocalUnlock, Entry::Session].map(|e| store.delete(e));
    results.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> StoredSession {
        StoredSession {
            server: "https://notes.example.com".into(),
            username: "alice".into(),
            vault_id: VaultId::from_parts(1, 1).to_string(),
            session_id: "s1".into(),
            refresh_token: Token("refresh-secret".into()),
        }
    }

    #[test]
    fn entries_round_trip_and_delete() {
        let store = MemoryStore::new();
        assert_eq!(load_session(&store).unwrap(), None);
        save_session(&store, &session()).unwrap();
        assert_eq!(load_session(&store).unwrap(), Some(session()));
        assert_eq!(
            load_session(&store).unwrap().unwrap().vault().unwrap(),
            VaultId::from_parts(1, 1)
        );

        let vault = VaultId::from_parts(1, 1);
        let key = VaultKey::generate().unwrap();
        save_auto_unlock(&store, &key).unwrap();
        let loaded = load_auto_unlock(&store).unwrap().unwrap();
        assert_eq!(*loaded.to_secret_bytes(), *key.to_secret_bytes());

        let local = LocalUnlock::seal(vault, &key, "correct horse battery").unwrap();
        save_local_unlock(&store, &local, &[9; 32]).unwrap();
        let (loaded, fingerprint) = load_local_unlock(&store, vault).unwrap().unwrap();
        assert_eq!(loaded, local);
        assert_eq!(fingerprint, [9; 32]);

        delete_all(&store).unwrap();
        assert_eq!(load_session(&store).unwrap(), None);
        assert!(load_auto_unlock(&store).unwrap().is_none());
        assert!(load_local_unlock(&store, vault).unwrap().is_none());
        // Deleting again is fine.
        delete_all(&store).unwrap();
    }

    #[test]
    fn unavailable_store_reports_no_secure_store() {
        let store = MemoryStore::unavailable();
        assert_eq!(
            save_session(&store, &session()),
            Err(CredentialError::NoSecureStore)
        );
        assert_eq!(load_session(&store), Err(CredentialError::NoSecureStore));
    }

    #[test]
    fn corrupt_entries_are_errors_without_contents() {
        let store = MemoryStore::new();
        store
            .set(Entry::Session, b"refresh-secret not json")
            .unwrap();
        let err = load_session(&store).unwrap_err();
        assert!(!err.to_string().contains("refresh-secret"));
        store.set(Entry::AutoUnlock, &[1; 5]).unwrap();
        assert!(load_auto_unlock(&store).is_err());
    }

    /// Writes to the real OS store, so it runs only on request:
    /// `cargo test -p notebook-app -- --ignored keyring_round_trip`.
    #[test]
    #[ignore]
    fn keyring_round_trip() {
        let account = format!("test-{}", std::process::id());
        let store = KeyringStore::new(&account);
        store.set(Entry::Session, b"probe").unwrap();
        assert_eq!(
            store
                .get(Entry::Session)
                .unwrap()
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"probe"[..])
        );
        #[cfg(target_os = "macos")]
        {
            let found = std::process::Command::new("security")
                .args(["find-generic-password", "-s", SERVICE, "-a"])
                .arg(format!("{account}:session"))
                .output()
                .unwrap();
            assert!(found.status.success(), "entry not in the login keychain");
        }
        store.delete(Entry::Session).unwrap();
        assert!(store.get(Entry::Session).unwrap().is_none());
    }

    #[test]
    fn session_debug_hides_the_token() {
        assert!(!format!("{:?}", session()).contains("refresh-secret"));
    }
}
