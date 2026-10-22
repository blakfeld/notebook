//! The client sync engine: keeps one vault's local replica in step with the
//! server, the same way on every platform.
//!
//! # Pieces
//!
//! - [`Replica`] is the unlocked vault in memory: every note's document,
//!   backed by a [`Storage`](crate::storage::Storage). Local edits go through
//!   it and are durable (stored and queued in the outbox) before they return.
//!   UI reads (notes, the tag tree, search, export) come from it too.
//! - [`SyncEngine`] is one long-lived task that pulls, pushes, holds the live
//!   channel, snapshots, backs off, and recovers from a server restore. It
//!   owns policy; storage owns durability, ordering, and echo recognition.
//! - [`Transport`] and [`Timer`] are the platform boundary:
//!   `NativeTransport`/`TokioTimer` with the `transport-native` feature,
//!   `WasmTransport`/`BrowserTimer` with `transport-wasm` (on wasm32), and the
//!   in-memory fake in `sync::fake` for tests (`test-util`).
//! - [`SessionSource`] supplies access tokens.
//!
//! # What the host does
//!
//! 1. Opens the store and the replica with the vault key, and wraps the
//!    replica in [`Shared`] so the UI and the engine can both use it.
//! 2. Implements [`SessionSource`]: holds the refresh token (keychain natively,
//!    memory in the browser), refreshes on request, and reports
//!    [`RefreshError::SessionEnded`] when the session is gone.
//! 3. Calls [`SyncEngine::new`] with the replica, a transport, a timer, the
//!    session, a [`SyncConfig`], and a `reopen` callback that opens an empty
//!    store for the same vault (used only to rebuild storage after a server
//!    restore).
//! 4. Spawns [`SyncEngine::run`]: with `wasm_bindgen_futures::spawn_local` in
//!    the browser, or on a current-thread tokio runtime on a dedicated thread
//!    natively. The future isn't `Send` and needn't be.
//! 5. Consumes [`SyncEvent`]s: shows [`SyncStatus`], reloads notes named in
//!    [`SyncEvent::NotesChanged`] from the replica (one that is gone was
//!    purged), and surfaces integrity warnings.
//! 6. Uses the [`SyncHandle`]: [`flush`](SyncHandle::flush) before
//!    backgrounding or closing, [`purge`](SyncHandle::purge) for permanent
//!    deletion, [`set_session`](SyncHandle::set_session) after the user signs
//!    in again, and [`shutdown`](SyncHandle::shutdown) on lock or sign-out.
//!
//! Locks on [`Shared`] are held only inside synchronous calls; never hold one
//! across an `.await`.

mod backoff;
mod engine;
mod replica;
mod session;
mod timer;
mod transport;

#[cfg(any(test, feature = "test-util"))]
pub mod fake;
#[cfg(all(feature = "transport-native", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "transport-native", not(target_arch = "wasm32")))]
pub use native::{NativeLive, NativeTransport, TokioTimer};
#[cfg(all(feature = "transport-wasm", target_arch = "wasm32"))]
mod wasm;
#[cfg(all(feature = "transport-wasm", target_arch = "wasm32"))]
pub use wasm::{BrowserTimer, WasmLive, WasmTransport};
#[cfg(test)]
mod tests;

use std::fmt;
use std::time::Duration;

use futures::channel::{mpsc, oneshot};

pub use engine::SyncEngine;
pub use replica::{EditorSessionId, Replica, ReplicaError};
pub use session::{AccessToken, RefreshError, SessionSource};
pub use timer::Timer;
pub use transport::{LiveChannel, LiveEvent, Transport, TransportError};

use crate::note::NoteId;
use crate::storage::StorageError;

/// Tuning for the engine. The defaults suit a personal vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    /// Snapshot a note once it has more log entries than this.
    pub snapshot_threshold: usize,
    /// How often to pull while the live channel is unavailable but the
    /// server answers.
    pub poll_interval: Duration,
    /// How often to pull anyway while live, in case a frame was missed.
    pub safety_pull_interval: Duration,
    pub backoff_base: Duration,
    pub backoff_cap: Duration,
    /// A live channel open this long resets the backoff.
    pub backoff_reset_after: Duration,
    /// Randomize backoff delays (full jitter). Off only in tests.
    pub jitter: bool,
    /// Most updates per push.
    pub max_batch_updates: usize,
    /// Most ciphertext bytes per push (a single larger update goes alone).
    pub max_batch_bytes: usize,
    /// Wait this long after the last local edit before pushing...
    pub debounce: Duration,
    /// ...but no longer than this after the first unpushed one.
    pub max_debounce: Duration,
    /// Give up on a live push result after this long and retry over HTTP.
    pub push_result_timeout: Duration,
    /// Give up on a new live channel that hasn't said `ready` by then.
    pub ready_timeout: Duration,
    /// Retry pushing this often while the quota is full.
    pub quota_retry: Duration,
    /// Re-authenticate an open live channel this long before its token
    /// expires.
    pub reauth_margin: Duration,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            snapshot_threshold: 200,
            poll_interval: Duration::from_secs(30),
            safety_pull_interval: Duration::from_secs(300),
            backoff_base: Duration::from_secs(1),
            backoff_cap: Duration::from_secs(60),
            backoff_reset_after: Duration::from_secs(10),
            jitter: true,
            max_batch_updates: 100,
            max_batch_bytes: 512 * 1024,
            debounce: Duration::from_millis(300),
            max_debounce: Duration::from_secs(2),
            push_result_timeout: Duration::from_secs(30),
            ready_timeout: Duration::from_secs(10),
            quota_retry: Duration::from_secs(600),
            reauth_margin: Duration::from_secs(60),
        }
    }
}

/// Connection state, as the UI shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Nothing queued, caught up, and the server is reachable.
    Synced,
    /// A request is in flight, or local updates are queued while online.
    Syncing,
    /// The last attempt to reach the server failed.
    Offline,
}

/// A condition the user has to act on (or wait out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    /// The session ended; sync resumes after
    /// [`SyncHandle::set_session`].
    SignInRequired,
    /// Some data is in a document format only a newer client can read.
    UpgradeRequired,
    /// The vault's storage quota is full; pushes are paused.
    QuotaFull,
    /// This note has an update larger than the server accepts; pushes are
    /// paused.
    UpdateTooLarge(NoteId),
    /// The server reported a conflict this client can't resolve; pushes are
    /// paused.
    PushBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStatus {
    pub state: SyncState,
    /// Local updates the server hasn't acknowledged.
    pub pending: usize,
    pub attention: Option<Attention>,
    /// How many integrity warnings were raised since the engine started.
    pub integrity_warnings: u32,
}

/// What the engine tells the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncEvent {
    /// The status changed.
    Status(SyncStatus),
    /// Remote changes created, changed, or removed these notes. Reload them
    /// from the replica (a missing note was purged).
    NotesChanged(Vec<NoteId>),
    /// Data for this note failed decryption or decoding and was skipped.
    IntegrityWarning(NoteId),
}

/// Why a [`SyncHandle`] request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// The server couldn't be reached.
    Offline,
    SignInRequired,
    /// The server asked the client to slow down.
    RateLimited,
    QuotaFull,
    UpdateTooLarge(NoteId),
    /// Pushing stopped after the server reported a conflict this client
    /// can't resolve.
    PushBlocked,
    /// Only trashed notes can be purged.
    NotTrashed(NoteId),
    NoteNotFound(NoteId),
    /// The server refused the request.
    Rejected(TransportError),
    Storage(StorageError),
    /// The engine has shut down.
    Stopped,
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offline => f.write_str("the server can't be reached"),
            Self::SignInRequired => f.write_str("sign-in required"),
            Self::RateLimited => f.write_str("rate limited by the server"),
            Self::QuotaFull => f.write_str("storage quota full"),
            Self::UpdateTooLarge(id) => write!(f, "note {id} has an update that is too large"),
            Self::PushBlocked => f.write_str("pushing is blocked by a conflict"),
            Self::NotTrashed(id) => write!(f, "note {id} is not in the trash"),
            Self::NoteNotFound(id) => write!(f, "note {id} not found"),
            Self::Rejected(err) => write!(f, "server rejected the request: {err}"),
            Self::Storage(err) => err.fmt(f),
            Self::Stopped => f.write_str("sync has stopped"),
        }
    }
}

impl std::error::Error for SyncError {}

enum Command {
    Flush(oneshot::Sender<Result<(), SyncError>>),
    Purge(NoteId, oneshot::Sender<Result<(), SyncError>>),
    SessionChanged,
    Shutdown,
}

/// The host's side of a running engine. Cheap to clone; `Send` on native.
#[derive(Clone)]
pub struct SyncHandle {
    commands: mpsc::UnboundedSender<Command>,
    wake: mpsc::Sender<()>,
}

impl SyncHandle {
    /// Asks the engine to look for work now (the replica does this after
    /// each local edit). Also retries a push paused by a full quota.
    pub fn wake(&self) {
        // Full means a wake is already pending.
        let _ = self.wake.clone().try_send(());
    }

    /// Pushes every queued update now. Completes when the outbox is empty,
    /// or with the reason a push couldn't be made.
    pub async fn flush(&self) -> Result<(), SyncError> {
        self.request(Command::Flush).await
    }

    /// Permanently deletes a trashed note on the server, then locally. Needs
    /// a connection; on failure nothing changes.
    pub async fn purge(&self, note: NoteId) -> Result<(), SyncError> {
        self.request(|reply| Command::Purge(note, reply)).await
    }

    /// Tells the engine the host's [`SessionSource`] has a new session (after
    /// signing in again), so sync resumes.
    pub fn set_session(&self) {
        let _ = self.commands.unbounded_send(Command::SessionChanged);
    }

    /// Stops the engine; its run future completes.
    pub fn shutdown(&self) {
        let _ = self.commands.unbounded_send(Command::Shutdown);
    }

    async fn request(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<(), SyncError>>) -> Command,
    ) -> Result<(), SyncError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .unbounded_send(command(reply))
            .map_err(|_| SyncError::Stopped)?;
        result.await.unwrap_or(Err(SyncError::Stopped))
    }
}

/// A value shared by the host and the engine: `Arc<Mutex<_>>` natively and
/// `Rc<RefCell<_>>` in the browser. Locks are only ever held inside
/// synchronous calls, never across an `.await`.
#[cfg(not(target_arch = "wasm32"))]
pub struct Shared<T>(std::sync::Arc<std::sync::Mutex<T>>);

#[cfg(not(target_arch = "wasm32"))]
impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(value)))
    }

    /// Locks the value. A panic while it was locked doesn't poison it: every
    /// replica write is atomic, so its state is still consistent.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A value shared by the host and the engine: `Arc<Mutex<_>>` natively and
/// `Rc<RefCell<_>>` in the browser. Locks are only ever held inside
/// synchronous calls, never across an `.await`.
#[cfg(target_arch = "wasm32")]
pub struct Shared<T>(std::rc::Rc<std::cell::RefCell<T>>);

#[cfg(target_arch = "wasm32")]
impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self(std::rc::Rc::new(std::cell::RefCell::new(value)))
    }

    /// Borrows the value. Panics if it is already borrowed, which can only
    /// happen if a lock is held across an `.await` or re-entered.
    pub fn lock(&self) -> std::cell::RefMut<'_, T> {
        self.0.borrow_mut()
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
