//! Secrets in the OS credential store, behind a trait with an in-memory
//! implementation for tests and for systems without a secure store.
//!
//! Entries under one service, each keyed by this install's id:
//!
//! | entry | contents | present when |
//! |---|---|---|
//! | `vault-key` | the vault id and the vault key | a notebook exists and the OS has a secure store |
//!
//! Installs that were signed in to a sync server may still have these,
//! which are only read (by the upgrade) and then deleted:
//!
//! | entry | contents | present when |
//! |---|---|---|
//! | `session` | server, username, vault id, session id, refresh token | signed in |
//! | `local-unlock` | the vault key wrapped under the passphrase, and the key bundle fingerprint | signed in, unless a passphrase change elsewhere was noticed |
//! | `auto-unlock` | the vault key | signed in and not locked |
//!
//! Nothing here ever falls back to a file.

#[cfg(test)]
use std::collections::HashMap;
use std::fmt;
#[cfg(test)]
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use notebook_core::crypto::{LocalUnlock, VaultId, VaultKey};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

/// The keychain service every entry lives under.
pub const SERVICE: &str = "com.blakfeld.notebook";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Entry {
    VaultKey,
    /// Legacy, read and deleted by the upgrade.
    Session,
    /// Legacy, read and deleted by the upgrade.
    LocalUnlock,
    /// Legacy, read and deleted by the upgrade.
    AutoUnlock,
}

impl Entry {
    fn name(self) -> &'static str {
        match self {
            Self::VaultKey => "vault-key",
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

/// Entries in memory only, for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<Entry, Zeroizing<Vec<u8>>>>,
    /// Fail every write as if there were no secure store (tests).
    pub unavailable: bool,
    /// Accept this many more writes, then fail like `unavailable` (tests).
    writes_left: Mutex<Option<usize>>,
}

#[cfg(test)]
impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }

    /// A store that stops working after `writes` writes, as a keychain
    /// that locks partway through.
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

#[cfg(test)]
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

fn corrupt(entry: Entry) -> CredentialError {
    CredentialError::Other(format!("{} entry is unreadable", entry.name()))
}

/// The `vault-key` entry. The encoded key is zeroized on drop.
#[derive(Serialize, Deserialize)]
struct StoredVaultKey {
    vault_id: String,
    key: String,
}

impl Drop for StoredVaultKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// The notebook's vault id and key, if stored.
pub fn load_vault_key(
    store: &dyn CredentialStore,
) -> Result<Option<(VaultId, VaultKey)>, CredentialError> {
    let Some(bytes) = store.get(Entry::VaultKey)? else {
        return Ok(None);
    };
    let stored: StoredVaultKey =
        serde_json::from_slice(&bytes).map_err(|_| corrupt(Entry::VaultKey))?;
    let vault = stored
        .vault_id
        .parse()
        .map_err(|_| corrupt(Entry::VaultKey))?;
    let key = URL_SAFE_NO_PAD
        .decode(stored.key.as_bytes())
        .map(Zeroizing::new)
        .ok()
        .and_then(|bytes| VaultKey::from_secret_bytes(&bytes).ok())
        .ok_or_else(|| corrupt(Entry::VaultKey))?;
    Ok(Some((vault, key)))
}

pub fn save_vault_key(
    store: &dyn CredentialStore,
    vault: VaultId,
    key: &VaultKey,
) -> Result<(), CredentialError> {
    let stored = StoredVaultKey {
        vault_id: vault.to_string(),
        key: URL_SAFE_NO_PAD.encode(*key.to_secret_bytes()),
    };
    let bytes = Zeroizing::new(serde_json::to_vec(&stored).expect("entries serialize"));
    store.set(Entry::VaultKey, &bytes)
}

/// The part of a legacy `session` entry the upgrade needs. Other fields,
/// the refresh token included, are never read.
#[derive(Deserialize)]
struct LegacySession {
    vault_id: String,
}

/// The vault of a legacy signed-in install, if there is one.
pub fn load_legacy_session(
    store: &dyn CredentialStore,
) -> Result<Option<VaultId>, CredentialError> {
    let Some(bytes) = store.get(Entry::Session)? else {
        return Ok(None);
    };
    let session: LegacySession =
        serde_json::from_slice(&bytes).map_err(|_| corrupt(Entry::Session))?;
    session
        .vault_id
        .parse()
        .map(Some)
        .map_err(|_| corrupt(Entry::Session))
}

/// The legacy `local-unlock` entry.
#[derive(Deserialize)]
struct StoredLocalUnlock {
    blob: String,
}

/// A legacy install's passphrase wrap of the vault key.
pub fn load_legacy_local_unlock(
    store: &dyn CredentialStore,
    vault: VaultId,
) -> Result<Option<LocalUnlock>, CredentialError> {
    let Some(bytes) = store.get(Entry::LocalUnlock)? else {
        return Ok(None);
    };
    let stored: StoredLocalUnlock =
        serde_json::from_slice(&bytes).map_err(|_| corrupt(Entry::LocalUnlock))?;
    URL_SAFE_NO_PAD
        .decode(&stored.blob)
        .ok()
        .and_then(|b| LocalUnlock::from_bytes(vault, &b).ok())
        .map(Some)
        .ok_or_else(|| corrupt(Entry::LocalUnlock))
}

/// A legacy install's stored vault key (present while it wasn't locked).
pub fn load_legacy_auto_unlock(
    store: &dyn CredentialStore,
) -> Result<Option<VaultKey>, CredentialError> {
    store
        .get(Entry::AutoUnlock)?
        .map(|bytes| VaultKey::from_secret_bytes(&bytes).map_err(|_| corrupt(Entry::AutoUnlock)))
        .transpose()
}

/// Removes the legacy entries. Tries all three even if one fails.
pub fn delete_legacy(store: &dyn CredentialStore) -> Result<(), CredentialError> {
    let results = [Entry::AutoUnlock, Entry::LocalUnlock, Entry::Session].map(|e| store.delete(e));
    results.into_iter().collect()
}

/// Writes legacy entries as a signed-in install of the server-backed app
/// left them, in that app's format.
#[cfg(test)]
pub mod legacy {
    use super::*;

    pub const REFRESH_TOKEN: &str = "legacy-refresh-secret";

    pub fn save_session(store: &dyn CredentialStore, vault: VaultId) {
        let session = serde_json::json!({
            "server": "https://notes.example.com",
            "username": "alice",
            "vault_id": vault.to_string(),
            "session_id": "s1",
            "refresh_token": REFRESH_TOKEN,
        });
        store
            .set(Entry::Session, &serde_json::to_vec(&session).unwrap())
            .unwrap();
    }

    pub fn save_local_unlock(store: &dyn CredentialStore, local: &LocalUnlock) {
        let stored = serde_json::json!({
            "blob": URL_SAFE_NO_PAD.encode(local.to_bytes()),
            "fingerprint": URL_SAFE_NO_PAD.encode([9u8; 32]),
        });
        store
            .set(Entry::LocalUnlock, &serde_json::to_vec(&stored).unwrap())
            .unwrap();
    }

    pub fn save_auto_unlock(store: &dyn CredentialStore, key: &VaultKey) {
        store
            .set(Entry::AutoUnlock, &*key.to_secret_bytes())
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> VaultId {
        VaultId::from_parts(1, 1)
    }

    #[test]
    fn vault_key_round_trips() {
        let store = MemoryStore::new();
        assert!(load_vault_key(&store).unwrap().is_none());
        let key = VaultKey::generate().unwrap();
        save_vault_key(&store, vault(), &key).unwrap();
        let (loaded_vault, loaded) = load_vault_key(&store).unwrap().unwrap();
        assert_eq!(loaded_vault, vault());
        assert_eq!(*loaded.to_secret_bytes(), *key.to_secret_bytes());
        store.delete(Entry::VaultKey).unwrap();
        assert!(load_vault_key(&store).unwrap().is_none());
    }

    #[test]
    fn legacy_entries_are_read_and_deleted() {
        let store = MemoryStore::new();
        assert_eq!(load_legacy_session(&store).unwrap(), None);
        legacy::save_session(&store, vault());
        assert_eq!(load_legacy_session(&store).unwrap(), Some(vault()));

        let key = VaultKey::generate().unwrap();
        legacy::save_auto_unlock(&store, &key);
        let loaded = load_legacy_auto_unlock(&store).unwrap().unwrap();
        assert_eq!(*loaded.to_secret_bytes(), *key.to_secret_bytes());

        let local = LocalUnlock::seal(vault(), &key, "correct horse battery").unwrap();
        legacy::save_local_unlock(&store, &local);
        assert_eq!(
            load_legacy_local_unlock(&store, vault()).unwrap(),
            Some(local)
        );

        save_vault_key(&store, vault(), &key).unwrap();
        delete_legacy(&store).unwrap();
        assert_eq!(load_legacy_session(&store).unwrap(), None);
        assert!(load_legacy_auto_unlock(&store).unwrap().is_none());
        assert!(load_legacy_local_unlock(&store, vault()).unwrap().is_none());
        // The new entry is not legacy.
        assert!(load_vault_key(&store).unwrap().is_some());
        // Deleting again is fine.
        delete_legacy(&store).unwrap();
    }

    #[test]
    fn unavailable_store_reports_no_secure_store() {
        let store = MemoryStore::unavailable();
        let key = VaultKey::generate().unwrap();
        assert_eq!(
            save_vault_key(&store, vault(), &key),
            Err(CredentialError::NoSecureStore)
        );
        assert!(matches!(
            load_vault_key(&store),
            Err(CredentialError::NoSecureStore)
        ));
        assert_eq!(
            load_legacy_session(&store),
            Err(CredentialError::NoSecureStore)
        );
    }

    #[test]
    fn corrupt_entries_are_errors_without_contents() {
        let store = MemoryStore::new();
        store
            .set(Entry::Session, b"legacy-refresh-secret not json")
            .unwrap();
        let err = load_legacy_session(&store).unwrap_err();
        assert!(!err.to_string().contains("refresh-secret"));
        store.set(Entry::AutoUnlock, &[1; 5]).unwrap();
        assert!(load_legacy_auto_unlock(&store).is_err());
        store
            .set(Entry::VaultKey, br#"{"vault_id":"x","key":"secret"}"#)
            .unwrap();
        let err = load_vault_key(&store).err().unwrap();
        assert!(!err.to_string().contains("secret"));
    }

    /// Writes to the real OS store, so it runs only on request:
    /// `cargo test -p notebook-app -- --ignored keyring_round_trip`.
    #[test]
    #[ignore]
    fn keyring_round_trip() {
        let account = format!("test-{}", std::process::id());
        let store = KeyringStore::new(&account);
        store.set(Entry::VaultKey, b"probe").unwrap();
        assert_eq!(
            store
                .get(Entry::VaultKey)
                .unwrap()
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"probe"[..])
        );
        #[cfg(target_os = "macos")]
        {
            let found = std::process::Command::new("security")
                .args(["find-generic-password", "-s", SERVICE, "-a"])
                .arg(format!("{account}:vault-key"))
                .output()
                .unwrap();
            assert!(found.status.success(), "entry not in the login keychain");
        }
        store.delete(Entry::VaultKey).unwrap();
        assert!(store.get(Entry::VaultKey).unwrap().is_none());
    }
}
