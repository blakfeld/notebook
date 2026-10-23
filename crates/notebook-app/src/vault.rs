//! An unlocked vault's runtime: the SQLite store, the replica, the sync
//! engine on its own thread, and the channels the host drains.
//!
//! The engine isn't `Send`, so it is built and run on a dedicated thread
//! with a current-thread tokio runtime. Only the shared replica, the token
//! state, and channel ends cross threads.

use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use futures::channel::mpsc;
use notebook_core::auth::NativeAuthHttp;
use notebook_core::auth::http::{AuthClient, AuthSession, SessionTokens};
use notebook_core::crypto::{VaultId, VaultKey};
use notebook_core::storage::StorageError;
use notebook_core::sync::{
    NativeTransport, Replica, Shared, SyncConfig, SyncEngine, SyncEvent, SyncHandle, TokioTimer,
};
use notebook_core::workspace::{Changed, ServerUrl, Workspace};
use notebook_storage_sqlite::SqliteStorage;

use crate::error::{AppError, AppResult};

pub const DATABASE_FILE: &str = "vault.sqlite3";

pub fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DATABASE_FILE)
}

/// Opens the vault's store. With `replace_other`, a database left by a
/// different vault (an earlier account on this install) is deleted first.
pub fn open_storage(path: &Path, vault: VaultId, replace_other: bool) -> AppResult<SqliteStorage> {
    match SqliteStorage::open(path, vault) {
        Err(StorageError::VaultMismatch { .. }) if replace_other => {
            SqliteStorage::delete_database(path).map_err(AppError::internal)?;
            Ok(SqliteStorage::open(path, vault)?)
        }
        other => Ok(other?),
    }
}

/// What the host gets from a started vault.
pub struct Started {
    pub workspace: Workspace<SqliteStorage>,
    pub changes: mpsc::UnboundedReceiver<Changed>,
    pub events: mpsc::UnboundedReceiver<SyncEvent>,
    /// A message after each storage rebuild (server restore resync).
    pub resynced: mpsc::UnboundedReceiver<()>,
    pub engine: EngineThread,
}

/// The engine's thread. Dropping this doesn't stop it; call
/// [`stop`](Self::stop).
pub struct EngineThread {
    handle: SyncHandle,
    thread: Option<JoinHandle<()>>,
}

impl EngineThread {
    /// Shuts the engine down and waits up to `wait` for its thread (and with
    /// it the replica and the store) to finish. Returns whether it did.
    pub fn stop(mut self, wait: Duration) -> bool {
        self.handle.shutdown();
        let Some(thread) = self.thread.take() else {
            return true;
        };
        let (done_tx, done_rx) = std_mpsc::channel();
        std::thread::spawn(move || {
            let _ = thread.join();
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(wait).is_ok()
    }
}

/// Opens the replica over `storage` and starts its engine against `server`.
pub fn start(
    storage: SqliteStorage,
    db_path: PathBuf,
    vault: VaultId,
    vault_key: VaultKey,
    server: ServerUrl,
    tokens: SessionTokens,
    config: SyncConfig,
) -> AppResult<Started> {
    let replica = Shared::new(Replica::open(storage, vault_key)?);
    let (resynced_tx, resynced) = mpsc::unbounded();
    let reopen = move || {
        let store = SqliteStorage::open(&db_path, vault);
        let _ = resynced_tx.unbounded_send(());
        store
    };

    let (ready_tx, ready_rx) = std_mpsc::sync_channel(1);
    let engine_replica = replica.clone();
    let thread = std::thread::Builder::new()
        .name("notebook-sync".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    let _ = ready_tx.send(Err(AppError::internal(err)));
                    return;
                }
            };
            runtime.block_on(async move {
                let parts = NativeTransport::new(server.as_str())
                    .map_err(AppError::internal)
                    .and_then(|transport| {
                        let http =
                            NativeAuthHttp::new(server.clone()).map_err(AppError::internal)?;
                        Ok((transport, http))
                    });
                let (transport, http) = match parts {
                    Ok(parts) => parts,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };
                let timer = TokioTimer::new();
                let session = AuthSession::new(AuthClient::new(http), tokens, timer);
                let (engine, handle, events) = SyncEngine::new(
                    engine_replica,
                    transport,
                    timer,
                    session,
                    config,
                    Box::new(reopen),
                );
                let _ = ready_tx.send(Ok((handle, events)));
                engine.run().await;
            });
        })
        .map_err(AppError::internal)?;
    let (handle, events) = ready_rx
        .recv()
        .map_err(|_| AppError::internal("the sync thread exited"))??;

    let (workspace, changes) = Workspace::open(replica, handle.clone());
    Ok(Started {
        workspace,
        changes,
        events,
        resynced,
        engine: EngineThread {
            handle,
            thread: Some(thread),
        },
    })
}
