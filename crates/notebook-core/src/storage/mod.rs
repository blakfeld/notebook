//! Local persistence of encrypted note documents.
//!
//! A [`Storage`] holds one vault's notes: per note, its wrapped key, an
//! optional compacted base snapshot, and an ordered log of encrypted updates
//! stored since; plus device-local settings.
//!
//! Storage only ever sees ciphertext and wrapped keys. It never opens
//! documents, unwraps keys, or builds indexes; consumers load the bytes and
//! do that in memory.
//!
//! # Log model
//!
//! A note's state is its base snapshot (if any) followed by its log entries
//! in [`LogPos`] order. Only [`Storage::compact`] replaces the base.
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

/// A storage-assigned position, strictly increasing across the whole store
/// and never reused. It orders log entries and is the compaction cutoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogPos(pub u64);

/// Whether a stored payload is an update or a full-state snapshot. The two
/// are encrypted as different payload kinds, so callers must apply each with
/// the matching document method. Stores written while the app synced may
/// hold snapshots in a note's log; new entries are always updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Update,
    Snapshot,
}

/// An encrypted update made on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUpdate {
    pub bytes: Vec<u8>,
}

/// One entry in a note's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub pos: LogPos,
    pub kind: EntryKind,
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

    /// Stores a new note: its wrapped key and its initial update.
    ///
    /// Fails with [`StorageError::DuplicateNote`] if `id` is stored.
    fn create_note(&mut self, id: NoteId, wrapped_key: &[u8], initial: LocalUpdate) -> Result<()>;

    /// Stores a note restored from a backup: its wrapped key and `snapshot`
    /// as its base, with no log entries.
    ///
    /// Fails with [`StorageError::DuplicateNote`] if `id` is stored.
    fn import_note(&mut self, id: NoteId, wrapped_key: &[u8], snapshot: &[u8]) -> Result<()>;

    /// Appends a local update to the end of note `id`'s log.
    ///
    /// Fails with [`StorageError::NoteNotFound`] if `id` isn't stored.
    fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<()>;

    /// Replaces note `id`'s base with `snapshot` and removes its log entries
    /// at or before `through`.
    ///
    /// Fails with [`StorageError::NoteNotFound`] if `id` isn't stored.
    fn compact(&mut self, id: NoteId, snapshot: &[u8], through: LogPos) -> Result<()>;

    /// Removes note `id` with its key, base, and log. Has no effect if `id`
    /// isn't stored.
    fn delete_note(&mut self, id: NoteId) -> Result<()>;

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
                StorageError::Backend("disk I/O error".into()),
                "storage backend error: disk I/O error".into(),
            ),
        ];
        for (err, want) in cases {
            assert_eq!(err.to_string(), want);
        }
    }
}
