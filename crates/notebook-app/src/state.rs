//! The app state machine: signed out, locked, or unlocked.
//!
//! All decrypted state, keys, storage, sync, and networking live here; the
//! webview asks for what it renders. Account operations (sign in, unlock,
//! lock, sign out, ...) are serialized by one async lock and never hold the
//! phase lock across a network call, so vault commands stay responsive.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use notebook_core::auth::api::{ErrorCode, Token};
use notebook_core::auth::http::{AuthClient, AuthClientError, Refreshed, SessionTokens, SignedIn};
use notebook_core::auth::{AuthError, Device, NativeAuthHttp};
use notebook_core::crypto::{KeyBundle, LocalUnlock, RecoveryKey, VaultId, VaultKey};
use notebook_core::note::Timestamp;
use notebook_core::storage::Storage;
use notebook_core::sync::{Attention, SyncConfig, SyncEvent};
use notebook_core::workspace::{Changed, ServerUrl, Workspace};
use notebook_storage_sqlite::SqliteStorage;
use zeroize::Zeroizing;

use crate::credentials::{
    self, CredentialError, CredentialStore, Entry, MemoryStore, StoredSession,
};
use crate::dto::{AccountDto, PhaseDto, PhaseName, SettingsDto, SyncStatusDto, VaultChangedDto};
use crate::error::{AppError, AppResult, Code};
use crate::events::{self, Emitter};
use crate::vault::{self, EngineThread};

/// Device setting keys. Values are not sensitive.
pub const VIM_MODE_KEY: &str = "editor.vimMode";
pub const THEME_KEY: &str = "appearance.theme";
pub const SOURCE_VIEW_KEY: &str = "editor.sourceView";
pub const FOCUS_MODE_KEY: &str = "editor.focusMode";
const THEMES: [&str; 3] = ["system", "light", "dark"];
const COLLAPSED_TAGS_KEY: &str = "sidebar.collapsedTags";
/// A UX hint only: the missing `auto-unlock` entry is what keeps the vault
/// locked.
const LOCKED_KEY: &str = "app.locked";

/// How long sign-out waits for the server to revoke the session.
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long lock and sign-out wait for the engine thread to finish.
const ENGINE_STOP_TIMEOUT: Duration = Duration::from_secs(5);

pub struct AppConfig {
    /// Where the vault database lives (`app_data_dir()` in the app).
    pub data_dir: PathBuf,
    pub sync: SyncConfig,
    pub device: Device,
}

/// The app state, shared by every command. Cheap to clone.
#[derive(Clone)]
pub struct AppState(Arc<Core>);

struct Core {
    config: AppConfig,
    /// Swapped for a [`MemoryStore`] when the OS store turns out to be
    /// unusable, so nothing is persisted.
    creds: RwLock<Arc<dyn CredentialStore>>,
    remembered: AtomicBool,
    emitter: Arc<dyn Emitter>,
    phase: Mutex<Phase>,
    ops: tokio::sync::Mutex<()>,
    /// The recovery key shown before registration, consumed by `register`.
    pending_recovery: Mutex<Option<RecoveryKey>>,
    next_generation: AtomicU64,
    /// Editor session numbers, never reused within a run (across unlocks
    /// too, so a stale editor can't reach another note).
    next_editor: AtomicU64,
}

pub(crate) enum Phase {
    SignedOut,
    Locked(Account),
    Unlocked(Box<Unlocked>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    server: ServerUrl,
    username: String,
    vault: VaultId,
}

impl Account {
    fn from_session(session: &StoredSession) -> Result<Self, CredentialError> {
        Ok(Self {
            server: session.server()?,
            username: session.username.clone(),
            vault: session.vault()?,
        })
    }

    fn dto(&self) -> AccountDto {
        AccountDto {
            server: self.server.to_string(),
            username: self.username.clone(),
        }
    }
}

pub(crate) struct Unlocked {
    account: Account,
    /// Distinguishes this unlock from later ones, so host tasks of an old
    /// vault never touch a new one.
    generation: u64,
    pub(crate) workspace: Workspace<SqliteStorage>,
    tokens: SessionTokens,
    engine: Option<EngineThread>,
    pub(crate) status: SyncStatusDto,
    /// Editor sessions by the id the frontend knows them by.
    pub(crate) editors: crate::editor::Editors,
}

pub fn now() -> Timestamp {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    Timestamp(i64::try_from(ms).unwrap_or(i64::MAX))
}

fn copy_key(key: &VaultKey) -> VaultKey {
    VaultKey::from_secret_bytes(&*key.to_secret_bytes()).expect("a vault key is 32 bytes")
}

fn auth_client(server: &ServerUrl) -> AppResult<AuthClient<NativeAuthHttp>> {
    Ok(AuthClient::new(
        NativeAuthHttp::new(server.clone()).map_err(AppError::internal)?,
    ))
}

fn mismatch() -> AppError {
    AppError::new(Code::PassphraseMismatch, "the passphrases don't match")
}

/// Runs Argon2id work off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(AppError::internal)
}

impl AppState {
    /// Builds the state and picks the launch phase from stored entries: no
    /// session is signed out; a session with an auto-unlock key opens the
    /// vault without the server; a session without one is locked.
    pub fn launch(
        config: AppConfig,
        creds: Arc<dyn CredentialStore>,
        emitter: Arc<dyn Emitter>,
    ) -> Self {
        let state = Self(Arc::new(Core {
            config,
            creds: RwLock::new(creds),
            remembered: AtomicBool::new(true),
            emitter,
            phase: Mutex::new(Phase::SignedOut),
            ops: tokio::sync::Mutex::new(()),
            pending_recovery: Mutex::new(None),
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

    /// Runs `f` on the unlocked vault, or fails with `locked`.
    pub(crate) fn with_vault<T>(
        &self,
        f: impl FnOnce(&mut Unlocked) -> AppResult<T>,
    ) -> AppResult<T> {
        match &mut *self.lock_phase() {
            Phase::Unlocked(unlocked) => f(unlocked),
            _ => Err(AppError::locked()),
        }
    }

    pub async fn sign_in(
        &self,
        server: &str,
        username: &str,
        passphrase: &str,
    ) -> AppResult<PhaseDto> {
        let server = ServerUrl::parse(server)?;
        let _ops = self.0.ops.lock().await;
        self.0.require_signed_out()?;
        let client = auth_client(&server)?;
        let signed_in = client
            .login(username, passphrase, self.0.config.device.clone())
            .await
            .map_err(|err| match err {
                // A malformed username can't belong to anyone: same message.
                AuthClientError::Auth(AuthError::InvalidUsername) => {
                    AppError::new(Code::BadCredentials, "wrong username or passphrase")
                }
                err => err.into(),
            })?;
        self.0
            .finish_sign_in(server, signed_in, passphrase, true)
            .await
    }

    /// Generates the recovery key for a registration and returns its display
    /// form. It stays here until `register` succeeds.
    pub fn generate_recovery_key(&self) -> AppResult<Zeroizing<String>> {
        let key = RecoveryKey::generate().map_err(AppError::internal)?;
        let display = key.to_display();
        *self
            .0
            .pending_recovery
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(key);
        Ok(display)
    }

    pub async fn register(
        &self,
        server: &str,
        username: &str,
        passphrase: &str,
        confirm: &str,
        invite_code: &str,
    ) -> AppResult<PhaseDto> {
        let server = ServerUrl::parse(server)?;
        if passphrase != confirm {
            return Err(mismatch());
        }
        let _ops = self.0.ops.lock().await;
        self.0.require_signed_out()?;
        // A copy, so a refused attempt keeps the key the user already saved.
        let recovery_key = self
            .0
            .pending_recovery
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|key| RecoveryKey::parse(&key.to_display()))
            .transpose()
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::new(
                    Code::RecoveryKeyRequired,
                    "generate and save a recovery key first",
                )
            })?;
        let client = auth_client(&server)?;
        let signed_in = client
            .register(
                username,
                passphrase,
                invite_code,
                recovery_key,
                self.0.config.device.clone(),
                now().as_millis().max(0) as u64,
            )
            .await?;
        *self
            .0
            .pending_recovery
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        self.0
            .finish_sign_in(server, signed_in, passphrase, true)
            .await
    }

    /// Replaces the passphrase with the recovery key and signs in. A key
    /// that fails its checksum is reported without contacting the server.
    pub async fn recover(
        &self,
        server: &str,
        username: &str,
        recovery_key: &str,
        passphrase: &str,
        confirm: &str,
    ) -> AppResult<PhaseDto> {
        let server = ServerUrl::parse(server)?;
        let key = RecoveryKey::parse(recovery_key).map_err(|_| {
            AppError::new(
                Code::RecoveryKeyMalformed,
                "the recovery key looks mistyped",
            )
        })?;
        if passphrase != confirm {
            return Err(mismatch());
        }
        let _ops = self.0.ops.lock().await;
        self.0.require_signed_out()?;
        let client = auth_client(&server)?;
        let signed_in = client
            .recover(username, key, passphrase, self.0.config.device.clone())
            .await
            .map_err(|err| match err {
                AuthClientError::Rejected(ErrorCode::InvalidCredentials)
                | AuthClientError::Auth(AuthError::Crypto(_)) => AppError::new(
                    Code::RecoveryKeyWrong,
                    "the recovery key doesn't unlock this account",
                ),
                err => err.into(),
            })?;
        self.0
            .finish_sign_in(server, signed_in, passphrase, true)
            .await
    }

    /// Unlocks a locked vault: through the server when it answers (which
    /// also catches a passphrase changed elsewhere), else with the local
    /// passphrase wrap.
    pub async fn unlock(&self, passphrase: &str) -> AppResult<PhaseDto> {
        let _ops = self.0.ops.lock().await;
        let account = match &*self.lock_phase() {
            Phase::Locked(account) => account.clone(),
            Phase::Unlocked(_) => return Ok(self.phase()),
            Phase::SignedOut => return Err(AppError::new(Code::Locked, "not signed in")),
        };
        let creds = self.0.creds();
        let stored = credentials::load_session(&*creds)?
            .ok_or_else(|| AppError::internal("the session entry is missing"))?;
        let client = auth_client(&account.server)?;
        match client
            .login(&account.username, passphrase, self.0.config.device.clone())
            .await
        {
            Ok(signed_in) => {
                if signed_in.vault_id != account.vault {
                    return Err(AppError::internal("the server returned another vault"));
                }
                // This device's earlier session is replaced by the new one.
                let access = signed_in.session.access_token.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = client.revoke_session(&access, &stored.session_id).await;
                });
                self.0
                    .finish_sign_in(account.server, signed_in, passphrase, false)
                    .await
            }
            Err(
                AuthClientError::Auth(AuthError::InvalidCredentials)
                | AuthClientError::Rejected(ErrorCode::InvalidCredentials),
            ) => Err(wrong_passphrase()),
            // Unlock works whether or not the server answers, rate limits
            // included.
            Err(
                AuthClientError::Network
                | AuthClientError::Server(_)
                | AuthClientError::RateLimited { .. }
                | AuthClientError::Auth(AuthError::Protocol),
            ) => self.0.unlock_offline(account, stored, passphrase).await,
            Err(err) => Err(err.into()),
        }
    }

    /// Discards keys and decrypted notes, stops sync, and requires the
    /// passphrase from now on, across relaunches too.
    pub async fn lock(&self) -> AppResult<PhaseDto> {
        let _ops = self.0.ops.lock().await;
        let unlocked = {
            let mut phase = self.lock_phase();
            let Phase::Unlocked(unlocked) = &*phase else {
                return Ok(self.0.dto_of(&phase));
            };
            let account = unlocked.account.clone();
            match std::mem::replace(&mut *phase, Phase::Locked(account)) {
                Phase::Unlocked(unlocked) => unlocked,
                _ => unreachable!(),
            }
        };
        let _ = unlocked
            .workspace
            .replica()
            .lock()
            .set_setting(LOCKED_KEY, "true");
        let creds = self.0.creds();
        let deleted = creds.delete(Entry::AutoUnlock);
        self.0.stop(*unlocked).await;
        self.0.emit_phase();
        deleted?;
        Ok(self.phase())
    }

    /// Signs in again after the session ended, keeping the vault open.
    pub async fn reauthenticate(&self, passphrase: &str) -> AppResult<()> {
        let _ops = self.0.ops.lock().await;
        let (account, tokens) = self.with_vault(|u| Ok((u.account.clone(), u.tokens.clone())))?;
        let client = auth_client(&account.server)?;
        let signed_in = client
            .login(&account.username, passphrase, self.0.config.device.clone())
            .await
            .map_err(|err| match err {
                AuthClientError::Auth(AuthError::InvalidCredentials)
                | AuthClientError::Rejected(ErrorCode::InvalidCredentials) => wrong_passphrase(),
                err => err.into(),
            })?;
        if signed_in.vault_id != account.vault {
            return Err(AppError::internal("the server returned another vault"));
        }
        let previous = credentials::load_session(&*self.0.creds()).ok().flatten();
        self.0
            .save_signed_in(&account.server, &signed_in, passphrase)
            .await?;
        tokens.install(&signed_in.session);
        // Usually already ended; revoke it in case it wasn't.
        if let Some(previous) = previous {
            let access = signed_in.session.access_token.clone();
            tauri::async_runtime::spawn(async move {
                let _ = client.revoke_session(&access, &previous.session_id).await;
            });
        }
        self.with_vault(|u| {
            u.workspace.sync().set_session();
            Ok(())
        })
    }

    /// Changes the passphrase on the server and rewraps the local copy.
    /// Needs a connection.
    pub async fn change_passphrase(
        &self,
        current: &str,
        new: &str,
        confirm: &str,
    ) -> AppResult<()> {
        if new != confirm {
            return Err(mismatch());
        }
        let _ops = self.0.ops.lock().await;
        let (account, tokens) = self.with_vault(|u| Ok((u.account.clone(), u.tokens.clone())))?;
        let client = auth_client(&account.server)?;
        let creds = self.0.creds();
        let key = credentials::load_auto_unlock(&*creds)?
            .ok_or_else(|| AppError::internal("the vault key entry is missing"))?;
        // Dropped until it is rewrapped below, so a failure after the server
        // accepted the change can't leave the old passphrase unlocking this
        // device offline; the next unlock then just needs the server.
        let old_wrap = creds.get(Entry::LocalUnlock)?;
        creds.delete(Entry::LocalUnlock)?;
        let bundle = tokens
            .authed(&client, |token| {
                let client = &client;
                async move { client.change_passphrase(&token, current, new).await }
            })
            .await
            .map_err(|err| match err {
                AuthClientError::Auth(AuthError::InvalidCredentials)
                | AuthClientError::Rejected(ErrorCode::InvalidCredentials) => wrong_passphrase(),
                AuthClientError::Network | AuthClientError::Server(_) => {
                    AppError::new(Code::Offline, "changing the passphrase needs a connection")
                }
                err => err.into(),
            });
        let bundle = match bundle {
            Ok(bundle) => bundle,
            Err(err) => {
                // Put the old wrap back only when the change was refused
                // before the finish request (a wrong current passphrase or a
                // too-short new one). After that, a lost response is
                // ambiguous, and it stays dropped.
                let refused_first =
                    matches!(err.code, Code::WrongPassphrase | Code::PassphraseTooShort);
                if refused_first && let Some(old) = old_wrap {
                    let _ = creds.set(Entry::LocalUnlock, &old);
                }
                return Err(err);
            }
        };
        self.0
            .save_local_unlock(account.vault, key, new, &bundle)
            .await
    }

    /// Local changes the server hasn't acknowledged.
    pub fn pending_count(&self) -> AppResult<usize> {
        self.0.pending_count()
    }

    /// Signs out: revokes the session when the server answers, deletes every
    /// entry and the local database. With pending changes, `confirm` must be
    /// set, since they are lost.
    pub async fn sign_out(&self, confirm: bool) -> AppResult<PhaseDto> {
        let _ops = self.0.ops.lock().await;
        // A local store that won't open must not trap the user: its count
        // is unknown, so ask, and go ahead once confirmed.
        let pending = match self.0.pending_count() {
            Ok(pending) => pending,
            Err(_) if confirm => 0,
            Err(_) => {
                return Err(AppError::new(
                    Code::PendingChanges,
                    "unsynced changes on this device may be lost",
                ));
            }
        };
        if pending > 0 && !confirm {
            return Err(AppError::new(
                Code::PendingChanges,
                format!("{pending} unsynced changes would be lost"),
            ));
        }
        let previous = std::mem::replace(&mut *self.lock_phase(), Phase::SignedOut);
        let creds = self.0.creds();
        let stored = credentials::load_session(&*creds).ok().flatten();
        let server = match &previous {
            Phase::Unlocked(u) => Some(u.account.server.clone()),
            Phase::Locked(a) => Some(a.server.clone()),
            Phase::SignedOut => None,
        };
        let tokens = match &previous {
            Phase::Unlocked(u) => Some(u.tokens.clone()),
            _ => stored
                .as_ref()
                .map(|s| SessionTokens::new(Some(s.refresh_token.clone()), None)),
        };
        if let (Some(server), Some(tokens)) = (server, tokens) {
            let client = auth_client(&server)?;
            let logout = tokens.authed(&client, |token| {
                let client = &client;
                async move { client.logout(&token).await }
            });
            let _ = tokio::time::timeout(LOGOUT_TIMEOUT, logout).await;
            tokens.clear();
        }
        if let Phase::Unlocked(unlocked) = previous {
            self.0.stop(*unlocked).await;
        }
        let deleted = credentials::delete_all(&*creds);
        let removed = SqliteStorage::delete_database(vault::database_path(&self.0.config.data_dir));
        *self
            .0
            .pending_recovery
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        self.0.emit_phase();
        deleted?;
        removed.map_err(AppError::internal)?;
        Ok(self.phase())
    }

    pub fn settings(&self) -> AppResult<SettingsDto> {
        let read = |storage: &dyn SettingsRead| -> AppResult<SettingsDto> {
            let on = |key| -> AppResult<bool> { Ok(storage.get(key)?.as_deref() == Some("true")) };
            let theme = storage
                .get(THEME_KEY)?
                .filter(|t| THEMES.contains(&t.as_str()))
                .unwrap_or_else(|| THEMES[0].to_owned());
            Ok(SettingsDto {
                vim_mode: on(VIM_MODE_KEY)?,
                theme,
                source_view: on(SOURCE_VIEW_KEY)?,
                focus_mode: on(FOCUS_MODE_KEY)?,
                collapsed_tags: Vec::new(),
            })
        };
        match &*self.lock_phase() {
            Phase::SignedOut => Ok(SettingsDto {
                vim_mode: false,
                theme: THEMES[0].to_owned(),
                source_view: false,
                focus_mode: false,
                collapsed_tags: Vec::new(),
            }),
            Phase::Locked(account) => read(&self.0.open_settings(account)?),
            Phase::Unlocked(u) => {
                let replica = u.workspace.replica().lock();
                let mut settings = read(replica.storage()?)?;
                // Tag names are vault content: sealed at rest, and only
                // shown while unlocked.
                settings.collapsed_tags = replica
                    .sealed_setting(COLLAPSED_TAGS_KEY)?
                    .and_then(|v| serde_json::from_slice(&v).ok())
                    .unwrap_or_default();
                Ok(settings)
            }
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        match &*self.lock_phase() {
            Phase::SignedOut => Err(AppError::new(Code::Locked, "not signed in")),
            Phase::Locked(account) => Ok(self.0.open_settings(account)?.set_setting(key, value)?),
            Phase::Unlocked(u) => Ok(u.workspace.replica().lock().set_setting(key, value)?),
        }
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

    /// Remembers whether a tag is expanded in the sidebar. Needs the vault
    /// unlocked, since the names are stored sealed.
    pub fn set_tag_expanded(&self, path: &str, expanded: bool) -> AppResult<()> {
        let key = path.to_lowercase();
        let mut collapsed = self.settings()?.collapsed_tags;
        collapsed.retain(|p| *p != key);
        if !expanded {
            collapsed.push(key);
            collapsed.sort();
        }
        let value = Zeroizing::new(serde_json::to_vec(&collapsed).expect("strings serialize"));
        self.with_vault(|u| {
            u.workspace
                .replica()
                .lock()
                .set_sealed_setting(COLLAPSED_TAGS_KEY, &value)
                .map_err(AppError::internal)
        })
    }
}

fn wrong_passphrase() -> AppError {
    AppError::new(Code::WrongPassphrase, "the passphrase is incorrect")
}

/// Read access to device settings, from an open replica's store or a store
/// opened just for this.
trait SettingsRead {
    fn get(&self, key: &str) -> AppResult<Option<String>>;
}

impl<S: Storage> SettingsRead for S {
    fn get(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.setting(key)?)
    }
}

impl Core {
    fn lock_phase(&self) -> MutexGuard<'_, Phase> {
        self.phase.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn creds(&self) -> Arc<dyn CredentialStore> {
        self.creds.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Writes through the credential store, switching to memory-only storage
    /// (and telling the user) the first time the OS store turns out to be
    /// unusable. Never falls back to a file.
    fn store(
        &self,
        write: impl Fn(&dyn CredentialStore) -> Result<(), CredentialError>,
    ) -> AppResult<()> {
        match write(&*self.creds()) {
            Err(CredentialError::NoSecureStore) => {
                tracing::warn!("no secure credential store; credentials won't be remembered");
                let memory: Arc<dyn CredentialStore> = Arc::new(MemoryStore::new());
                let old = std::mem::replace(
                    &mut *self.creds.write().unwrap_or_else(|e| e.into_inner()),
                    memory.clone(),
                );
                // Writes may have reached the OS store before it failed;
                // don't leave them behind where lock and sign-out can't
                // reach them. Everything is replayed into memory below.
                if let Some(session) = credentials::load_session(&*old).ok().flatten() {
                    let _ = credentials::save_session(&*memory, &session);
                }
                let _ = credentials::delete_all(&*old);
                self.remembered.store(false, Ordering::SeqCst);
                Ok(write(&*memory)?)
            }
            other => Ok(other?),
        }
    }

    fn dto_of(&self, phase: &Phase) -> PhaseDto {
        let (name, account) = match phase {
            Phase::SignedOut => (PhaseName::SignedOut, None),
            Phase::Locked(account) => (PhaseName::Locked, Some(account.dto())),
            Phase::Unlocked(u) => (PhaseName::Unlocked, Some(u.account.dto())),
        };
        PhaseDto {
            phase: name,
            account,
            credentials_remembered: self.remembered.load(Ordering::SeqCst),
        }
    }

    fn phase_dto(&self) -> PhaseDto {
        self.dto_of(&self.lock_phase())
    }

    fn emit_phase(&self) {
        events::emit(&*self.emitter, events::APP_PHASE_CHANGED, &self.phase_dto());
    }

    fn require_signed_out(&self) -> AppResult<()> {
        match &*self.lock_phase() {
            Phase::SignedOut => Ok(()),
            _ => Err(AppError::internal("already signed in")),
        }
    }

    fn open_settings(&self, account: &Account) -> AppResult<SqliteStorage> {
        vault::open_storage(
            &vault::database_path(&self.config.data_dir),
            account.vault,
            false,
        )
    }

    fn pending_count(&self) -> AppResult<usize> {
        match &*self.lock_phase() {
            Phase::SignedOut => Ok(0),
            Phase::Locked(account) => Ok(self.open_settings(account)?.outbox()?.len()),
            Phase::Unlocked(u) => Ok(u.workspace.replica().lock().pending()?),
        }
    }

    /// Picks the launch phase (see [`AppState::launch`]).
    fn restore(self: &Arc<Self>) {
        let creds = self.creds();
        let stored = match credentials::load_session(&*creds) {
            Ok(Some(stored)) => stored,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(%err, "can't read the stored session; signed out");
                return;
            }
        };
        let Ok(account) = Account::from_session(&stored) else {
            tracing::warn!("the stored session is unreadable; signed out");
            return;
        };
        *self.lock_phase() = Phase::Locked(account.clone());
        let locked_hint = self
            .open_settings(&account)
            .ok()
            .and_then(|s| s.setting(LOCKED_KEY).ok().flatten())
            .is_some_and(|v| v == "true");
        if locked_hint {
            return;
        }
        match credentials::load_auto_unlock(&*creds) {
            Ok(Some(key)) => {
                let tokens = self.tokens(Some(stored.refresh_token));
                if let Err(err) = self.open_vault(account, key, tokens, false) {
                    tracing::warn!(%err, "couldn't open the vault at launch; locked");
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(%err, "can't read the auto-unlock entry; locked"),
        }
    }

    /// Token state whose refreshes update the session entry and check the
    /// key bundle fingerprint.
    fn tokens(self: &Arc<Self>, refresh: Option<Token>) -> SessionTokens {
        let core = Arc::downgrade(self);
        SessionTokens::new(
            refresh,
            Some(Arc::new(move |refreshed: &Refreshed| {
                if let Some(core) = core.upgrade() {
                    core.on_refresh(refreshed);
                }
            })),
        )
    }

    /// After a refresh (on the engine thread): stores the rotated token and
    /// drops the local passphrase wrap if the passphrase changed elsewhere.
    fn on_refresh(&self, refreshed: &Refreshed) {
        let creds = self.creds();
        // Signed out meanwhile: don't bring the entry back.
        let Ok(Some(mut stored)) = credentials::load_session(&*creds) else {
            return;
        };
        stored.refresh_token = refreshed.refresh_token.clone();
        if let Err(err) = credentials::save_session(&*creds, &stored) {
            tracing::warn!(%err, "couldn't store the rotated refresh token");
        }
        if let (Some(bundle), Ok(vault)) = (&refreshed.key_bundle, stored.vault()) {
            check_fingerprint(&*creds, vault, bundle);
        }
    }

    /// Stores what a sign-in produced: the session entry, the local
    /// passphrase wrap, and the auto-unlock key.
    async fn save_signed_in(
        &self,
        server: &ServerUrl,
        signed_in: &SignedIn,
        passphrase: &str,
    ) -> AppResult<()> {
        let stored = StoredSession {
            server: server.to_string(),
            username: signed_in.session.username.clone(),
            vault_id: signed_in.vault_id.to_string(),
            session_id: signed_in.session.session_id.clone(),
            refresh_token: signed_in.session.refresh_token.clone(),
        };
        let (local, fingerprint) = seal_local_unlock(
            signed_in.vault_id,
            copy_key(&signed_in.vault_key),
            passphrase,
            &signed_in.key_bundle,
        )
        .await?;
        // One write set, so a fallback to memory replays all of it.
        self.store(|c| {
            credentials::save_session(c, &stored)?;
            credentials::save_local_unlock(c, &local, &fingerprint)?;
            credentials::save_auto_unlock(c, &signed_in.vault_key)
        })
    }

    async fn save_local_unlock(
        &self,
        vault: VaultId,
        key: VaultKey,
        passphrase: &str,
        bundle: &[u8],
    ) -> AppResult<()> {
        let (local, fingerprint) = seal_local_unlock(vault, key, passphrase, bundle).await?;
        self.store(|c| credentials::save_local_unlock(c, &local, &fingerprint))
    }
}

/// Wraps the vault key under the passphrase (Argon2id, off the async
/// threads) and fingerprints the bundle it goes with.
async fn seal_local_unlock(
    vault: VaultId,
    key: VaultKey,
    passphrase: &str,
    bundle: &[u8],
) -> AppResult<(LocalUnlock, [u8; 32])> {
    let fingerprint = KeyBundle::from_bytes(bundle)
        .map_err(AppError::internal)?
        .fingerprint();
    let passphrase = Zeroizing::new(passphrase.to_owned());
    let local = blocking(move || LocalUnlock::seal(vault, &key, &passphrase))
        .await?
        .map_err(AppError::internal)?;
    Ok((local, fingerprint))
}

impl Core {
    async fn finish_sign_in(
        self: &Arc<Self>,
        server: ServerUrl,
        signed_in: SignedIn,
        passphrase: &str,
        replace_other: bool,
    ) -> AppResult<PhaseDto> {
        self.save_signed_in(&server, &signed_in, passphrase).await?;
        let tokens = self.tokens(Some(signed_in.session.refresh_token.clone()));
        tokens.install(&signed_in.session);
        let account = Account {
            server,
            username: signed_in.session.username.clone(),
            vault: signed_in.vault_id,
        };
        self.open_vault(account, signed_in.vault_key, tokens, replace_other)?;
        Ok(self.phase_dto())
    }

    async fn unlock_offline(
        self: &Arc<Self>,
        account: Account,
        stored: StoredSession,
        passphrase: &str,
    ) -> AppResult<PhaseDto> {
        let creds = self.creds();
        let Some((local, _)) = credentials::load_local_unlock(&*creds, account.vault)? else {
            return Err(AppError::new(
                Code::Offline,
                "unlocking needs a connection after a passphrase change",
            ));
        };
        let passphrase = Zeroizing::new(passphrase.to_owned());
        let key = blocking(move || local.open(&passphrase))
            .await?
            .map_err(|_| wrong_passphrase())?;
        self.store(|c| credentials::save_auto_unlock(c, &key))?;
        let tokens = self.tokens(Some(stored.refresh_token));
        self.open_vault(account, key, tokens, false)?;
        Ok(self.phase_dto())
    }

    /// Opens the store and replica, starts sync and the host tasks, and
    /// switches to unlocked.
    fn open_vault(
        self: &Arc<Self>,
        account: Account,
        key: VaultKey,
        tokens: SessionTokens,
        replace_other: bool,
    ) -> AppResult<()> {
        let path = vault::database_path(&self.config.data_dir);
        let storage = vault::open_storage(&path, account.vault, replace_other)?;
        let started = vault::start(
            storage,
            path,
            account.vault,
            key,
            account.server.clone(),
            tokens.clone(),
            self.config.sync.clone(),
        )?;
        let _ = started
            .workspace
            .replica()
            .lock()
            .set_setting(LOCKED_KEY, "false");
        let pending = started.workspace.replica().lock().pending().unwrap_or(0);
        let generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        *self.lock_phase() = Phase::Unlocked(Box::new(Unlocked {
            account,
            generation,
            workspace: started.workspace,
            tokens,
            engine: Some(started.engine),
            status: SyncStatusDto::starting(pending),
            editors: Default::default(),
        }));
        self.spawn_tasks(
            generation,
            started.events,
            started.changes,
            started.resynced,
        );
        self.emit_phase();
        Ok(())
    }

    fn spawn_tasks(
        self: &Arc<Self>,
        generation: u64,
        mut events: futures::channel::mpsc::UnboundedReceiver<SyncEvent>,
        mut changes: futures::channel::mpsc::UnboundedReceiver<Changed>,
        mut resynced: futures::channel::mpsc::UnboundedReceiver<()>,
    ) {
        let core: Weak<Self> = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.next().await {
                let Some(core) = core.upgrade() else { break };
                core.on_sync_event(generation, &event);
            }
        });
        let core: Weak<Self> = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            while let Some(changed) = changes.next().await {
                let Some(core) = core.upgrade() else { break };
                core.on_changed(generation, &changed);
            }
        });
        let core: Weak<Self> = Arc::downgrade(self);
        tauri::async_runtime::spawn(async move {
            while resynced.next().await.is_some() {
                let Some(core) = core.upgrade() else { break };
                if let Phase::Unlocked(u) = &mut *core.lock_phase()
                    && u.generation == generation
                {
                    u.workspace.reload();
                }
            }
        });
    }

    fn on_sync_event(&self, generation: u64, event: &SyncEvent) {
        let mut phase = self.lock_phase();
        let Phase::Unlocked(u) = &mut *phase else {
            return;
        };
        if u.generation != generation {
            return;
        }
        match event {
            SyncEvent::Status(status) => {
                if status.attention == Some(Attention::SignInRequired) {
                    // The server ends every other session when the passphrase
                    // changes or the account is recovered, and this device
                    // can't tell which happened. Fail closed: the next unlock
                    // needs the server, which checks the passphrase.
                    if let Err(err) = self.creds().delete(Entry::LocalUnlock) {
                        tracing::warn!(%err, "couldn't drop the local passphrase wrap");
                    }
                }
                u.status = SyncStatusDto::from(status);
                events::emit(&*self.emitter, events::SYNC_STATUS, &u.status);
            }
            event => u.workspace.apply_sync_event(event),
        }
    }

    fn on_changed(&self, generation: u64, changed: &Changed) {
        let mut phase = self.lock_phase();
        let Phase::Unlocked(u) = &mut *phase else {
            return;
        };
        if u.generation != generation {
            return;
        }
        events::emit(
            &*self.emitter,
            events::VAULT_CHANGED,
            &VaultChangedDto::from(changed),
        );
        crate::editor::forward_remote(&*self.emitter, u, changed);
    }

    /// Stops a vault taken out of the phase: its engine thread, and with it
    /// the replica (keys zeroize on drop) and the store.
    async fn stop(&self, mut unlocked: Unlocked) {
        let engine = unlocked.engine.take();
        drop(unlocked);
        if let Some(engine) = engine {
            let stopped = blocking(move || engine.stop(ENGINE_STOP_TIMEOUT)).await;
            if !matches!(stopped, Ok(true)) {
                tracing::warn!("the sync thread didn't stop in time");
            }
        }
    }
}

/// Drops the local passphrase wrap if `bundle` isn't the one it was made
/// alongside: the passphrase changed on another device, so the old one must
/// stop unlocking this one.
fn check_fingerprint(creds: &dyn CredentialStore, vault: VaultId, bundle: &[u8]) {
    let Ok(current) = KeyBundle::from_bytes(bundle).map(|b| b.fingerprint()) else {
        return;
    };
    if let Ok(Some((_, stored))) = credentials::load_local_unlock(creds, vault)
        && stored != current
    {
        tracing::info!("the key bundle changed; the local passphrase wrap is dropped");
        if let Err(err) = creds.delete(Entry::LocalUnlock) {
            tracing::warn!(%err, "couldn't drop the local passphrase wrap");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
