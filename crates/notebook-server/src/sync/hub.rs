//! Per-vault live delivery: a write mutex held across commit and broadcast,
//! so live entries go out in sequence order, and a broadcast channel that
//! every live connection of the vault subscribes to.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use notebook_core::note::NoteId;
use notebook_core::protocol::{Entry, PushUpdate};
use sqlx::SqlitePool;
use tokio::sync::{Mutex, broadcast, watch};

use super::SyncError;
use super::store::{self, Purged, Pushed};

/// Live-channel limits and timings.
#[derive(Debug, Clone)]
pub struct LiveSettings {
    /// Entries buffered per vault; a connection further behind is closed
    /// with `4002`.
    pub channel_capacity: usize,
    pub max_connections_per_vault: usize,
    /// How long a new connection has to authenticate.
    pub auth_timeout: Duration,
    /// Keepalive ping and session re-check interval.
    pub ping_interval: Duration,
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self {
            channel_capacity: 256,
            max_connections_per_vault: 16,
            auth_timeout: Duration::from_secs(10),
            ping_interval: Duration::from_secs(30),
        }
    }
}

/// One vault's write lock, broadcast channel, and connection count.
pub struct VaultHub {
    write: Mutex<()>,
    entries: broadcast::Sender<Arc<Entry>>,
    connections: AtomicUsize,
}

impl VaultHub {
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Entry>> {
        self.entries.subscribe()
    }
}

/// Holds one of a vault's live-connection slots until dropped.
pub struct Slot(Arc<VaultHub>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Sync state shared by the handlers: per-vault hubs and the shutdown flag.
pub struct SyncState {
    hubs: DashMap<Vec<u8>, Arc<VaultHub>>,
    pub settings: LiveSettings,
    shutdown: watch::Sender<bool>,
}

impl SyncState {
    pub fn new(settings: LiveSettings) -> Self {
        Self {
            hubs: DashMap::new(),
            settings,
            shutdown: watch::Sender::new(false),
        }
    }

    pub fn hub(&self, vault: &[u8]) -> Arc<VaultHub> {
        if let Some(hub) = self.hubs.get(vault) {
            return hub.clone();
        }
        self.hubs
            .entry(vault.to_vec())
            .or_insert_with(|| {
                Arc::new(VaultHub {
                    write: Mutex::new(()),
                    entries: broadcast::channel(self.settings.channel_capacity).0,
                    connections: AtomicUsize::new(0),
                })
            })
            .clone()
    }

    /// Takes a live-connection slot for `vault`, or `None` at the cap.
    pub fn join(&self, vault: &[u8]) -> Option<(Arc<VaultHub>, Slot)> {
        let hub = self.hub(vault);
        let max = self.settings.max_connections_per_vault;
        hub.connections
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < max).then_some(n + 1)
            })
            .ok()?;
        Some((hub.clone(), Slot(hub)))
    }

    /// Tells live connections the server is shutting down.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// [`store::push`], then broadcasts the new entries.
    pub async fn push(
        &self,
        pool: &SqlitePool,
        vault: &[u8],
        updates: &[PushUpdate],
        max_update_bytes: usize,
    ) -> Result<Pushed, SyncError> {
        let hub = self.hub(vault);
        let (pool, vault, updates) = (pool.clone(), vault.to_vec(), updates.to_vec());
        detached(async move {
            let _write = hub.write.lock().await;
            let pushed = store::push(&pool, &vault, &updates, max_update_bytes).await?;
            for entry in &pushed.entries {
                // No receivers is not an error.
                let _ = hub.entries.send(Arc::new(entry.clone()));
            }
            Ok(pushed)
        })
        .await
    }

    /// [`store::snapshot`] under the vault's write lock. Nothing is
    /// broadcast: live clients already have the updates it covers.
    pub async fn snapshot(
        &self,
        pool: &SqlitePool,
        vault: &[u8],
        note: NoteId,
        covers: u64,
        data: &[u8],
    ) -> Result<i64, SyncError> {
        let hub = self.hub(vault);
        let _write = hub.write.lock().await;
        store::snapshot(pool, vault, note, covers, data).await
    }

    /// [`store::purge`], then broadcasts the tombstone.
    pub async fn purge(
        &self,
        pool: &SqlitePool,
        vault: &[u8],
        note: NoteId,
    ) -> Result<Purged, SyncError> {
        let hub = self.hub(vault);
        let (pool, vault) = (pool.clone(), vault.to_vec());
        detached(async move {
            let _write = hub.write.lock().await;
            let purged = store::purge(&pool, &vault, note).await?;
            if let Some(entry) = &purged.entry {
                let _ = hub.entries.send(Arc::new(entry.clone()));
            }
            Ok(purged)
        })
        .await
    }
}

/// Runs a write to completion even if the caller is dropped (a client
/// disconnecting, a request timeout). Otherwise a commit that lands after
/// cancellation would never be broadcast, and live connections would skip it.
async fn detached<T: Send + 'static>(
    write: impl Future<Output = Result<T, SyncError>> + Send + 'static,
) -> Result<T, SyncError> {
    tokio::spawn(write).await.unwrap_or_else(|e| {
        tracing::error!("sync write task failed: {e}");
        Err(SyncError::INTERNAL)
    })
}
