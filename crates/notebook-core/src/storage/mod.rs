//! Local persistence of encrypted note documents and sync bookkeeping.
//!
//! A [`Storage`] holds one vault's local copy: per note, its wrapped key, an
//! optional compacted base snapshot, and an ordered log of encrypted updates
//! and snapshots stored since; the outbox of local updates the server hasn't
//! acknowledged; the sync cursor; and device-local settings.
//!
//! Storage only ever sees ciphertext and wrapped keys. It never opens
//! documents, unwraps keys, or builds indexes; consumers load the bytes and
//! do that in memory.
//!
//! # Log model
//!
//! A note's state is its base snapshot (if any) followed by its log entries
//! in [`LogPos`] order. Remote snapshots are appended to the log rather than
//! replacing the base, because applying a snapshot over more state is
//! idempotent and replacing could drop newer acknowledged updates. Only
//! [`Storage::compact`] replaces the base, and it never removes entries still
//! in the outbox.
//!
//! [`MemoryStorage`] implements the trait for the browser and tests. The
//! native SQLite implementation lives in the `notebook-storage-sqlite` crate.

use std::fmt;

use crate::crypto::VaultId;
use crate::note::NoteId;

mod memory;

#[cfg(any(test, feature = "conformance"))]
pub mod conformance;

pub use memory::MemoryStorage;

/// A server-assigned sequence number in the vault's op log. Zero means
/// nothing has been seen.
///
/// Values must not exceed `i64::MAX`, the range of the server's and the
/// native store's SQLite integers; the native store fails with
/// [`StorageError::Backend`] above it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerSeq(pub u64);

impl fmt::Display for ServerSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A caller-supplied identifier for an update, unique within a store. The sync
/// engine generates one for each local update; remote updates carry theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UpdateId(pub [u8; 16]);

/// A storage-assigned position, strictly increasing across the whole store
/// and never reused. It orders log and outbox entries and is the compaction
/// cutoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogPos(pub u64);

/// Whether a stored payload is an update or a full-state snapshot. The two
/// are encrypted as different payload kinds, so callers must apply each with
/// the matching document method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Update,
    Snapshot,
}

/// An encrypted update made on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUpdate {
    pub id: UpdateId,
    pub bytes: Vec<u8>,
}

/// One entry in a note's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub pos: LogPos,
    /// Absent for remote snapshots.
    pub id: Option<UpdateId>,
    pub kind: EntryKind,
    /// Absent while the entry waits in the outbox.
    pub seq: Option<ServerSeq>,
    pub bytes: Vec<u8>,
}

/// Everything stored for one note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNote {
    pub wrapped_key: Vec<u8>,
    pub base: Option<Vec<u8>>,
    /// Entries after the base, in stored order.
    pub entries: Vec<LogEntry>,
}

/// An encrypted update or snapshot received from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub note: NoteId,
    pub kind: EntryKind,
    /// Absent for snapshots.
    pub id: Option<UpdateId>,
    pub seq: ServerSeq,
    pub bytes: Vec<u8>,
}

/// A batch pulled from the server, applied atomically with its cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBatch {
    /// Wrapped keys for notes that may be new to this store.
    pub keys: Vec<(NoteId, Vec<u8>)>,
    pub entries: Vec<RemoteEntry>,
    pub cursor: ServerSeq,
}

/// A local update waiting for the server to acknowledge it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    pub pos: LogPos,
    pub note: NoteId,
    pub id: UpdateId,
    pub bytes: Vec<u8>,
}

/// Why a storage call failed. Stored bytes are never included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// The store holds data for another vault.
    VaultMismatch {
        stored: VaultId,
        requested: VaultId,
    },
    /// The store was written by a newer client with this layout version.
    SchemaTooNew(u32),
    /// The store can't be read as a valid store.
    Corrupt,
    NoteNotFound(NoteId),
    DuplicateNote(NoteId),
    /// An update with this identifier is already stored.
    DuplicateUpdate,
    CursorRegressed {
        current: ServerSeq,
        requested: ServerSeq,
    },
    /// The backend failed; the message is for logs.
    Backend(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VaultMismatch { stored, requested } => {
                write!(f, "store belongs to vault {stored}, not {requested}")
            }
            Self::SchemaTooNew(v) => write!(f, "store layout version {v} is too new"),
            Self::Corrupt => f.write_str("store is corrupt"),
            Self::NoteNotFound(id) => write!(f, "note {id} not found"),
            Self::DuplicateNote(id) => write!(f, "note {id} already exists"),
            Self::DuplicateUpdate => f.write_str("update identifier already stored"),
            Self::CursorRegressed { current, requested } => {
                write!(f, "cursor {requested} is behind current cursor {current}")
            }
            Self::Backend(msg) => write!(f, "storage backend error: {msg}"),
        }
    }
}

impl std::error::Error for StorageError {}

pub type Result<T> = std::result::Result<T, StorageError>;

/// The local store for one vault.
///
/// Every method that writes is atomic: when it returns an error, nothing it
/// would have changed has changed. Implementations need not be `Send` (wasm
/// values aren't); native implementations are, and callers share one behind
/// a mutex.
pub trait Storage {
    /// The vault this store is bound to.
    fn vault_id(&self) -> VaultId;

    /// Every note ID in the store, in ID order.
    fn list_notes(&self) -> Result<Vec<NoteId>>;

    /// Everything stored for `id`, or `None` if the store doesn't hold it.
    fn load_note(&self, id: NoteId) -> Result<Option<StoredNote>>;

    /// Stores a locally created note: its wrapped key (pending upload) and its
    /// initial update, which also goes in the outbox.
    ///
    /// Fails with [`StorageError::DuplicateNote`] if `id` is stored, or
    /// [`StorageError::DuplicateUpdate`] if the update's ID is.
    fn create_note(&mut self, id: NoteId, wrapped_key: &[u8], initial: LocalUpdate) -> Result<()>;

    /// Stores a note restored from a backup: its wrapped key (not pending
    /// upload) and `snapshot` as its base, with no log or outbox entries.
    ///
    /// Fails with [`StorageError::DuplicateNote`] if `id` is stored.
    fn import_note(&mut self, id: NoteId, wrapped_key: &[u8], snapshot: &[u8]) -> Result<()>;

    /// Appends a local update to the end of note `id`'s log and to the outbox.
    ///
    /// Fails with [`StorageError::NoteNotFound`] if `id` isn't stored, or
    /// [`StorageError::DuplicateUpdate`] if the update's ID is.
    fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<()>;

    /// Pending local updates across all notes, in the order they were stored.
    fn outbox(&self) -> Result<Vec<OutboxEntry>>;

    /// Removes `update` from the outbox and records `seq` on its log entry.
    /// Has no effect if `update` isn't pending.
    fn acknowledge(&mut self, update: UpdateId, seq: ServerSeq) -> Result<()>;

    /// Wrapped keys of locally created notes not yet marked uploaded, in note
    /// ID order.
    fn pending_keys(&self) -> Result<Vec<(NoteId, Vec<u8>)>>;

    /// Marks note `id`'s key as uploaded. Has no effect if `id` isn't stored.
    fn mark_key_uploaded(&mut self, id: NoteId) -> Result<()>;

    /// The highest server sequence whose updates are stored; zero when new.
    fn cursor(&self) -> Result<ServerSeq>;

    /// Applies a remote batch and advances the cursor, all or nothing.
    ///
    /// Keys for notes already stored are ignored. An entry whose update ID is
    /// already stored isn't stored again; if it's pending it's acknowledged
    /// with the entry's sequence instead.
    ///
    /// Fails with [`StorageError::CursorRegressed`] if `batch.cursor` is
    /// behind the current cursor (equal is fine), or
    /// [`StorageError::NoteNotFound`] if an entry's note is neither stored nor
    /// keyed in the batch.
    fn apply_remote(&mut self, batch: RemoteBatch) -> Result<()>;

    /// Replaces note `id`'s base with `snapshot` and removes its log entries
    /// at or before `through`, except those still in the outbox.
    ///
    /// Fails with [`StorageError::NoteNotFound`] if `id` isn't stored.
    fn compact(&mut self, id: NoteId, snapshot: &[u8], through: LogPos) -> Result<()>;

    /// Removes note `id` with its key, base, log, and outbox entries. Has no
    /// effect if `id` isn't stored.
    fn delete_note(&mut self, id: NoteId) -> Result<()>;

    /// Removes every note, log entry, outbox entry, and pending key, resets the
    /// cursor, and releases the vault binding. Settings are kept. Consumes the
    /// store so nothing can write to it unbound.
    fn clear(self) -> Result<()>
    where
        Self: Sized;

    /// The device-local setting `key`, or `None` if never written. Settings
    /// are stored unencrypted, so they must not hold anything sensitive.
    fn setting(&self, key: &str) -> Result<Option<String>>;

    /// Writes the device-local setting `key`, replacing any earlier value.
    fn set_setting(&mut self, key: &str, value: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_seq_orders_numerically() {
        assert!(ServerSeq(0) < ServerSeq(1));
        assert!(ServerSeq(9) < ServerSeq(10));
        assert_eq!(ServerSeq::default(), ServerSeq(0));
    }

    #[test]
    fn errors_display_without_payloads() {
        let vault = VaultId::from_parts(1, 1);
        let other = VaultId::from_parts(1, 2);
        let note = NoteId::from_parts(1, 3);
        let cases = [
            (
                StorageError::VaultMismatch {
                    stored: vault,
                    requested: other,
                },
                format!("store belongs to vault {vault}, not {other}"),
            ),
            (
                StorageError::SchemaTooNew(7),
                "store layout version 7 is too new".into(),
            ),
            (StorageError::Corrupt, "store is corrupt".into()),
            (
                StorageError::NoteNotFound(note),
                format!("note {note} not found"),
            ),
            (
                StorageError::DuplicateNote(note),
                format!("note {note} already exists"),
            ),
            (
                StorageError::DuplicateUpdate,
                "update identifier already stored".into(),
            ),
            (
                StorageError::CursorRegressed {
                    current: ServerSeq(10),
                    requested: ServerSeq(7),
                },
                "cursor 7 is behind current cursor 10".into(),
            ),
            (
                StorageError::Backend("disk I/O error".into()),
                "storage backend error: disk I/O error".into(),
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.to_string(), want);
        }
    }
}
