//! Encrypted backups into a folder the user picks: settings, status, the
//! scheduler, Back up now, and restore.
//!
//! The file format is [`notebook_core::backup`], and writing and pruning
//! are [`backup::folder`]. The pickers run here, not in the webview, so the
//! webview never names a path that is read or written.
//!
//! Settings are device-local and not sensitive: `backup.folder`,
//! `backup.auto`, `backup.interval`, `backup.keep`, `backup.last`,
//! `backup.last_error`, and `backup.dirty`. An empty value means unset.
//! Changes since the last backup are tracked in memory and written to
//! `backup.dirty` at most once per scheduler tick.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use notebook_core::backup::{self, BackupError, BackupHeader, BackupNote, OpenedBackup};
use notebook_core::crypto::{RecoveryEntry, VaultId, VaultKey};
use notebook_core::note::Timestamp;
use notebook_core::storage::Storage;
use notebook_storage_sqlite::SqliteStorage;
use serde::{Deserialize, Serialize};

use crate::dto::{BackupStatusDto, PhaseDto, PickedBackupDto};
use crate::error::{AppError, AppResult, Code};
use crate::events;
use crate::state::{
    self, AppState, Core, FOCUS_MODE_KEY, OpenVault, Phase, SOURCE_VIEW_KEY, THEME_KEY,
    VIM_MODE_KEY, blocking, copy_key, now, wrong_phase,
};
use crate::vault;

pub const FOLDER_KEY: &str = "backup.folder";
pub const AUTO_KEY: &str = "backup.auto";
pub const INTERVAL_KEY: &str = "backup.interval";
pub const KEEP_KEY: &str = "backup.keep";
pub const LAST_KEY: &str = "backup.last";
pub const LAST_ERROR_KEY: &str = "backup.last_error";
pub const DIRTY_KEY: &str = "backup.dirty";

/// Device settings a restore carries over to the restored notebook.
const CARRIED_SETTINGS: [&str; 8] = [
    FOLDER_KEY,
    AUTO_KEY,
    INTERVAL_KEY,
    KEEP_KEY,
    VIM_MODE_KEY,
    THEME_KEY,
    SOURCE_VIEW_KEY,
    FOCUS_MODE_KEY,
];

const DEFAULT_KEEP: usize = 14;
/// How often the scheduler checks whether a backup is due.
const TICK: Duration = Duration::from_secs(10 * 60);

/// How often automatic backups run, at most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interval {
    Hourly,
    Daily,
    Weekly,
}

impl Interval {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [Self::Hourly, Self::Daily, Self::Weekly]
            .into_iter()
            .find(|i| i.as_str() == value)
    }

    fn millis(self) -> i64 {
        const HOUR: i64 = 3_600_000;
        match self {
            Self::Hourly => HOUR,
            Self::Daily => 24 * HOUR,
            Self::Weekly => 7 * 24 * HOUR,
        }
    }
}

/// Asks the user for the backup folder and for a backup to restore.
pub trait BackupPicker: Send + Sync {
    /// The chosen folder, or `None` when the user cancels.
    fn pick_folder(&self) -> BoxFuture<'static, Option<PathBuf>>;
    /// The chosen backup file, or `None` when the user cancels.
    fn pick_file(&self) -> BoxFuture<'static, Option<PathBuf>>;
}

/// The picker the backup commands use, as managed state.
#[derive(Clone)]
pub struct Pickers(pub Arc<dyn BackupPicker>);

/// The system pickers, through `tauri-plugin-dialog`.
#[cfg(desktop)]
pub struct DialogPicker<R: tauri::Runtime>(pub tauri::AppHandle<R>);

#[cfg(desktop)]
impl<R: tauri::Runtime> BackupPicker for DialogPicker<R> {
    fn pick_folder(&self) -> BoxFuture<'static, Option<PathBuf>> {
        use futures::FutureExt;
        use tauri_plugin_dialog::DialogExt;

        let (tx, rx) = futures::channel::oneshot::channel();
        self.0
            .dialog()
            .file()
            .set_title("Choose the backup folder")
            .pick_folder(move |folder| {
                let _ = tx.send(folder.and_then(|f| f.into_path().ok()));
            });
        rx.map(|picked| picked.ok().flatten()).boxed()
    }

    fn pick_file(&self) -> BoxFuture<'static, Option<PathBuf>> {
        use futures::FutureExt;
        use tauri_plugin_dialog::DialogExt;

        let (tx, rx) = futures::channel::oneshot::channel();
        self.0
            .dialog()
            .file()
            .set_title("Restore from backup")
            .add_filter("Notebook backup", &[backup::BACKUP_EXTENSION])
            .pick_file(move |file| {
                let _ = tx.send(file.and_then(|f| f.into_path().ok()));
            });
        rx.map(|picked| picked.ok().flatten()).boxed()
    }
}

/// Backup state that lives with the app rather than the vault.
#[derive(Default)]
pub(crate) struct Backups {
    /// Held while a backup is written, so two never overlap, and across a
    /// restore's swap.
    writing: tokio::sync::Mutex<()>,
    running: AtomicBool,
    /// Notes changed since the last backup's snapshot was taken.
    changed: AtomicBool,
    /// The backup chosen for the next `restore_backup`.
    picked: Mutex<Option<Picked>>,
}

impl Backups {
    /// A vault was opened: changes to the previous one don't count.
    /// A pick never outlives the notebook it was made in.
    pub(crate) fn opened(&self) {
        self.changed.store(false, Ordering::SeqCst);
        *self.picked() = None;
    }

    /// Notes of `open` changed. The first change since the last backup is
    /// written to `backup.dirty` right away, so it survives a quit before
    /// the next scheduler tick.
    pub(crate) fn changed(&self, open: &OpenVault) {
        if !self.changed.swap(true, Ordering::SeqCst)
            && let Err(err) = open
                .workspace
                .replica()
                .lock()
                .set_setting(DIRTY_KEY, "true")
        {
            tracing::warn!(%err, "couldn't record a change for backup");
        }
    }

    fn picked(&self) -> MutexGuard<'_, Option<Picked>> {
        self.picked.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A backup file the user picked: its bytes (ciphertext) and header.
#[derive(Clone)]
struct Picked {
    bytes: Arc<Vec<u8>>,
    header: BackupHeader,
}

/// The `backup.*` settings.
struct Config {
    folder: Option<PathBuf>,
    auto: bool,
    interval: Interval,
    keep: usize,
    last: Option<Timestamp>,
    last_error: Option<String>,
    dirty: bool,
}

impl Config {
    fn read(storage: &SqliteStorage) -> AppResult<Self> {
        let get = |key| -> AppResult<Option<String>> {
            Ok(storage.setting(key)?.filter(|v| !v.is_empty()))
        };
        Ok(Self {
            folder: get(FOLDER_KEY)?.map(PathBuf::from),
            auto: get(AUTO_KEY)?.as_deref() != Some("false"),
            interval: get(INTERVAL_KEY)?
                .and_then(|v| Interval::parse(&v))
                .unwrap_or(Interval::Daily),
            keep: get(KEEP_KEY)?
                .and_then(|v| v.parse().ok())
                .filter(|keep| *keep >= 1)
                .unwrap_or(DEFAULT_KEEP),
            last: get(LAST_KEY)?.and_then(|v| v.parse().ok()).map(Timestamp),
            last_error: get(LAST_ERROR_KEY)?,
            dirty: get(DIRTY_KEY)?.as_deref() == Some("true"),
        })
    }

    fn scheduled(&self) -> bool {
        self.folder.is_some() && self.auto
    }

    /// Whether the scheduler should back up at `now`: something changed and
    /// the interval has passed since the last backup.
    fn due(&self, changed: bool, now: Timestamp) -> bool {
        self.scheduled()
            && (self.dirty || changed)
            && self
                .last
                .is_none_or(|last| now.as_millis() - last.as_millis() >= self.interval.millis())
    }

    fn status(&self, changed: bool, running: bool, now: Timestamp) -> BackupStatusDto {
        let changed = self.dirty || changed;
        let interval = self.interval.millis();
        let next_due = self
            .last
            .filter(|_| self.scheduled())
            .map(|last| last.as_millis().saturating_add(interval));
        let overdue = self.scheduled()
            && changed
            && self
                .last
                .is_some_and(|last| now.as_millis() - last.as_millis() > 2 * interval);
        BackupStatusDto {
            folder: self.folder.as_ref().map(|f| f.display().to_string()),
            auto: self.auto,
            interval: self.interval,
            keep: self.keep,
            last: self.last.map(Timestamp::as_millis),
            next_due,
            changed,
            last_error: self.last_error.clone(),
            failing: self.folder.is_some() && self.last_error.is_some(),
            overdue,
            running,
        }
    }
}

/// What a backup is built from, taken under the replica lock.
struct Job {
    generation: u64,
    folder: PathBuf,
    keep: usize,
    recovery: RecoveryEntry,
    key: VaultKey,
    notes: Vec<BackupNote>,
    /// Whether changes were pending when the snapshot was taken.
    was_changed: bool,
}

fn backup_error(err: BackupError) -> AppError {
    let (code, message) = match err {
        BackupError::NotABackup => (Code::BackupNotABackup, "this isn't a Notebook backup"),
        BackupError::TooNew(_) => (
            Code::BackupTooNew,
            "this backup needs a newer version of Notebook",
        ),
        BackupError::WrongKey => (
            Code::RecoveryKeyWrong,
            "the recovery key doesn't open this backup",
        ),
        BackupError::Damaged => (Code::BackupDamaged, "the backup is damaged"),
        BackupError::TooLarge | BackupError::Crypto(_) => return AppError::internal(err),
    };
    AppError::new(code, message)
}

/// Reads a picked file, checking the header before reading the rest, so a
/// large unrelated file is refused without being read whole.
fn read_backup_file(path: &Path) -> AppResult<Vec<u8>> {
    let mut file = std::fs::File::open(path).map_err(AppError::internal)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(backup::MAX_HEADER_LEN as u64)
        .read_to_end(&mut bytes)
        .map_err(AppError::internal)?;
    match backup::read_header(&bytes) {
        Ok(_) | Err(BackupError::Damaged) => {}
        Err(err) => return Err(backup_error(err)),
    }
    file.read_to_end(&mut bytes).map_err(AppError::internal)?;
    Ok(bytes)
}

/// Writes the restored notebook to a new database at `path`: the recovery
/// entry, every note, and the carried-over device settings.
fn build_restored(
    path: &Path,
    recovery: &RecoveryEntry,
    opened: &OpenedBackup,
    settings: &[(&str, String)],
) -> AppResult<()> {
    SqliteStorage::delete_database(path).map_err(AppError::internal)?;
    let result = (|| {
        let mut storage = vault::open_storage(path, opened.vault)?;
        storage.set_recovery_entry(recovery)?;
        for note in &opened.notes {
            storage.import_note(note.id, &note.wrapped_key, note.snapshot.as_bytes())?;
        }
        for (key, value) in settings {
            storage.set_setting(key, value)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = SqliteStorage::delete_database(path);
    }
    result
}

/// Runs a scheduler tick now and then every [`TICK`] while vault
/// `generation` stays open.
pub(crate) fn spawn_scheduler(core: Weak<Core>, generation: u64) {
    tauri::async_runtime::spawn(async move {
        loop {
            let Some(core) = core.upgrade() else { break };
            let state = AppState(core);
            if !state.is_open(generation) {
                break;
            }
            if let Err(err) = state.backup_tick(now()).await {
                tracing::warn!(code = ?err.code, "backup check failed");
            }
            drop(state);
            tokio::time::sleep(TICK).await;
        }
    });
}

impl AppState {
    fn is_open(&self, generation: u64) -> bool {
        matches!(&*self.lock_phase(), Phase::Open(open) if open.generation == generation)
    }

    fn backup_config(&self) -> AppResult<Config> {
        self.with_vault(|open| Config::read(open.workspace.replica().lock().storage()?))
    }

    pub fn backup_status(&self) -> AppResult<BackupStatusDto> {
        self.backup_status_at(now())
    }

    pub(crate) fn backup_status_at(&self, now: Timestamp) -> AppResult<BackupStatusDto> {
        let backups = &self.0.backups;
        Ok(self.backup_config()?.status(
            backups.changed.load(Ordering::SeqCst),
            backups.running.load(Ordering::SeqCst),
            now,
        ))
    }

    fn emit_backup_status(&self) {
        if let Ok(status) = self.backup_status() {
            events::emit(&*self.0.emitter, events::BACKUP_STATUS_CHANGED, &status);
        }
    }

    /// Sets backup settings in one go.
    fn set_backup_settings(&self, settings: &[(&str, &str)]) -> AppResult<()> {
        self.with_vault(|open| {
            let mut replica = open.workspace.replica().lock();
            for (key, value) in settings {
                replica.set_setting(key, value)?;
            }
            Ok(())
        })
    }

    /// Opens the folder picker, saves the chosen folder, and backs up into
    /// it right away. `None` when the user cancels. A failed first backup
    /// is reported in the status; the folder stays chosen.
    pub async fn choose_backup_folder(
        &self,
        picker: &dyn BackupPicker,
    ) -> AppResult<Option<BackupStatusDto>> {
        self.with_vault(|_| Ok(()))?;
        let Some(folder) = picker.pick_folder().await else {
            return Ok(None);
        };
        let folder = folder.to_str().ok_or_else(|| {
            AppError::new(
                Code::BackupFailed,
                "the folder's name can't be stored; choose another folder",
            )
        })?;
        self.set_backup_settings(&[(FOLDER_KEY, folder), (LAST_ERROR_KEY, "")])?;
        match self.run_backup(now()).await {
            Ok(status) => Ok(Some(status)),
            Err(err) if err.code == Code::BackupFailed => self.backup_status().map(Some),
            Err(err) => Err(err),
        }
    }

    pub fn set_backup_options(
        &self,
        auto: bool,
        interval: Interval,
        keep: usize,
    ) -> AppResult<BackupStatusDto> {
        if keep == 0 {
            return Err(AppError::internal("keep at least one backup"));
        }
        self.set_backup_settings(&[
            (AUTO_KEY, if auto { "true" } else { "false" }),
            (INTERVAL_KEY, interval.as_str()),
            (KEEP_KEY, &keep.to_string()),
        ])?;
        self.emit_backup_status();
        self.backup_status()
    }

    /// Writes a backup now, whether or not anything changed.
    pub async fn backup_now(&self) -> AppResult<BackupStatusDto> {
        self.run_backup(now()).await
    }

    /// A scheduler tick at `now`: records pending changes, and backs up if
    /// one is due. A failed backup is recorded in the status, not returned.
    pub(crate) async fn backup_tick(&self, now: Timestamp) -> AppResult<()> {
        let changed = self.0.backups.changed.load(Ordering::SeqCst);
        let due = self.with_vault(|open| {
            let mut replica = open.workspace.replica().lock();
            let config = Config::read(replica.storage()?)?;
            if changed && !config.dirty {
                replica.set_setting(DIRTY_KEY, "true")?;
            }
            Ok(config.due(changed, now))
        })?;
        if due {
            match self.run_backup(now).await {
                Ok(_) => {}
                Err(err) if err.code == Code::BackupFailed => {}
                Err(err) => return Err(err),
            }
        } else {
            // Time alone can make backups overdue, so the notice hears of it.
            self.emit_backup_status();
        }
        Ok(())
    }

    /// Writes a backup created at `created_at` into the backup folder and
    /// prunes old ones, then records the outcome. Fails with
    /// `backup_failed` when the file couldn't be written.
    pub(crate) async fn run_backup(&self, created_at: Timestamp) -> AppResult<BackupStatusDto> {
        let backups = &self.0.backups;
        let _writing = backups.writing.lock().await;
        let job = self.with_vault(|open| {
            let replica = open.workspace.replica().lock();
            let storage = replica.storage()?;
            let config = Config::read(storage)?;
            let folder = config
                .folder
                .ok_or_else(|| AppError::new(Code::BackupNoFolder, "no backup folder is set"))?;
            let recovery = storage
                .recovery_entry()?
                .ok_or_else(|| AppError::internal("the notebook has no recovery entry"))?;
            let was_changed = backups.changed.swap(false, Ordering::SeqCst);
            let notes = match replica.backup_notes() {
                Ok(notes) => notes,
                Err(err) => {
                    backups.changed.fetch_or(was_changed, Ordering::SeqCst);
                    return Err(AppError::internal(err));
                }
            };
            Ok(Job {
                generation: open.generation,
                folder,
                keep: config.keep,
                recovery,
                key: copy_key(&open.vault_key),
                notes,
                was_changed,
            })
        })?;
        backups.running.store(true, Ordering::SeqCst);
        self.emit_backup_status();

        let (generation, was_changed) = (job.generation, job.was_changed);
        let result = blocking(move || -> Result<PathBuf, String> {
            let bytes = backup::build(&job.recovery, &job.key, created_at, &job.notes)
                .map_err(|err| err.to_string())?;
            let path = backup::folder::write(&job.folder, created_at, &bytes)
                .map_err(|err| err.to_string())?;
            let vault = job.recovery.vault_id();
            match backup::folder::prune(&job.folder, vault, job.keep, &path, created_at) {
                Ok(deleted) if !deleted.is_empty() => {
                    tracing::info!(count = deleted.len(), "pruned old backups");
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, "couldn't prune old backups"),
            }
            Ok(path)
        })
        .await;
        backups.running.store(false, Ordering::SeqCst);
        let result = result.and_then(|r| r.map_err(|msg| AppError::new(Code::BackupFailed, msg)));

        match &result {
            Ok(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                tracing::info!(file = %name, "wrote a backup");
            }
            Err(err) => {
                backups.changed.fetch_or(was_changed, Ordering::SeqCst);
                tracing::warn!(error = %err.message, "backup failed");
            }
        }
        let recorded = self.with_vault(|open| {
            if open.generation != generation {
                return Ok(());
            }
            let mut replica = open.workspace.replica().lock();
            match &result {
                Ok(_) => {
                    let still = backups.changed.load(Ordering::SeqCst);
                    replica.set_setting(LAST_KEY, &created_at.as_millis().to_string())?;
                    replica.set_setting(LAST_ERROR_KEY, "")?;
                    replica.set_setting(DIRTY_KEY, if still { "true" } else { "false" })?;
                }
                Err(err) => replica.set_setting(LAST_ERROR_KEY, &err.message)?,
            }
            Ok(())
        });
        self.emit_backup_status();
        recorded?;
        result?;
        self.backup_status()
    }

    /// The open vault's id, `None` on the welcome screen, or `not_open` in
    /// any other phase: where a backup can be picked and restored.
    fn restorable(&self) -> AppResult<Option<VaultId>> {
        match &*self.lock_phase() {
            Phase::Setup => Ok(None),
            Phase::Open(open) => Ok(Some(open.workspace.replica().lock().vault_id())),
            _ => Err(wrong_phase()),
        }
    }

    /// Opens the file picker and reads the chosen backup's header, keeping
    /// the file for [`restore_backup`](Self::restore_backup). `None` when
    /// the user cancels.
    pub async fn pick_backup(
        &self,
        picker: &dyn BackupPicker,
    ) -> AppResult<Option<PickedBackupDto>> {
        self.restorable()?;
        let Some(path) = picker.pick_file().await else {
            return Ok(None);
        };
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let bytes = blocking(move || read_backup_file(&path)).await??;
        let header = backup::read_header(&bytes).map_err(backup_error)?;
        let same_vault = self.restorable()? == Some(header.vault);
        *self.0.backups.picked() = Some(Picked {
            bytes: Arc::new(bytes),
            header,
        });
        Ok(Some(PickedBackupDto {
            file_name,
            same_vault,
        }))
    }

    /// Forgets the picked backup, when the user cancels a restore, so it
    /// can't be restored later without being picked again.
    pub fn cancel_restore(&self) {
        *self.0.backups.picked() = None;
    }

    /// Replaces this device's notebook with the picked backup.
    ///
    /// Checks everything first: the recovery key (unless the backup is of
    /// the open notebook), then every note. From an open notebook with a
    /// backup folder, it then backs up the current notes, unless
    /// `skip_safety_backup`. Only then is the new database built beside the
    /// old one and swapped in, keeping the old one as
    /// `vault.sqlite3.before-restore`, and the vault key stored. If storing
    /// the key fails, the swap is rolled back.
    pub async fn restore_backup(
        &self,
        recovery_key: Option<&str>,
        skip_safety_backup: bool,
    ) -> AppResult<PhaseDto> {
        let result = self.restore(recovery_key, skip_safety_backup).await;
        if let Err(err) = &result {
            tracing::warn!(code = ?err.code, "restore failed");
        }
        result
    }

    async fn restore(
        &self,
        recovery_key: Option<&str>,
        skip_safety_backup: bool,
    ) -> AppResult<PhaseDto> {
        let recovery = recovery_key.map(state::parse_recovery_key).transpose()?;
        let _ops = self.0.ops.lock().await;
        let current = self.restorable()?;
        let picked = self
            .0
            .backups
            .picked()
            .clone()
            .ok_or_else(|| AppError::new(Code::NotOpen, "no backup was picked"))?;
        let vault = picked.header.vault;
        let key = if current == Some(vault) {
            self.with_vault(|open| Ok(copy_key(&open.vault_key)))?
        } else {
            let recovery = recovery.ok_or_else(|| {
                AppError::new(
                    Code::RecoveryKeyRequired,
                    "this backup needs its recovery key",
                )
            })?;
            picked.header.vault_key(&recovery).map_err(backup_error)?
        };
        let opened = {
            let (bytes, key) = (picked.bytes.clone(), copy_key(&key));
            blocking(move || {
                let opened = backup::open(&bytes, &key)?;
                opened.verify(&key)?;
                Ok(opened)
            })
            .await?
            .map_err(backup_error)?
        };

        let mut settings = Vec::new();
        if current.is_some() {
            let config = self.backup_config()?;
            if config.folder.is_some() && !skip_safety_backup {
                self.run_backup(now()).await.map_err(|err| match err.code {
                    Code::BackupFailed => AppError::new(Code::SafetyBackupFailed, err.message),
                    _ => err,
                })?;
            }
            settings = self.with_vault(|open| {
                let replica = open.workspace.replica().lock();
                let storage = replica.storage()?;
                let mut settings = Vec::new();
                for key in CARRIED_SETTINGS {
                    if let Some(value) = storage.setting(key)? {
                        settings.push((key, value));
                    }
                }
                Ok(settings)
            })?;
        }

        let count = opened.notes.len();
        let db = self.0.db_path();
        let restoring = vault::with_suffix(&db, ".restoring");
        {
            let (restoring, recovery) = (restoring.clone(), picked.header.recovery.clone());
            blocking(move || build_restored(&restoring, &recovery, &opened, &settings)).await??;
        }

        let _writing = self.0.backups.writing.lock().await;
        let old_key = match current {
            Some(_) => Some(self.with_vault(|open| Ok(copy_key(&open.vault_key)))?),
            None => None,
        };
        // Closes the store, so its files can move.
        *self.lock_phase() = Phase::Setup;
        let before = vault::with_suffix(&db, ".before-restore");
        // From the welcome screen, a `before-restore` may be the only copy
        // of a notebook whose restore was cut short, so it is kept.
        let cleared = match current {
            Some(_) => SqliteStorage::delete_database(&before).map_err(AppError::internal),
            None => vault::set_aside(&before, now().as_millis()).map(|_| ()),
        };
        let swapped = cleared
            .and_then(|()| vault::move_database(&db, &before))
            .and_then(|_| match vault::move_database(&restoring, &db) {
                Ok(_) => Ok(()),
                Err(err) => {
                    let _ = vault::move_database(&before, &db);
                    Err(err)
                }
            })
            .and_then(|()| match self.0.remember(vault, &key) {
                Ok(()) => Ok(()),
                Err(err) => {
                    let _ = SqliteStorage::delete_database(&db);
                    let _ = vault::move_database(&before, &db);
                    Err(err)
                }
            });
        if let Err(err) = swapped {
            let _ = SqliteStorage::delete_database(&restoring);
            if let (Some(old_vault), Some(old_key)) = (current, old_key) {
                vault::open_storage(&db, old_vault)
                    .and_then(|storage| self.0.open_vault(storage, old_key))
                    .inspect_err(
                        |err| tracing::warn!(%err, "couldn't reopen after a failed restore"),
                    )
                    .ok();
            }
            self.0.emit_phase();
            return Err(err);
        }
        *self.0.backups.picked() = None;
        if let Err(err) =
            vault::open_storage(&db, vault).and_then(|storage| self.0.open_vault(storage, key))
        {
            // The old notebook is gone from view: let the frontend leave it.
            self.0.emit_phase();
            return Err(err);
        }
        tracing::info!(notes = count, "restored a backup");
        Ok(self.phase())
    }
}

#[cfg(test)]
pub(crate) mod tests;
