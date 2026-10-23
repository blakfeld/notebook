//! State machine tests: launch phases, creating a notebook, the recovery
//! key, and the upgrade from a server-backed install. No server or network
//! is involved anywhere.

use std::path::{Path, PathBuf};

use notebook_core::crypto::LocalUnlock;
use notebook_core::sync::{Replica, Shared};
use notebook_storage_sqlite::SqliteStorage;
use serde_json::Value;

use super::*;
use crate::credentials::{Entry, MemoryStore, legacy};
use crate::dto::{SelectionDto, ViewDto};
use crate::events::Recorder;

pub(crate) const OLD_PASSPHRASE: &str = "correct horse battery staple";

/// One install: its data directory and credential store, which survive a
/// relaunch.
pub(crate) struct Install {
    pub dir: tempfile::TempDir,
    pub creds: Arc<MemoryStore>,
    pub events: Arc<Recorder>,
}

impl Install {
    pub fn new() -> Self {
        Self::with_store(MemoryStore::new())
    }

    pub fn with_store(store: MemoryStore) -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            creds: Arc::new(store),
            events: Arc::new(Recorder::default()),
        }
    }

    /// Starts the app, as at launch.
    pub fn launch(&self) -> AppState {
        AppState::launch(
            AppConfig {
                data_dir: self.dir.path().to_owned(),
            },
            self.creds.clone(),
            self.events.clone(),
        )
    }

    pub fn db(&self) -> PathBuf {
        vault::database_path(self.dir.path())
    }

    pub fn has(&self, entry: Entry) -> bool {
        self.creds.get(entry).unwrap().is_some()
    }

    pub fn files(&self) -> Vec<String> {
        file_names(self.dir.path())
    }
}

impl AppState {
    /// Quits: closes the vault without changing any entry.
    pub(crate) fn quit(&self) {
        *self.lock_phase() = Phase::Setup;
    }
}

/// A new notebook on `install`, open, and its recovery key.
pub(crate) async fn created(install: &Install) -> (AppState, String) {
    let app = install.launch();
    let key = app.create_notebook().await.unwrap().to_string();
    app.acknowledge_recovery_key().await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::Open);
    (app, key)
}

pub(crate) fn notes() -> SelectionDto {
    SelectionDto::View(ViewDto::Notes)
}

fn code<T: std::fmt::Debug>(result: AppResult<T>) -> Code {
    result.unwrap_err().code
}

fn phases(events: &Recorder) -> Vec<Value> {
    events
        .take()
        .into_iter()
        .filter(|(name, _)| *name == events::APP_PHASE_CHANGED)
        .map(|(_, payload)| payload["phase"].clone())
        .collect()
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn bodies(app: &AppState) -> Vec<String> {
    let mut titles: Vec<String> = app
        .list_notes(notes(), "")
        .unwrap()
        .into_iter()
        .map(|e| e.title)
        .collect();
    titles.sort();
    titles
}

/// A copy of the vault key, for tests that need to keep one.
fn same_key(key: &VaultKey) -> VaultKey {
    copy_key(key)
}

// ---- launch phases ----

#[tokio::test]
async fn launch_with_nothing_shows_setup() {
    let install = Install::new();
    let app = install.launch();
    let phase = app.phase();
    assert_eq!(phase.phase, PhaseName::Setup);
    assert!(phase.secure_store);
    assert!(!phase.upgrade);
    assert_eq!(code(app.list_notes(notes(), "")), Code::NotOpen);
    assert!(install.files().is_empty());
}

#[tokio::test]
async fn relaunch_opens_with_no_prompt() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    let id = app.create_note(notes()).unwrap();
    app.quit();

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::Open);
    assert_eq!(app.list_notes(notes(), "").unwrap()[0].id, id);
}

#[tokio::test]
async fn database_without_a_keychain_entry_asks_for_the_recovery_key() {
    let install = Install::new();
    let (app, key) = created(&install).await;
    app.create_note(notes()).unwrap();
    app.quit();
    install.creds.delete(Entry::VaultKey).unwrap();

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::NeedsRecoveryKey);
    assert!(app.phase().secure_store);
    app.open_with_recovery_key(&key).await.unwrap();
    assert_eq!(app.list_notes(notes(), "").unwrap().len(), 1);
    // With a secure store, the key is remembered again.
    assert!(install.has(Entry::VaultKey));
    app.quit();
    assert_eq!(install.launch().phase().phase, PhaseName::Open);
}

#[tokio::test]
async fn a_keychain_entry_for_another_vault_is_ignored() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    app.quit();
    credentials::save_vault_key(
        &*install.creds,
        VaultId::from_parts(1, 1),
        &VaultKey::generate().unwrap(),
    )
    .unwrap();
    assert_eq!(install.launch().phase().phase, PhaseName::NeedsRecoveryKey);
}

#[tokio::test]
async fn a_database_nothing_can_open_is_moved_aside() {
    let install = Install::new();
    // A version-one store with no credentials: an old install's leftovers.
    let vault = VaultId::from_parts(5, 5);
    let mut storage = SqliteStorage::open(install.db(), vault).unwrap();
    storage.set_setting("editor.vimMode", "true").unwrap();
    drop(storage);

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::Setup);
    let files = install.files();
    assert!(
        !files.contains(&vault::DATABASE_FILE.to_owned()),
        "{files:?}"
    );
    let moved = files
        .iter()
        .find(|f| f.starts_with("vault.sqlite3.legacy-") && !f.ends_with("-wal"))
        .unwrap();
    let kept = SqliteStorage::open(install.dir.path().join(moved), vault).unwrap();
    assert_eq!(
        kept.setting("editor.vimMode").unwrap().as_deref(),
        Some("true")
    );
}

// ---- creating a notebook ----

#[tokio::test]
async fn create_shows_a_key_and_opens_an_empty_notebook() {
    let install = Install::new();
    let app = install.launch();
    let key = app.create_notebook().await.unwrap();
    assert!(RecoveryKey::parse(&key).is_ok());
    assert_eq!(app.phase().phase, PhaseName::PendingRecoveryKey);
    assert_eq!(*app.pending_recovery_key().unwrap(), *key);
    assert_eq!(code(app.list_notes(notes(), "")), Code::NotOpen);
    assert_eq!(code(app.create_notebook().await), Code::NotOpen);

    app.acknowledge_recovery_key().await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::Open);
    assert!(app.list_notes(notes(), "").unwrap().is_empty());
    assert!(app.tag_tree().unwrap().is_empty());
    assert!(install.has(Entry::VaultKey));
    assert_eq!(phases(&install.events), ["pending_recovery_key", "open"]);

    // Never shown again.
    assert_eq!(code(app.pending_recovery_key()), Code::NotOpen);
    assert_eq!(code(app.acknowledge_recovery_key().await), Code::NotOpen);

    // The stored recovery entry opens with the key that was shown.
    let stored = SqliteStorage::peek(install.db()).unwrap().unwrap();
    let recovery = RecoveryKey::parse(&key).unwrap();
    let (vault, stored_key) = credentials::load_vault_key(&*install.creds)
        .unwrap()
        .unwrap();
    assert_eq!(stored.vault, vault);
    let opened = stored.recovery_entry.unwrap().open(&recovery).unwrap();
    assert_eq!(*opened.to_secret_bytes(), *stored_key.to_secret_bytes());
}

#[tokio::test]
async fn quitting_before_acknowledgment_leaves_nothing() {
    let install = Install::new();
    let app = install.launch();
    app.create_notebook().await.unwrap();
    app.quit();
    drop(app);

    assert!(install.files().is_empty());
    assert!(!install.has(Entry::VaultKey));
    assert_eq!(install.launch().phase().phase, PhaseName::Setup);
}

#[tokio::test]
async fn a_keychain_refusing_the_write_falls_back_to_the_recovery_key() {
    // Reads work but every write fails, as a keychain that locks does.
    let install = Install::with_store(MemoryStore::fails_after(0));
    let app = install.launch();
    let key = app.create_notebook().await.unwrap();
    // The refusal reads as a missing store, which is not an error: the
    // notebook opens and the key is asked for next time.
    app.acknowledge_recovery_key().await.unwrap();
    assert!(!app.phase().secure_store);
    app.quit();
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::NeedsRecoveryKey);
    app.open_with_recovery_key(&key).await.unwrap();
}

// ---- without a secure store ----

#[tokio::test]
async fn without_a_secure_store_the_recovery_key_opens_each_launch() {
    let install = Install::with_store(MemoryStore::unavailable());
    let app = install.launch();
    assert!(!app.phase().secure_store);
    let key = app.create_notebook().await.unwrap().to_string();
    app.acknowledge_recovery_key().await.unwrap();
    app.create_note(notes()).unwrap();
    app.quit();

    for _ in 0..2 {
        let app = install.launch();
        let phase = app.phase();
        assert_eq!(phase.phase, PhaseName::NeedsRecoveryKey);
        assert!(!phase.secure_store);
        app.open_with_recovery_key(&key).await.unwrap();
        assert_eq!(app.list_notes(notes(), "").unwrap().len(), 1);
        app.quit();
    }
    // Only the database, which holds no usable key.
    for name in install.files() {
        assert!(name.starts_with(vault::DATABASE_FILE), "{name}");
    }
}

#[tokio::test]
async fn wrong_and_mistyped_recovery_keys_differ() {
    let install = Install::with_store(MemoryStore::unavailable());
    let (app, key) = created(&install).await;
    app.quit();
    let app = install.launch();

    let other = RecoveryKey::generate().unwrap().to_display();
    assert_eq!(
        code(app.open_with_recovery_key(&other).await),
        Code::RecoveryKeyWrong
    );
    // One character changed fails the checksum.
    let mut typo: Vec<char> = key.chars().collect();
    typo[0] = if typo[0] == 'A' { 'B' } else { 'A' };
    let typo: String = typo.into_iter().collect();
    assert_eq!(
        code(app.open_with_recovery_key(&typo).await),
        Code::RecoveryKeyMalformed
    );
    assert_eq!(
        code(app.open_with_recovery_key("").await),
        Code::RecoveryKeyMalformed
    );
    assert_eq!(app.phase().phase, PhaseName::NeedsRecoveryKey);
    assert_eq!(code(app.list_notes(notes(), "")), Code::NotOpen);

    // Typed loosely, the right key still opens it.
    let loose = key.to_lowercase().replace('-', " ");
    app.open_with_recovery_key(&loose).await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::Open);
}

// ---- upgrade from a server-backed install ----

/// A database as a signed-in install of the server-backed app left it:
/// notes written through a replica (so some are still in the outbox), a
/// version-one-compatible layout, and its legacy credential entries.
struct Legacy {
    vault: VaultId,
    key: VaultKey,
    bodies: Vec<String>,
    outbox: usize,
}

fn seed_legacy(install: &Install, locked: bool) -> Legacy {
    let vault = VaultId::from_parts(1_700_000_000_000, 42);
    let key = VaultKey::generate().unwrap();
    let storage = SqliteStorage::open(install.db(), vault).unwrap();
    let replica = Shared::new(Replica::open(storage, same_key(&key)).unwrap());
    for body in [
        "Groceries\n#home",
        "Standup notes\n#work/acme",
        "Untitled idea",
    ] {
        replica.lock().create_note(body, now()).unwrap();
    }
    let mut bodies: Vec<String> = replica
        .lock()
        .notes()
        .iter()
        .map(|n| n.title().to_owned())
        .collect();
    let outbox = replica.lock().pending().unwrap();
    assert!(outbox > 0);
    drop(replica);

    legacy::save_session(&*install.creds, vault);
    let local = LocalUnlock::seal(vault, &key, OLD_PASSPHRASE).unwrap();
    legacy::save_local_unlock(&*install.creds, &local);
    if !locked {
        legacy::save_auto_unlock(&*install.creds, &key);
    }
    bodies.sort();
    Legacy {
        vault,
        key,
        bodies,
        outbox,
    }
}

fn assert_upgraded(install: &Install, app: &AppState, old: &Legacy) {
    assert_eq!(app.phase().phase, PhaseName::Open);
    assert_eq!(bodies(app), old.bodies);
    for entry in [Entry::Session, Entry::LocalUnlock, Entry::AutoUnlock] {
        assert!(!install.has(entry), "{entry:?}");
    }
    let (vault, key) = credentials::load_vault_key(&*install.creds)
        .unwrap()
        .unwrap();
    assert_eq!(vault, old.vault);
    assert_eq!(*key.to_secret_bytes(), *old.key.to_secret_bytes());
}

#[tokio::test]
async fn signed_in_install_upgrades_with_a_new_recovery_key() {
    let install = Install::new();
    let old = seed_legacy(&install, false);

    let app = install.launch();
    let phase = app.phase();
    assert_eq!(phase.phase, PhaseName::PendingRecoveryKey);
    assert!(phase.upgrade);
    let key = app.pending_recovery_key().unwrap();
    // Nothing is deleted before acknowledgment.
    assert!(install.has(Entry::Session));
    assert!(install.has(Entry::AutoUnlock));
    assert!(!install.has(Entry::VaultKey));

    app.acknowledge_recovery_key().await.unwrap();
    assert_upgraded(&install, &app, &old);
    // Pending outbox entries are left in place for `retire-sync-server`.
    app.with_vault(|open| {
        assert_eq!(
            open.workspace.replica().lock().pending().unwrap(),
            old.outbox
        );
        Ok(())
    })
    .unwrap();
    app.quit();

    // Later launches open with no prompt, and the new key opens a copy.
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::Open);
    assert_eq!(bodies(&app), old.bodies);
    let entry = SqliteStorage::peek(install.db())
        .unwrap()
        .unwrap()
        .recovery_entry
        .unwrap();
    assert!(entry.open(&RecoveryKey::parse(&key).unwrap()).is_ok());
}

#[tokio::test]
async fn quitting_an_upgrade_before_acknowledgment_runs_it_again() {
    let install = Install::new();
    let old = seed_legacy(&install, false);
    let app = install.launch();
    let first = app.pending_recovery_key().unwrap();
    app.quit();
    drop(app);
    assert!(install.has(Entry::Session));

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::PendingRecoveryKey);
    let second = app.pending_recovery_key().unwrap();
    assert_ne!(*first, *second);
    app.acknowledge_recovery_key().await.unwrap();
    assert_upgraded(&install, &app, &old);
}

#[tokio::test]
async fn locked_install_upgrades_with_the_old_passphrase() {
    let install = Install::new();
    let old = seed_legacy(&install, true);

    let app = install.launch();
    let phase = app.phase();
    assert_eq!(phase.phase, PhaseName::NeedsOldPassphrase);
    assert!(phase.upgrade);
    assert_eq!(
        code(app.upgrade_with_passphrase("not the passphrase").await),
        Code::WrongPassphrase
    );
    assert_eq!(app.phase().phase, PhaseName::NeedsOldPassphrase);

    app.upgrade_with_passphrase(OLD_PASSPHRASE).await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::PendingRecoveryKey);
    assert!(install.has(Entry::LocalUnlock));
    app.acknowledge_recovery_key().await.unwrap();
    assert_upgraded(&install, &app, &old);
    app.quit();

    // No passphrase is asked for again.
    assert_eq!(install.launch().phase().phase, PhaseName::Open);
}

#[tokio::test]
async fn starting_over_keeps_the_old_database() {
    let install = Install::new();
    let old = seed_legacy(&install, true);
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::NeedsOldPassphrase);

    app.start_over().await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::Setup);
    // The passphrase entry still opens the moved file if remembered later.
    assert!(install.has(Entry::Session));
    assert!(install.has(Entry::LocalUnlock));
    // Quitting here and relaunching shows the welcome screen, not the
    // passphrase prompt again.
    app.quit();
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::Setup);
    let files = install.files();
    assert!(
        !files.contains(&vault::DATABASE_FILE.to_owned()),
        "{files:?}"
    );
    let moved = files
        .iter()
        .find(|f| f.starts_with("vault.sqlite3.legacy-") && !f.contains("-wal"))
        .unwrap()
        .clone();

    // A new, empty notebook beside the old one.
    let key = app.create_notebook().await.unwrap();
    app.acknowledge_recovery_key().await.unwrap();
    assert!(app.list_notes(notes(), "").unwrap().is_empty());
    assert!(RecoveryKey::parse(&key).is_ok());
    app.quit();
    // The new notebook wins over the leftover legacy entries.
    assert_eq!(install.launch().phase().phase, PhaseName::Open);

    // The old database is intact and still opens with the old key.
    let storage = SqliteStorage::open(install.dir.path().join(moved), old.vault).unwrap();
    let replica = Replica::open(storage, same_key(&old.key)).unwrap();
    let mut kept: Vec<String> = replica
        .notes()
        .iter()
        .map(|n| n.title().to_owned())
        .collect();
    kept.sort();
    assert_eq!(kept, old.bodies);
}

#[tokio::test]
async fn upgrade_with_neither_key_entry_starts_over() {
    let install = Install::new();
    seed_legacy(&install, true);
    install.creds.delete(Entry::LocalUnlock).unwrap();

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::Setup);
    assert!(
        install
            .files()
            .iter()
            .any(|f| f.starts_with("vault.sqlite3.legacy-"))
    );
    // Entries are never deleted without an acknowledged upgrade, and a
    // relaunch doesn't try to upgrade the moved file again.
    assert!(install.has(Entry::Session));
    app.quit();
    assert_eq!(install.launch().phase().phase, PhaseName::Setup);
}

#[tokio::test]
async fn an_unreadable_session_entry_keeps_the_legacy_key() {
    let install = Install::new();
    let old = seed_legacy(&install, false);
    install.creds.set(Entry::Session, b"not json").unwrap();

    // Nothing is moved or deleted; the user can choose to start over.
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::NeedsRecoveryKey);
    assert!(install.files().contains(&vault::DATABASE_FILE.to_owned()));
    app.start_over().await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::Setup);
    let key = credentials::load_legacy_auto_unlock(&*install.creds)
        .unwrap()
        .unwrap();
    assert_eq!(*key.to_secret_bytes(), *old.key.to_secret_bytes());
}

#[tokio::test]
async fn a_keychain_denied_at_launch_changes_nothing() {
    let install = Install::new();
    let old = seed_legacy(&install, false);

    // One launch where the keychain refuses every read, as after a denied
    // macOS prompt.
    let denied = AppState::launch(
        AppConfig {
            data_dir: install.dir.path().to_owned(),
        },
        Arc::new(MemoryStore::unavailable()),
        install.events.clone(),
    );
    assert_eq!(denied.phase().phase, PhaseName::NeedsRecoveryKey);
    assert!(!denied.phase().secure_store);
    drop(denied);
    assert!(install.files().contains(&vault::DATABASE_FILE.to_owned()));

    // Allowed next time: the upgrade proceeds with every note.
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::PendingRecoveryKey);
    app.acknowledge_recovery_key().await.unwrap();
    assert_upgraded(&install, &app, &old);
}

#[tokio::test]
async fn an_unreadable_database_can_be_started_over() {
    let install = Install::new();
    std::fs::write(install.db(), b"not a database at all, just some bytes").unwrap();

    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::NeedsRecoveryKey);
    let key = RecoveryKey::generate().unwrap().to_display();
    assert_eq!(code(app.open_with_recovery_key(&key).await), Code::Internal);
    app.start_over().await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::Setup);
    assert!(
        install
            .files()
            .iter()
            .any(|f| f.starts_with("vault.sqlite3.legacy-"))
    );
    app.create_notebook().await.unwrap();
    app.acknowledge_recovery_key().await.unwrap();
}

#[tokio::test]
async fn a_failed_upgrade_acknowledgment_keeps_everything_and_retries() {
    let install = Install::new();
    let old = seed_legacy(&install, false);
    let app = install.launch();
    assert_eq!(app.phase().phase, PhaseName::PendingRecoveryKey);

    // The database is replaced by one for another vault behind the app's
    // back, so acknowledging can't open it.
    let path = install.db();
    let moved = path.with_extension("bak");
    std::fs::rename(&path, &moved).unwrap();
    drop(SqliteStorage::open(&path, VaultId::from_parts(9, 9)).unwrap());
    assert_eq!(code(app.acknowledge_recovery_key().await), Code::Internal);
    assert_eq!(app.phase().phase, PhaseName::PendingRecoveryKey);
    for entry in [Entry::Session, Entry::LocalUnlock, Entry::AutoUnlock] {
        assert!(install.has(entry), "{entry:?}");
    }
    assert!(!install.has(Entry::VaultKey));

    // Put back, the same acknowledgment succeeds.
    SqliteStorage::delete_database(&path).unwrap();
    std::fs::rename(&moved, &path).unwrap();
    app.acknowledge_recovery_key().await.unwrap();
    assert_upgraded(&install, &app, &old);
}

#[tokio::test]
async fn an_upgrade_whose_database_wont_open_can_start_over() {
    let install = Install::new();
    seed_legacy(&install, false);
    // A database that peeks as the right vault but won't open: here, one
    // layout version newer than this build knows.
    let path = install.db();
    let mut bytes = std::fs::read(&path).unwrap();
    // `user_version` is the big-endian u32 at offset 60 of the header.
    bytes[60..64].copy_from_slice(&99u32.to_be_bytes());
    std::fs::write(&path, bytes).unwrap();

    for _ in 0..2 {
        let app = install.launch();
        assert_eq!(app.phase().phase, PhaseName::PendingRecoveryKey);
        assert_eq!(code(app.acknowledge_recovery_key().await), Code::Internal);
        app.quit();
    }
    let app = install.launch();
    app.start_over().await.unwrap();
    assert_eq!(app.phase().phase, PhaseName::Setup);
    assert!(install.has(Entry::AutoUnlock));
    assert!(
        install
            .files()
            .iter()
            .any(|f| f.starts_with("vault.sqlite3.legacy-"))
    );

    // Not for a new notebook's key screen, which has nothing to set aside.
    app.create_notebook().await.unwrap();
    assert_eq!(code(app.start_over().await), Code::NotOpen);
}

#[tokio::test]
async fn upgrade_commands_need_their_phase() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    assert_eq!(
        code(app.upgrade_with_passphrase(OLD_PASSPHRASE).await),
        Code::NotOpen
    );
    assert_eq!(code(app.start_over().await), Code::NotOpen);
    let key = RecoveryKey::generate().unwrap().to_display();
    assert_eq!(code(app.open_with_recovery_key(&key).await), Code::NotOpen);
}

// ---- settings ----

#[tokio::test]
async fn collapsed_tags_are_sealed_at_rest() {
    let install = Install::new();
    let (app, _) = created(&install).await;
    app.set_tag_expanded("Secret-Project", false).unwrap();
    assert_eq!(app.settings().unwrap().collapsed_tags, ["secret-project"]);
    app.quit();

    for name in install.files() {
        let bytes = std::fs::read(install.dir.path().join(&name)).unwrap();
        assert!(!bytes.windows(14).any(|w| w == b"secret-project"), "{name}");
    }
    let app = install.launch();
    assert_eq!(app.settings().unwrap().collapsed_tags, ["secret-project"]);
}
