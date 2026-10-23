//! The app state machine: no notebook yet, a recovery key waiting to be
//! acknowledged, a notebook waiting for its key, or open.
//!
//! All decrypted state, keys, and storage live here; the webview asks for
//! what it renders. Phase changes (create, acknowledge, open, upgrade) are
//! serialized by one async lock and never hold the phase lock across slow
//! work, so vault commands stay responsive. Nothing here uses the network.
//!
//! Launch picks the phase in this order:
//!
//! 1. The `vault-key` entry is present and the database holds that vault:
//!    open.
//! 2. A legacy `session` entry and a database are present: upgrade a
//!    server-backed install (see [`AppState::upgrade_with_passphrase`]).
//!    Legacy entries are deleted only once the upgrade is acknowledged.
//! 3. A database with a recovery entry: ask for the recovery key.
//! 4. Otherwise: set up a new notebook. A database that nothing can open is
//!    first moved aside, never deleted.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use notebook_core::crypto::{CryptoError, RecoveryEntry, RecoveryKey, VaultId, VaultKey};
use notebook_core::note::Timestamp;
use notebook_core::storage::Storage;
use notebook_core::workspace::{Changed, Workspace};
use notebook_storage_sqlite::SqliteStorage;
use zeroize::Zeroizing;

use crate::credentials::{self, CredentialError, CredentialStore};
use crate::dto::{PhaseDto, PhaseName, SettingsDto, VaultChangedDto};
use crate::error::{AppError, AppResult, Code};
use crate::events::{self, Emitter};
use crate::vault;

/// Device setting keys. Values are not sensitive.
pub const VIM_MODE_KEY: &str = "editor.vimMode";
pub const THEME_KEY: &str = "appearance.theme";
pub const SOURCE_VIEW_KEY: &str = "editor.sourceView";
pub const FOCUS_MODE_KEY: &str = "editor.focusMode";
const THEMES: [&str; 3] = ["system", "light", "dark"];
const COLLAPSED_TAGS_KEY: &str = "sidebar.collapsedTags";

pub struct AppConfig {
    /// Where the vault database lives (`app_data_dir()` in the app).
    pub data_dir: PathBuf,
    /// Run automatic backups in the background while open. Off in tests,
    /// which drive [`AppState::backup_tick`] with their own clock.
    pub backup_schedule: bool,
}

/// The app state, shared by every command. Cheap to clone.
#[derive(Clone)]
pub struct AppState(pub(crate) Arc<Core>);

pub(crate) struct Core {
    pub(crate) config: AppConfig,
    creds: Arc<dyn CredentialStore>,
    /// Cleared the first time the OS store turns out to be unusable.
    secure_store: AtomicBool,
    pub(crate) emitter: Arc<dyn Emitter>,
    phase: Mutex<Phase>,
    pub(crate) ops: tokio::sync::Mutex<()>,
    pub(crate) backups: crate::backup::Backups,
    next_generation: AtomicU64,
    /// Editor session numbers, never reused within a run, so a stale editor
    /// can't reach another note.
    next_editor: AtomicU64,
}

pub(crate) enum Phase {
    Setup,
    PendingRecoveryKey(Box<Pending>),
    NeedsRecoveryKey,
    NeedsOldPassphrase(VaultId),
    Open(Box<OpenVault>),
}

/// A vault key and its new recovery key, shown but not yet acknowledged.
/// Nothing about it has been written anywhere.
pub(crate) struct Pending {
    vault: VaultId,
    key: VaultKey,
    recovery: RecoveryKey,
    /// Converting a server-backed install rather than creating a notebook.
    upgrade: bool,
}

pub(crate) struct OpenVault {
    /// Distinguishes this opening from later ones, so host tasks of an old
    /// vault never touch a new one.
    pub(crate) generation: u64,
    /// For building backups off the replica lock.
    pub(crate) vault_key: VaultKey,
    pub(crate) workspace: Workspace<SqliteStorage>,
    /// Editor sessions by the id the frontend knows them by.
    pub(crate) editors: crate::editor::Editors,
}

pub fn now() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    Timestamp(i64::try_from(ms).unwrap_or(i64::MAX))
}

fn new_vault_id() -> AppResult<VaultId> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(AppError::internal)?;
    let time = u64::try_from(now().as_millis()).unwrap_or(0);
    Ok(VaultId::from_parts(time, u128::from_be_bytes(random)))
}

/// A recovery key as typed; one that fails its checksum is
/// `recovery_key_malformed`.
pub(crate) fn parse_recovery_key(typed: &str) -> AppResult<RecoveryKey> {
    RecoveryKey::parse(typed).map_err(|_| {
        AppError::new(
            Code::RecoveryKeyMalformed,
            "the recovery key looks mistyped",
        )
    })
}

pub(crate) fn wrong_phase() -> AppError {
    AppError::new(Code::NotOpen, "not possible at this point")
}

/// Runs slow work (Argon2id, backup files) off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(AppError::internal)
}

impl AppState {
    /// Builds the state and picks the launch phase (see the module docs).
    pub fn launch(
        config: AppConfig,
        creds: Arc<dyn CredentialStore>,
        emitter: Arc<dyn Emitter>,
    ) -> Self {
        let state = Self(Arc::new(Core {
            config,
            creds,
            secure_store: AtomicBool::new(true),
            emitter,
            phase: Mutex::new(Phase::Setup),
            ops: tokio::sync::Mutex::new(()),
            backups: Default::default(),
            next_generation: AtomicU64::new(0),
            next_editor: AtomicU64::new(0),
        }));
        state.0.restore();
        state
    }

    pub(crate) fn next_editor_id(&self) -> u64 {
        self.0.next_editor.fetch_add(1, Ordering::SeqCst)
    }

    pub fn phase(&self) -> PhaseDto {
        self.0.phase_dto()
    }

    pub(crate) fn lock_phase(&self) -> MutexGuard<'_, Phase> {
        self.0.lock_phase()
    }

    /// Runs `f` on the open vault, or fails with `not_open`.
    pub(crate) fn with_vault<T>(
        &self,
        f: impl FnOnce(&mut OpenVault) -> AppResult<T>,
    ) -> AppResult<T> {
        match &mut *self.lock_phase() {
            Phase::Open(open) => f(open),
            _ => Err(AppError::not_open()),
        }
    }

    /// Generates a new notebook's vault id, vault key, and recovery key in
    /// memory and returns the recovery key's display form. Nothing is
    /// written until [`acknowledge_recovery_key`](Self::acknowledge_recovery_key).
    pub async fn create_notebook(&self) -> AppResult<Zeroizing<String>> {
        let _ops = self.0.ops.lock().await;
        if !matches!(*self.lock_phase(), Phase::Setup) {
            return Err(wrong_phase());
        }
        let pending = Pending {
            vault: new_vault_id()?,
            key: VaultKey::generate().map_err(AppError::internal)?,
            recovery: RecoveryKey::generate().map_err(AppError::internal)?,
            upgrade: false,
        };
        let display = pending.recovery.to_display();
        *self.lock_phase() = Phase::PendingRecoveryKey(Box::new(pending));
        self.0.emit_phase();
        Ok(display)
    }

    /// The recovery key waiting to be acknowledged. Only available until it
    /// is, so no screen can show it after that.
    pub fn pending_recovery_key(&self) -> AppResult<Zeroizing<String>> {
        match &*self.lock_phase() {
            Phase::PendingRecoveryKey(pending) => Ok(pending.recovery.to_display()),
            _ => Err(wrong_phase()),
        }
    }

    /// The user saved the recovery key: writes the database with the
    /// recovery entry, stores the vault key, deletes legacy entries of an
    /// upgrade (only now, so quitting earlier just runs the upgrade again),
    /// and opens the notebook.
    pub async fn acknowledge_recovery_key(&self) -> AppResult<PhaseDto> {
        let _ops = self.0.ops.lock().await;
        let (vault, key, entry, upgrade) = match &*self.lock_phase() {
            Phase::PendingRecoveryKey(p) => (
                p.vault,
                copy_key(&p.key),
                RecoveryEntry::create(p.vault, &p.key, &p.recovery).map_err(AppError::internal)?,
                p.upgrade,
            ),
            _ => return Err(wrong_phase()),
        };
        let mut storage = vault::open_storage(&self.0.db_path(), vault)?;
        storage.set_recovery_entry(&entry)?;
        self.0.remember(vault, &key)?;
        if upgrade && let Err(err) = credentials::delete_legacy(&*self.0.creds) {
            tracing::warn!(%err, "couldn't delete the legacy credential entries");
        }
        self.0.open_vault(storage, key)?;
        Ok(self.phase())
    }

    /// Opens the notebook with its recovery key, when the vault key isn't in
    /// the secure store. A key that fails its checksum is
    /// `recovery_key_malformed`; one that doesn't open this notebook is
    /// `recovery_key_wrong`.
    pub async fn open_with_recovery_key(&self, recovery_key: &str) -> AppResult<PhaseDto> {
        let recovery = parse_recovery_key(recovery_key)?;
        let _ops = self.0.ops.lock().await;
        if !matches!(*self.lock_phase(), Phase::NeedsRecoveryKey) {
            return Err(wrong_phase());
        }
        let path = self.0.db_path();
        let stored = SqliteStorage::peek(&path)?
            .ok_or_else(|| AppError::internal("the notebook's database is gone"))?;
        let entry = stored
            .recovery_entry
            .ok_or_else(|| AppError::internal("the notebook has no recovery entry"))?;
        if entry.vault_id() != stored.vault {
            return Err(AppError::internal(
                "the recovery entry belongs to another notebook",
            ));
        }
        let key = entry.open(&recovery).map_err(|err| match err {
            CryptoError::Decrypt => AppError::new(
                Code::RecoveryKeyWrong,
                "the recovery key doesn't open this notebook",
            ),
            err => AppError::internal(err),
        })?;
        let storage = vault::open_storage(&path, stored.vault)?;
        // The secure store may work again (or for the first time); if not,
        // the key is asked for at the next launch too.
        if let Err(err) = self.0.remember(stored.vault, &key) {
            tracing::warn!(%err, "couldn't store the vault key");
        }
        self.0.open_vault(storage, key)?;
        Ok(self.phase())
    }

    /// Upgrades a locked server-backed install: the old passphrase opens the
    /// device-local wrap of its vault key (Argon2id, off the async threads),
    /// and a new recovery key is generated as for a new notebook.
    pub async fn upgrade_with_passphrase(&self, passphrase: &str) -> AppResult<PhaseDto> {
        let _ops = self.0.ops.lock().await;
        let Phase::NeedsOldPassphrase(vault) = *self.lock_phase() else {
            return Err(wrong_phase());
        };
        let local = credentials::load_legacy_local_unlock(&*self.0.creds, vault)?
            .ok_or_else(|| AppError::internal("the passphrase entry is gone"))?;
        let passphrase = Zeroizing::new(passphrase.to_owned());
        let key = blocking(move || local.open(&passphrase))
            .await?
            .map_err(|_| AppError::new(Code::WrongPassphrase, "the passphrase is incorrect"))?;
        *self.lock_phase() = self.0.begin_upgrade(vault, key)?;
        self.0.emit_phase();
        Ok(self.phase())
    }

    /// Gives up on the notebook on this device, when its old passphrase or
    /// recovery key is lost or it can't be read (an upgrade whose database
    /// won't open included): moves the database aside
    /// under a new name (never deleting it) and shows the welcome screen.
    /// Credential entries stay, so a key remembered later still opens the
    /// old file.
    pub async fn start_over(&self) -> AppResult<PhaseDto> {
        let _ops = self.0.ops.lock().await;
        // Also during an upgrade, whose database may turn out unopenable only
        // at acknowledgment.
        let allowed = match &*self.lock_phase() {
            Phase::NeedsOldPassphrase(_) | Phase::NeedsRecoveryKey => true,
            Phase::PendingRecoveryKey(pending) => pending.upgrade,
            Phase::Setup | Phase::Open(_) => false,
        };
        if !allowed {
            return Err(wrong_phase());
        }
        self.0.set_aside()?;
        *self.lock_phase() = Phase::Setup;
        self.0.emit_phase();
        Ok(self.phase())
    }

    pub fn settings(&self) -> AppResult<SettingsDto> {
        match &*self.lock_phase() {
            Phase::Open(open) => {
                let replica = open.workspace.replica().lock();
                let storage = replica.storage()?;
                let on = |key| -> AppResult<bool> {
                    Ok(storage.setting(key)?.as_deref() == Some("true"))
                };
                let theme = storage
                    .setting(THEME_KEY)?
                    .filter(|t| THEMES.contains(&t.as_str()))
                    .unwrap_or_else(|| THEMES[0].to_owned());
                // Tag names are vault content, so they are sealed at rest.
                let collapsed_tags = replica
                    .sealed_setting(COLLAPSED_TAGS_KEY)?
                    .and_then(|v| serde_json::from_slice(&v).ok())
                    .unwrap_or_default();
                Ok(SettingsDto {
                    vim_mode: on(VIM_MODE_KEY)?,
                    theme,
                    source_view: on(SOURCE_VIEW_KEY)?,
                    focus_mode: on(FOCUS_MODE_KEY)?,
                    collapsed_tags,
                })
            }
            _ => Ok(SettingsDto {
                vim_mode: false,
                theme: THEMES[0].to_owned(),
                source_view: false,
                focus_mode: false,
                collapsed_tags: Vec::new(),
            }),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.with_vault(|open| Ok(open.workspace.replica().lock().set_setting(key, value)?))
    }

    pub fn set_vim_mode(&self, on: bool) -> AppResult<()> {
        self.set_setting(VIM_MODE_KEY, if on { "true" } else { "false" })
    }

    /// Stores one of the UI preferences the frontend owns (theme, source
    /// view, focus mode). Other keys and malformed values are refused.
    pub fn set_preference(&self, key: &str, value: &str) -> AppResult<()> {
        let valid = match key {
            THEME_KEY => THEMES.contains(&value),
            SOURCE_VIEW_KEY | FOCUS_MODE_KEY => matches!(value, "true" | "false"),
            _ => false,
        };
        if !valid {
            return Err(AppError::new(
                Code::Internal,
                format!("unsupported preference {key}={value}"),
            ));
        }
        self.set_setting(key, value)
    }

    /// Remembers whether a tag is expanded in the sidebar.
    pub fn set_tag_expanded(&self, path: &str, expanded: bool) -> AppResult<()> {
        let key = path.to_lowercase();
        let mut collapsed = self.settings()?.collapsed_tags;
        collapsed.retain(|p| *p != key);
        if !expanded {
            collapsed.push(key);
            collapsed.sort();
        }
        let value = Zeroizing::new(serde_json::to_vec(&collapsed).expect("strings serialize"));
        self.with_vault(|open| {
            open.workspace
                .replica()
                .lock()
                .set_sealed_setting(COLLAPSED_TAGS_KEY, &value)
                .map_err(AppError::internal)
        })
    }
}

pub(crate) fn copy_key(key: &VaultKey) -> VaultKey {
    VaultKey::from_secret_bytes(&*key.to_secret_bytes()).expect("a vault key is 32 bytes")
}

impl Core {
    pub(crate) fn lock_phase(&self) -> MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn db_path(&self) -> PathBuf {
        vault::database_path(&self.config.data_dir)
    }

    fn phase_dto(&self) -> PhaseDto {
        let phase = self.lock_phase();
        let (name, upgrade) = match &*phase {
            Phase::Setup => (PhaseName::Setup, false),
            Phase::PendingRecoveryKey(p) => (PhaseName::PendingRecoveryKey, p.upgrade),
            Phase::NeedsRecoveryKey => (PhaseName::NeedsRecoveryKey, false),
            Phase::NeedsOldPassphrase(_) => (PhaseName::NeedsOldPassphrase, true),
            Phase::Open(_) => (PhaseName::Open, false),
        };
        PhaseDto {
            phase: name,
            secure_store: self.secure_store.load(Ordering::SeqCst),
            upgrade,
        }
    }

    pub(crate) fn emit_phase(&self) {
        events::emit(&*self.emitter, events::APP_PHASE_CHANGED, &self.phase_dto());
    }

    /// Notes that the OS store is unusable when `result` says so.
    fn note_store<T>(&self, result: &Result<T, CredentialError>) {
        if matches!(result, Err(CredentialError::NoSecureStore))
            && self.secure_store.swap(false, Ordering::SeqCst)
        {
            tracing::warn!("no secure credential store; the vault key won't be remembered");
        }
    }

    /// Stores the vault key in the OS store. Without one, it is simply not
    /// stored (never written to a file), and that is not an error.
    pub(crate) fn remember(&self, vault: VaultId, key: &VaultKey) -> AppResult<()> {
        let result = credentials::save_vault_key(&*self.creds, vault, key);
        self.note_store(&result);
        match result {
            Ok(()) | Err(CredentialError::NoSecureStore) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Picks the launch phase (see the module docs).
    fn restore(self: &Arc<Self>) {
        let path = self.db_path();

        let stored_key = credentials::load_vault_key(&*self.creds);
        self.note_store(&stored_key);
        match stored_key {
            Ok(Some((vault, key))) if path.exists() => match vault::open_storage(&path, vault) {
                Ok(storage) => match self.open_vault(storage, key) {
                    Ok(()) => return,
                    Err(err) => tracing::warn!(%err, "couldn't open the notebook at launch"),
                },
                Err(err) => tracing::warn!(%err, "the stored vault key doesn't fit the database"),
            },
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "can't read the vault key entry"),
        }

        let peeked = SqliteStorage::peek(&path);
        let legacy = credentials::load_legacy_session(&*self.creds);
        self.note_store(&legacy);
        let phase = match (legacy, peeked) {
            (_, Ok(None)) => Phase::Setup,
            (Ok(Some(vault)), Ok(Some(stored))) if stored.vault == vault => {
                self.restore_legacy(vault)
            }
            (_, Ok(Some(stored))) if stored.recovery_entry.is_some() => Phase::NeedsRecoveryKey,
            // A database no key can open: an old install without its
            // credentials. Moved aside only when the keychain answered
            // that there is nothing for it.
            (Ok(None), Ok(Some(_))) => self.set_aside_or(Phase::NeedsRecoveryKey),
            // The keychain or the database can't be read right now (a denied
            // prompt, a busy file, a damaged entry): change nothing, so the
            // next launch tries again. The prompt offers starting over.
            (legacy, peeked) => {
                if let Err(err) = legacy {
                    tracing::warn!(%err, "can't read the legacy session entry");
                }
                if let Err(err) = peeked {
                    tracing::warn!(%err, "can't read the database at launch");
                }
                Phase::NeedsRecoveryKey
            }
        };
        *self.lock_phase() = phase;
    }

    /// Picks how to upgrade a server-backed install of `vault`, whose
    /// database is present: with its stored vault key, with its old
    /// passphrase, or, when the keychain says it has neither, by starting
    /// over. Any read error changes nothing.
    fn restore_legacy(&self, vault: VaultId) -> Phase {
        let auto = credentials::load_legacy_auto_unlock(&*self.creds);
        self.note_store(&auto);
        let local = credentials::load_legacy_local_unlock(&*self.creds, vault);
        self.note_store(&local);
        match (auto, local) {
            (Ok(Some(key)), _) => match self.begin_upgrade(vault, key) {
                Ok(pending) => pending,
                Err(err) => {
                    tracing::warn!(%err, "couldn't start the upgrade");
                    Phase::NeedsRecoveryKey
                }
            },
            (_, Ok(Some(_))) => Phase::NeedsOldPassphrase(vault),
            (Ok(None), Ok(None)) => self.set_aside_or(Phase::NeedsRecoveryKey),
            (auto, local) => {
                for err in [auto.err(), local.err()].into_iter().flatten() {
                    tracing::warn!(%err, "can't read a legacy credential entry");
                }
                Phase::NeedsRecoveryKey
            }
        }
    }

    /// Moves the database aside and returns `Setup`, or `fallback` if it
    /// couldn't be moved.
    fn set_aside_or(&self, fallback: Phase) -> Phase {
        match self.set_aside() {
            Ok(()) => Phase::Setup,
            Err(err) => {
                tracing::warn!(%err, "couldn't move the old database aside");
                fallback
            }
        }
    }

    /// Shows a new recovery key for the upgrade. The database isn't touched
    /// until acknowledgment, which reports (and can retry) a failure to open
    /// it, so a transient error never costs the legacy key entries.
    fn begin_upgrade(&self, vault: VaultId, key: VaultKey) -> AppResult<Phase> {
        Ok(Phase::PendingRecoveryKey(Box::new(Pending {
            vault,
            key,
            recovery: RecoveryKey::generate().map_err(AppError::internal)?,
            upgrade: true,
        })))
    }

    /// Moves the database aside. Credential entries are left alone: legacy
    /// ones may be the only key to the moved file, and they are deleted only
    /// when an upgrade is acknowledged.
    fn set_aside(&self) -> AppResult<()> {
        if let Some(moved) = vault::set_aside(&self.db_path(), now().as_millis())? {
            tracing::info!(path = %moved.display(), "moved the old database aside");
        }
        Ok(())
    }

    /// Opens the replica and workspace over `storage`, starts the host task
    /// that forwards changes and the backup scheduler, and switches to open.
    pub(crate) fn open_vault(
        self: &Arc<Self>,
        storage: SqliteStorage,
        key: VaultKey,
    ) -> AppResult<()> {
        let vault_key = copy_key(&key);
        let (workspace, changes) = vault::open(storage, key)?;
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        self.backups.opened();
        *self.lock_phase() = Phase::Open(Box::new(OpenVault {
            generation,
            vault_key,
            workspace,
            editors: Default::default(),
        }));
        self.spawn_changes(generation, changes);
        if self.config.backup_schedule {
            crate::backup::spawn_scheduler(Arc::downgrade(self), generation);
        }
        self.emit_phase();
        Ok(())
    }

    fn spawn_changes(
        self: &Arc<Self>,
        generation: u64,
        mut changes: futures::channel::mpsc::UnboundedReceiver<Changed>,
    ) {
        let core: Weak<Self> = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            while let Some(changed) = changes.next().await {
                let Some(core) = core.upgrade() else { break };
                core.on_changed(generation, &changed);
            }
        });
    }

    fn on_changed(&self, generation: u64, changed: &Changed) {
        let mut phase = self.lock_phase();
        let Phase::Open(open) = &mut *phase else {
            return;
        };
        if open.generation != generation {
            return;
        }
        self.backups.changed(open);
        events::emit(
            &*self.emitter,
            events::VAULT_CHANGED,
            &VaultChangedDto::from(changed),
        );
        crate::editor::forward_changes(&*self.emitter, open, changed);
    }
}

#[cfg(test)]
pub(crate) mod tests;
