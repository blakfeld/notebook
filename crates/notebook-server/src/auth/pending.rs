//! Server state held between the start and finish of a handshake: single
//! use, expiring, and capped so floods stay bounded in memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand_core::RngCore;
use tokio::time::Instant;

use notebook_core::auth::SecureRng;

/// How long a handshake may take between start and finish.
pub const HANDSHAKE_TTL: Duration = Duration::from_secs(60);

/// How many handshakes may be in flight at once.
pub const MAX_PENDING: usize = 10_000;

/// How often expired entries are dropped.
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// The map is full; the caller answers `503`.
#[derive(Debug, PartialEq, Eq)]
pub struct Full;

/// Entries keyed by a random handshake ID, expiring [`HANDSHAKE_TTL`] after
/// insertion. Uses tokio's clock, so tests can pause time.
pub struct PendingMap<T> {
    entries: Mutex<HashMap<String, (Instant, T)>>,
    cap: usize,
}

impl<T> PendingMap<T> {
    pub fn new() -> Self {
        Self::with_cap(MAX_PENDING)
    }

    pub fn with_cap(cap: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            cap,
        }
    }

    /// Stores `value` under a new random ID, or [`Full`] at the cap.
    pub fn insert(&self, value: T) -> Result<String, Full> {
        let mut id = [0u8; 16];
        SecureRng.fill_bytes(&mut id);
        let id = URL_SAFE_NO_PAD.encode(id);
        let mut entries = self.lock();
        if entries.len() >= self.cap {
            // Expired entries don't count against the cap.
            let now = Instant::now();
            entries.retain(|_, (expires, _)| *expires > now);
            if entries.len() >= self.cap {
                return Err(Full);
            }
        }
        entries.insert(id.clone(), (Instant::now() + HANDSHAKE_TTL, value));
        Ok(id)
    }

    /// Removes and returns the entry, unless it is unknown or expired.
    pub fn take(&self, id: &str) -> Option<T> {
        let (expires, value) = self.lock().remove(id)?;
        (expires > Instant::now()).then_some(value)
    }

    /// Drops expired entries.
    pub fn sweep(&self) {
        let now = Instant::now();
        self.lock().retain(|_, (expires, _)| *expires > now);
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Instant, T)>> {
        // A panic while holding the lock leaves the map consistent.
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl<T> Default for PendingMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sweeps `map` every 30 seconds until it is dropped.
pub fn spawn_sweep_task<T: Send + 'static>(
    map: &Arc<PendingMap<T>>,
) -> tokio::task::JoinHandle<()> {
    let weak: Weak<PendingMap<T>> = Arc::downgrade(map);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            match weak.upgrade() {
                Some(map) => map.sweep(),
                None => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn entries_expire_after_the_ttl() {
        let map = PendingMap::new();
        let id = map.insert(1).unwrap();
        tokio::time::advance(HANDSHAKE_TTL + Duration::from_millis(1)).await;
        assert_eq!(map.take(&id), None);

        let id = map.insert(2).unwrap();
        tokio::time::advance(HANDSHAKE_TTL - Duration::from_secs(1)).await;
        assert_eq!(map.take(&id), Some(2));
    }

    #[tokio::test]
    async fn entries_are_single_use() {
        let map = PendingMap::new();
        let id = map.insert("x").unwrap();
        assert_eq!(map.take(&id), Some("x"));
        assert_eq!(map.take(&id), None);
        assert_eq!(map.take("unknown"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn cap_rejects_until_entries_expire() {
        let map = PendingMap::with_cap(3);
        for i in 0..3 {
            map.insert(i).unwrap();
        }
        assert_eq!(map.insert(9), Err(Full));
        tokio::time::advance(HANDSHAKE_TTL + Duration::from_secs(1)).await;
        assert!(map.insert(9).is_ok());
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn default_cap() {
        assert_eq!(PendingMap::<()>::new().cap, 10_000);
    }

    #[tokio::test(start_paused = true)]
    async fn sweep_drops_expired_entries() {
        let map = Arc::new(PendingMap::new());
        map.insert(()).unwrap();
        let _task = spawn_sweep_task(&map);
        // Paused time auto-advances through the task's ticks while we sleep.
        tokio::time::sleep(HANDSHAKE_TTL + 2 * SWEEP_INTERVAL).await;
        assert!(map.is_empty());
    }

    #[test]
    fn ids_are_random_and_unguessable() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let map = PendingMap::new();
            let a = map.insert(()).unwrap();
            let b = map.insert(()).unwrap();
            assert_ne!(a, b);
            assert_eq!(URL_SAFE_NO_PAD.decode(&a).unwrap().len(), 16);
        });
    }
}
