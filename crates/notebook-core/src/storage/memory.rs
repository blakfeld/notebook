//! In-memory [`Storage`] for the browser and tests.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    EntryKind, LocalUpdate, LogEntry, LogPos, OutboxEntry, RemoteBatch, Result, ServerSeq, Storage,
    StorageError, StoredNote, UpdateId,
};
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
    /// Where each stored update ID lives.
    ids: BTreeMap<UpdateId, (NoteId, u64)>,
    /// Positions of pending entries, with their note.
    outbox: BTreeMap<u64, NoteId>,
    cursor: ServerSeq,
    settings: BTreeMap<String, String>,
    next_pos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemNote {
    wrapped_key: Vec<u8>,
    key_pending: bool,
    base: Option<Vec<u8>>,
    log: BTreeMap<u64, Row>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
    id: Option<UpdateId>,
    kind: EntryKind,
    seq: Option<ServerSeq>,
    bytes: Vec<u8>,
}

impl MemoryStorage {
    /// An empty store bound to `vault`.
    pub fn new(vault: VaultId) -> Self {
        Self {
            vault,
            notes: BTreeMap::new(),
            ids: BTreeMap::new(),
            outbox: BTreeMap::new(),
            cursor: ServerSeq(0),
            settings: BTreeMap::new(),
            // Matches SQLite's AUTOINCREMENT, which starts at 1.
            next_pos: 1,
        }
    }

    fn push(&mut self, note: NoteId, row: Row, pending: bool) {
        let pos = self.next_pos;
        self.next_pos += 1;
        if let Some(id) = row.id {
            self.ids.insert(id, (note, pos));
        }
        if pending {
            self.outbox.insert(pos, note);
        }
        self.notes
            .get_mut(&note)
            .expect("caller checked the note exists")
            .log
            .insert(pos, row);
    }

    fn local_row(update: LocalUpdate) -> Row {
        Row {
            id: Some(update.id),
            kind: EntryKind::Update,
            seq: None,
            bytes: update.bytes,
        }
    }

    /// Marks a pending entry acknowledged, if `id` is pending.
    fn ack(&mut self, id: UpdateId, seq: ServerSeq) {
        let Some(&(note, pos)) = self.ids.get(&id) else {
            return;
        };
        if self.outbox.remove(&pos).is_some() {
            let row = self
                .notes
                .get_mut(&note)
                .and_then(|n| n.log.get_mut(&pos))
                .expect("indexed entry exists");
            row.seq = Some(seq);
        }
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
                .map(|(&pos, row)| LogEntry {
                    pos: LogPos(pos),
                    id: row.id,
                    kind: row.kind,
                    seq: row.seq,
                    bytes: row.bytes.clone(),
                })
                .collect(),
        }))
    }

    fn create_note(&mut self, id: NoteId, wrapped_key: &[u8], initial: LocalUpdate) -> Result<()> {
        if self.notes.contains_key(&id) {
            return Err(StorageError::DuplicateNote(id));
        }
        if self.ids.contains_key(&initial.id) {
            return Err(StorageError::DuplicateUpdate);
        }
        self.notes.insert(
            id,
            MemNote {
                wrapped_key: wrapped_key.to_vec(),
                key_pending: true,
                base: None,
                log: BTreeMap::new(),
            },
        );
        self.push(id, Self::local_row(initial), true);
        Ok(())
    }

    fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<()> {
        if !self.notes.contains_key(&id) {
            return Err(StorageError::NoteNotFound(id));
        }
        if self.ids.contains_key(&update.id) {
            return Err(StorageError::DuplicateUpdate);
        }
        self.push(id, Self::local_row(update), true);
        Ok(())
    }

    fn outbox(&self) -> Result<Vec<OutboxEntry>> {
        Ok(self
            .outbox
            .iter()
            .map(|(&pos, &note)| {
                let row = &self.notes[&note].log[&pos];
                OutboxEntry {
                    pos: LogPos(pos),
                    note,
                    id: row.id.expect("local entries have IDs"),
                    bytes: row.bytes.clone(),
                }
            })
            .collect())
    }

    fn acknowledge(&mut self, update: UpdateId, seq: ServerSeq) -> Result<()> {
        self.ack(update, seq);
        Ok(())
    }

    fn pending_keys(&self) -> Result<Vec<(NoteId, Vec<u8>)>> {
        Ok(self
            .notes
            .iter()
            .filter(|(_, n)| n.key_pending)
            .map(|(&id, n)| (id, n.wrapped_key.clone()))
            .collect())
    }

    fn mark_key_uploaded(&mut self, id: NoteId) -> Result<()> {
        if let Some(n) = self.notes.get_mut(&id) {
            n.key_pending = false;
        }
        Ok(())
    }

    fn cursor(&self) -> Result<ServerSeq> {
        Ok(self.cursor)
    }

    fn apply_remote(&mut self, batch: RemoteBatch) -> Result<()> {
        if batch.cursor < self.cursor {
            return Err(StorageError::CursorRegressed {
                current: self.cursor,
                requested: batch.cursor,
            });
        }
        let keyed: BTreeSet<NoteId> = batch.keys.iter().map(|(id, _)| *id).collect();
        if let Some(e) = batch
            .entries
            .iter()
            .find(|e| !self.notes.contains_key(&e.note) && !keyed.contains(&e.note))
        {
            return Err(StorageError::NoteNotFound(e.note));
        }

        for (id, wrapped_key) in batch.keys {
            self.notes.entry(id).or_insert(MemNote {
                wrapped_key,
                key_pending: false,
                base: None,
                log: BTreeMap::new(),
            });
        }
        for e in batch.entries {
            if let Some(id) = e.id
                && self.ids.contains_key(&id)
            {
                self.ack(id, e.seq);
                continue;
            }
            let row = Row {
                id: e.id,
                kind: e.kind,
                seq: Some(e.seq),
                bytes: e.bytes,
            };
            self.push(e.note, row, false);
        }
        self.cursor = batch.cursor;
        Ok(())
    }

    fn compact(&mut self, id: NoteId, snapshot: &[u8], through: LogPos) -> Result<()> {
        let note = self
            .notes
            .get_mut(&id)
            .ok_or(StorageError::NoteNotFound(id))?;
        let outbox = &self.outbox;
        let ids = &mut self.ids;
        note.log.retain(|pos, row| {
            let keep = *pos > through.0 || outbox.contains_key(pos);
            if !keep && let Some(uid) = row.id {
                ids.remove(&uid);
            }
            keep
        });
        note.base = Some(snapshot.to_vec());
        Ok(())
    }

    fn delete_note(&mut self, id: NoteId) -> Result<()> {
        if let Some(note) = self.notes.remove(&id) {
            for (pos, row) in note.log {
                self.outbox.remove(&pos);
                if let Some(uid) = row.id {
                    self.ids.remove(&uid);
                }
            }
        }
        Ok(())
    }

    fn clear(self) -> Result<()> {
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
    use crate::storage::{RemoteEntry, conformance};

    fn vault() -> VaultId {
        VaultId::from_parts(1_000, 1)
    }

    fn note(n: u128) -> NoteId {
        NoteId::from_parts(1_000, n)
    }

    fn local(n: u8) -> LocalUpdate {
        LocalUpdate {
            id: UpdateId([n; 16]),
            bytes: vec![n; 4],
        }
    }

    #[test]
    fn conformance() {
        conformance::run_all(|| MemoryStorage::new(vault()));
    }

    /// Everything observable through the trait.
    fn observe(s: &MemoryStorage) -> impl PartialEq + std::fmt::Debug + use<> {
        let notes: Vec<_> = s
            .list_notes()
            .unwrap()
            .into_iter()
            .map(|id| (id, s.load_note(id).unwrap()))
            .collect();
        (
            notes,
            s.outbox().unwrap(),
            s.cursor().unwrap(),
            s.pending_keys().unwrap(),
        )
    }

    #[test]
    fn failed_writes_leave_state_unchanged() {
        let mut s = MemoryStorage::new(vault());
        s.create_note(note(1), b"key", local(1)).unwrap();
        s.append_local(note(1), local(2)).unwrap();
        s.apply_remote(RemoteBatch {
            keys: vec![(note(2), b"r".to_vec())],
            entries: vec![],
            cursor: ServerSeq(4),
        })
        .unwrap();
        let before = s.clone();
        let observed = observe(&s);

        let mid_batch_unknown = RemoteBatch {
            keys: vec![(note(3), b"new".to_vec())],
            entries: vec![
                RemoteEntry {
                    note: note(3),
                    kind: EntryKind::Update,
                    id: Some(UpdateId([7; 16])),
                    seq: ServerSeq(5),
                    bytes: vec![7],
                },
                RemoteEntry {
                    note: note(1),
                    kind: EntryKind::Update,
                    id: Some(UpdateId([2; 16])),
                    seq: ServerSeq(6),
                    bytes: vec![2; 4],
                },
                RemoteEntry {
                    note: note(9),
                    kind: EntryKind::Snapshot,
                    id: None,
                    seq: ServerSeq(7),
                    bytes: vec![9],
                },
            ],
            cursor: ServerSeq(7),
        };
        assert_eq!(
            s.apply_remote(mid_batch_unknown),
            Err(StorageError::NoteNotFound(note(9)))
        );
        assert_eq!(s, before);
        assert_eq!(observe(&s), observed);

        assert_eq!(
            s.create_note(note(1), b"other", local(3)),
            Err(StorageError::DuplicateNote(note(1)))
        );
        assert_eq!(
            s.create_note(note(4), b"other", local(2)),
            Err(StorageError::DuplicateUpdate)
        );
        assert_eq!(s, before);
        assert_eq!(observe(&s), observed);
    }
}
