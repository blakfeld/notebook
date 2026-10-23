//! In-memory [`Storage`] for the browser and tests.

use std::collections::BTreeMap;

use super::{EntryKind, LocalUpdate, LogEntry, LogPos, Result, Storage, StorageError, StoredNote};
use crate::crypto::VaultId;
use crate::note::NoteId;

/// A [`Storage`] whose contents last only as long as the value.
///
/// Every write validates first and then mutates infallibly, so a failing
/// call leaves the store unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryStorage {
    vault: VaultId,
    notes: BTreeMap<NoteId, MemNote>,
    settings: BTreeMap<String, String>,
    next_pos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemNote {
    wrapped_key: Vec<u8>,
    base: Option<Vec<u8>>,
    log: BTreeMap<u64, Vec<u8>>,
}

impl MemoryStorage {
    /// An empty store bound to `vault`.
    pub fn new(vault: VaultId) -> Self {
        Self {
            vault,
            notes: BTreeMap::new(),
            settings: BTreeMap::new(),
            // Matches SQLite's AUTOINCREMENT, which starts at 1.
            next_pos: 1,
        }
    }

    fn push(&mut self, note: NoteId, update: LocalUpdate) {
        let pos = self.next_pos;
        self.next_pos += 1;
        self.notes
            .get_mut(&note)
            .expect("caller checked the note exists")
            .log
            .insert(pos, update.bytes);
    }
}

impl Storage for MemoryStorage {
    fn vault_id(&self) -> VaultId {
        self.vault
    }

    fn list_notes(&self) -> Result<Vec<NoteId>> {
        Ok(self.notes.keys().copied().collect())
    }

    fn load_note(&self, id: NoteId) -> Result<Option<StoredNote>> {
        Ok(self.notes.get(&id).map(|n| StoredNote {
            wrapped_key: n.wrapped_key.clone(),
            base: n.base.clone(),
            entries: n
                .log
                .iter()
                .map(|(&pos, bytes)| LogEntry {
                    pos: LogPos(pos),
                    kind: EntryKind::Update,
                    bytes: bytes.clone(),
                })
                .collect(),
        }))
    }

    fn create_note(&mut self, id: NoteId, wrapped_key: &[u8], initial: LocalUpdate) -> Result<()> {
        if self.notes.contains_key(&id) {
            return Err(StorageError::DuplicateNote(id));
        }
        self.notes.insert(
            id,
            MemNote {
                wrapped_key: wrapped_key.to_vec(),
                base: None,
                log: BTreeMap::new(),
            },
        );
        self.push(id, initial);
        Ok(())
    }

    fn import_note(&mut self, id: NoteId, wrapped_key: &[u8], snapshot: &[u8]) -> Result<()> {
        if self.notes.contains_key(&id) {
            return Err(StorageError::DuplicateNote(id));
        }
        self.notes.insert(
            id,
            MemNote {
                wrapped_key: wrapped_key.to_vec(),
                base: Some(snapshot.to_vec()),
                log: BTreeMap::new(),
            },
        );
        Ok(())
    }

    fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<()> {
        if !self.notes.contains_key(&id) {
            return Err(StorageError::NoteNotFound(id));
        }
        self.push(id, update);
        Ok(())
    }

    fn compact(&mut self, id: NoteId, snapshot: &[u8], through: LogPos) -> Result<()> {
        let note = self
            .notes
            .get_mut(&id)
            .ok_or(StorageError::NoteNotFound(id))?;
        note.log.retain(|pos, _| *pos > through.0);
        note.base = Some(snapshot.to_vec());
        Ok(())
    }

    fn delete_note(&mut self, id: NoteId) -> Result<()> {
        self.notes.remove(&id);
        Ok(())
    }

    fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.settings.get(key).cloned())
    }

    fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.settings.insert(key.to_owned(), value.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::conformance;

    fn vault() -> VaultId {
        VaultId::from_parts(1_000, 1)
    }

    fn note(n: u128) -> NoteId {
        NoteId::from_parts(1_000, n)
    }

    fn local(n: u8) -> LocalUpdate {
        LocalUpdate { bytes: vec![n; 4] }
    }

    #[test]
    fn conformance() {
        conformance::run_all(|| MemoryStorage::new(vault()));
    }

    #[test]
    fn failed_writes_leave_state_unchanged() {
        let mut s = MemoryStorage::new(vault());
        s.create_note(note(1), b"key", local(1)).unwrap();
        s.append_local(note(1), local(2)).unwrap();
        let before = s.clone();

        assert_eq!(
            s.create_note(note(1), b"other", local(3)),
            Err(StorageError::DuplicateNote(note(1)))
        );
        assert_eq!(
            s.import_note(note(1), b"other", b"snap"),
            Err(StorageError::DuplicateNote(note(1)))
        );
        assert_eq!(
            s.append_local(note(2), local(3)),
            Err(StorageError::NoteNotFound(note(2)))
        );
        assert_eq!(
            s.compact(note(2), b"snap", LogPos(1)),
            Err(StorageError::NoteNotFound(note(2)))
        );
        assert_eq!(s, before);
    }
}
