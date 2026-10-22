//! The unlocked vault in memory: every note's document, kept in step with
//! storage. Synchronous and free of network code; the engine drives the
//! remote side through the crate-private methods.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use futures::channel::mpsc;

use crate::crypto::{CryptoError, NoteKey, VaultId, VaultKey, fill_random};
use crate::document::{DocumentError, EncryptedSnapshot, EncryptedUpdate, NoteDocument};
use crate::note::{Note, NoteId, Timestamp};
use crate::protocol::{ChangesResponse, Entry};
use crate::storage::{
    EntryKind, LocalUpdate, LogPos, RemoteBatch, RemoteEntry, ServerSeq, Storage, StorageError,
    StoredNote, UpdateId,
};

/// Why a replica operation failed. On error nothing changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaError {
    Storage(StorageError),
    Document(DocumentError),
    Crypto(CryptoError),
    NoteNotFound(NoteId),
}

impl fmt::Display for ReplicaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(err) => err.fmt(f),
            Self::Document(err) => err.fmt(f),
            Self::Crypto(err) => err.fmt(f),
            Self::NoteNotFound(id) => write!(f, "note {id} not found"),
        }
    }
}

impl std::error::Error for ReplicaError {}

impl From<StorageError> for ReplicaError {
    fn from(err: StorageError) -> Self {
        Self::Storage(err)
    }
}

impl From<DocumentError> for ReplicaError {
    fn from(err: DocumentError) -> Self {
        Self::Document(err)
    }
}

impl From<CryptoError> for ReplicaError {
    fn from(err: CryptoError) -> Self {
        Self::Crypto(err)
    }
}

/// One open note.
struct Open {
    doc: NoteDocument,
    wrapped_key: Vec<u8>,
    /// Log entries since the base, counted at open and on every append.
    entries: usize,
    /// Snapshot once `entries` exceeds this; `None` uses the threshold.
    trigger: Option<usize>,
    /// Holds an entry in a format this build can't apply. Compacting would
    /// drop it, so the note is never snapshotted.
    unapplied: bool,
}

/// Problems found while applying stored or remote data.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Issues {
    /// Notes with data that failed decryption or decoding.
    pub(crate) integrity: BTreeSet<NoteId>,
    /// Whether any data was in a format this build doesn't know.
    pub(crate) upgrade_required: bool,
}

impl Issues {
    fn record(&mut self, note: NoteId, err: &DocumentError) {
        match err {
            DocumentError::UnsupportedFormat(_) => self.upgrade_required = true,
            DocumentError::Crypto(_) | DocumentError::Malformed => {
                self.integrity.insert(note);
            }
        }
    }

    pub(crate) fn merge(&mut self, other: Issues) {
        self.integrity.extend(other.integrity);
        self.upgrade_required |= other.upgrade_required;
    }
}

/// The result of applying a page of remote changes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    /// Notes whose materialized state changed, created and purged included.
    pub(crate) changed: BTreeSet<NoteId>,
    pub(crate) issues: Issues,
}

/// A snapshot ready to upload, and the log position it covers locally.
pub(crate) struct PreparedSnapshot {
    pub(crate) snapshot: EncryptedSnapshot,
    pub(crate) through: LogPos,
}

/// The unlocked vault: storage plus every note's open document.
///
/// Edits go through here so each one is durable before it returns. The
/// host shares the replica with the engine through [`Shared`](super::Shared)
/// and reads notes from it directly.
pub struct Replica<S: Storage> {
    vault: VaultId,
    vault_key: VaultKey,
    /// `None` only if a resync failed between clearing and reopening.
    storage: Option<S>,
    notes: BTreeMap<NoteId, Open>,
    /// Stored notes whose key won't unwrap; reported once, then left alone.
    unopenable: BTreeSet<NoteId>,
    /// Problems found at open, not yet reported.
    open_issues: Issues,
    wake: Option<mpsc::Sender<()>>,
}

impl<S: Storage> Replica<S> {
    /// Opens every stored note: unwraps its key, loads its base, and replays
    /// its log. Data that fails decryption or decoding is skipped, and data
    /// in an unknown format is left unapplied; both are reported to the
    /// engine when it starts. A note whose key won't unwrap stays in storage
    /// but isn't shown.
    pub fn open(storage: S, vault_key: VaultKey) -> Result<Self, StorageError> {
        let vault = storage.vault_id();
        let mut replica = Self {
            vault,
            vault_key,
            storage: None,
            notes: BTreeMap::new(),
            unopenable: BTreeSet::new(),
            open_issues: Issues::default(),
            wake: None,
        };
        for id in storage.list_notes()? {
            if let Some(stored) = storage.load_note(id)? {
                let (open, issues) = replica.build(id, &stored);
                replica.open_issues.merge(issues);
                match open {
                    Some(open) => {
                        replica.notes.insert(id, open);
                    }
                    None => {
                        replica.unopenable.insert(id);
                    }
                }
            }
        }
        replica.storage = Some(storage);
        Ok(replica)
    }

    pub fn vault_id(&self) -> VaultId {
        self.vault
    }

    /// Every readable note, in ID order. Notes whose creating update hasn't
    /// arrived yet are left out.
    pub fn notes(&self) -> Vec<Note> {
        self.notes.values().filter_map(|n| n.doc.note()).collect()
    }

    pub fn note(&self, id: NoteId) -> Option<Note> {
        self.notes.get(&id).and_then(|n| n.doc.note())
    }

    /// Creates a note with a fresh key, stored (key pending upload) and
    /// queued for push before this returns.
    pub fn create_note(&mut self, body: &str, now: Timestamp) -> Result<NoteId, ReplicaError> {
        let id = NoteId::from_parts(now.as_millis().max(0) as u64, random_u128()?);
        let key = NoteKey::generate()?;
        let wrapped_key = self.vault_key.wrap_note_key(self.vault, id, &key)?;
        let (doc, update) = NoteDocument::create(self.vault, id, key, body, now)?;
        let initial = local_update(update)?;
        self.store_mut()?.create_note(id, &wrapped_key, initial)?;
        self.notes.insert(
            id,
            Open {
                doc,
                wrapped_key,
                entries: 1,
                trigger: None,
                unapplied: false,
            },
        );
        self.poke();
        Ok(id)
    }

    /// Sets note `id`'s body. Does nothing if it's unchanged.
    pub fn set_body(&mut self, id: NoteId, body: &str, now: Timestamp) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_body(body, now))
    }

    pub fn set_pinned(&mut self, id: NoteId, pinned: bool) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_pinned(pinned))
    }

    pub fn set_archived(&mut self, id: NoteId, archived: bool) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_archived(archived))
    }

    pub fn set_trashed(&mut self, id: NoteId, trashed: bool) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_trashed(trashed))
    }

    /// The number of local updates not yet acknowledged by the server.
    pub fn pending(&self) -> Result<usize, StorageError> {
        Ok(self.store()?.outbox()?.len())
    }

    /// Applies a local change, then stores it. If storing fails, the
    /// document is rebuilt from storage, since a document can't undo.
    fn edit(
        &mut self,
        id: NoteId,
        change: impl FnOnce(&mut NoteDocument) -> Result<Option<EncryptedUpdate>, DocumentError>,
    ) -> Result<(), ReplicaError> {
        let open = self
            .notes
            .get_mut(&id)
            .ok_or(ReplicaError::NoteNotFound(id))?;
        let stored = match change(&mut open.doc) {
            Ok(None) => return Ok(()),
            Ok(Some(update)) => local_update(update)
                .map_err(ReplicaError::from)
                .and_then(|update| Ok(self.store_mut()?.append_local(id, update)?)),
            Err(err) => Err(err.into()),
        };
        match stored {
            Ok(()) => {
                if let Some(open) = self.notes.get_mut(&id) {
                    open.entries += 1;
                }
                self.poke();
                Ok(())
            }
            Err(err) => {
                self.reload(id);
                Err(err)
            }
        }
    }

    /// Rebuilds note `id` from storage. If storage can't produce it, the
    /// document in memory is kept: it may be the only copy (for example
    /// while a resync is pending), and losing it is worse than a document
    /// that is one failed edit ahead of storage.
    fn reload(&mut self, id: NoteId) {
        let stored = self
            .store()
            .ok()
            .and_then(|s| s.load_note(id).ok().flatten());
        if let Some((Some(open), _)) = stored.map(|stored| self.build(id, &stored)) {
            self.notes.insert(id, open);
        }
    }

    /// Opens a note's document from its stored data.
    fn build(&self, id: NoteId, stored: &StoredNote) -> (Option<Open>, Issues) {
        let mut issues = Issues::default();
        let key = match self
            .vault_key
            .unwrap_note_key(self.vault, id, &stored.wrapped_key)
        {
            Ok(key) => key,
            Err(_) => {
                issues.integrity.insert(id);
                return (None, issues);
            }
        };
        let mut doc = match NoteDocument::open(self.vault, id, key) {
            Ok(doc) => doc,
            Err(err) => {
                issues.record(id, &err);
                return (None, issues);
            }
        };
        let mut unapplied = false;
        let base = stored.base.iter().map(|b| (EntryKind::Snapshot, b));
        let log = stored.entries.iter().map(|e| (e.kind, &e.bytes));
        for (kind, bytes) in base.chain(log) {
            if let Err(err) = apply_entry(&mut doc, kind, bytes) {
                unapplied |= matches!(err, DocumentError::UnsupportedFormat(_));
                issues.record(id, &err);
            }
        }
        let open = Open {
            doc,
            wrapped_key: stored.wrapped_key.clone(),
            entries: stored.entries.len(),
            trigger: None,
            unapplied,
        };
        (Some(open), issues)
    }

    fn poke(&mut self) {
        if let Some(wake) = &mut self.wake {
            // Full means a wake is already pending.
            let _ = wake.try_send(());
        }
    }

    fn store(&self) -> Result<&S, StorageError> {
        self.storage.as_ref().ok_or_else(closed)
    }

    fn store_mut(&mut self) -> Result<&mut S, StorageError> {
        self.storage.as_mut().ok_or_else(closed)
    }

    // ---- Engine interface ----

    pub(crate) fn set_wake(&mut self, wake: mpsc::Sender<()>) {
        self.wake = Some(wake);
    }

    pub(crate) fn take_open_issues(&mut self) -> Issues {
        std::mem::take(&mut self.open_issues)
    }

    /// Read access to the underlying store (for export and diagnostics).
    /// Writes go through the replica so memory and storage stay in step.
    pub fn storage(&self) -> Result<&S, StorageError> {
        self.store()
    }

    /// For bookkeeping that doesn't touch note data: acknowledgments and key
    /// upload marks.
    pub(crate) fn storage_mut(&mut self) -> Result<&mut S, StorageError> {
        self.store_mut()
    }

    /// Applies one page of remote changes: deletes purged notes, stores the
    /// rest with the page's cursor in one transaction, then applies it to the
    /// documents. Data that fails to apply is reported in [`Applied::issues`]
    /// and doesn't stop the rest; the cursor moves past it either way.
    pub(crate) fn apply_page(&mut self, page: &ChangesResponse) -> Result<Applied, StorageError> {
        let mut applied = Applied::default();
        let purged: BTreeSet<NoteId> = page
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Purge { note, .. } => Some(*note),
                Entry::Update { .. } => None,
            })
            .collect();
        for &note in &purged {
            if self.delete_note(note)? {
                applied.changed.insert(note);
            }
        }

        let mut keys: BTreeMap<NoteId, Vec<u8>> = BTreeMap::new();
        let mut entries = Vec::new();
        for n in &page.notes {
            if purged.contains(&n.note) {
                continue;
            }
            if let Some(key) = &n.key {
                keys.entry(n.note).or_insert_with(|| key.0.clone());
            }
            if let Some(snapshot) = &n.snapshot {
                entries.push(RemoteEntry {
                    note: n.note,
                    kind: EntryKind::Snapshot,
                    id: None,
                    seq: ServerSeq(snapshot.covers),
                    bytes: snapshot.data.0.clone(),
                });
            }
        }
        for e in &page.entries {
            if let Entry::Update {
                seq,
                id,
                note,
                data,
                note_key,
            } = e
                && !purged.contains(note)
            {
                if let Some(key) = note_key {
                    keys.entry(*note).or_insert_with(|| key.0.clone());
                }
                entries.push(RemoteEntry {
                    note: *note,
                    kind: EntryKind::Update,
                    id: Some(UpdateId::from(*id)),
                    seq: ServerSeq(*seq),
                    bytes: data.0.clone(),
                });
            }
        }
        // Data for a note this store has no key for can't be stored or read.
        // The protocol never sends it; if a server does, skip it rather than
        // fail every pull from here on.
        let stored: BTreeSet<NoteId> = self.store()?.list_notes()?.into_iter().collect();
        entries.retain(|e| {
            let known = stored.contains(&e.note) || keys.contains_key(&e.note);
            if !known {
                applied.issues.integrity.insert(e.note);
            }
            known
        });

        self.store_mut()?.apply_remote(RemoteBatch {
            keys: keys.iter().map(|(n, k)| (*n, k.clone())).collect(),
            entries: entries.clone(),
            cursor: ServerSeq(page.cursor),
        })?;

        let mut by_note: BTreeMap<NoteId, Vec<&RemoteEntry>> =
            keys.keys().map(|note| (*note, Vec::new())).collect();
        for e in &entries {
            by_note.entry(e.note).or_default().push(e);
        }
        for (note, list) in by_note {
            if !self.notes.contains_key(&note) {
                if self.unopenable.contains(&note) {
                    // Already reported when it failed to open.
                    continue;
                }
                // New to memory: open it from storage, which now holds
                // everything received for it, including this page. Its key
                // may have come on an earlier page than its data.
                let Some(stored) = self.store()?.load_note(note)? else {
                    continue;
                };
                let (open, issues) = self.build(note, &stored);
                applied.issues.merge(issues);
                match open {
                    Some(open) => {
                        if open.doc.note().is_some() {
                            applied.changed.insert(note);
                        }
                        self.notes.insert(note, open);
                    }
                    None => {
                        self.unopenable.insert(note);
                    }
                }
                continue;
            }
            // Echoes of this device's updates aren't stored twice, so count
            // what storage holds rather than what arrived.
            let stored_entries = self
                .store()?
                .load_note(note)?
                .map_or(0, |n| n.entries.len());
            let Some(open) = self.notes.get_mut(&note) else {
                continue;
            };
            let before = open.doc.note();
            for e in &list {
                if let Err(err) = apply_entry(&mut open.doc, e.kind, &e.bytes) {
                    open.unapplied |= matches!(err, DocumentError::UnsupportedFormat(_));
                    applied.issues.record(note, &err);
                }
            }
            open.entries = stored_entries;
            if open.doc.note() != before {
                applied.changed.insert(note);
            }
        }
        Ok(applied)
    }

    /// Deletes note `id` from storage (with its outbox entries) and memory.
    /// Returns whether it was open.
    pub(crate) fn delete_note(&mut self, id: NoteId) -> Result<bool, StorageError> {
        self.store_mut()?.delete_note(id)?;
        self.unopenable.remove(&id);
        Ok(self.notes.remove(&id).is_some())
    }

    /// A note due for a snapshot: more entries than its trigger, nothing in
    /// the outbox, and nothing left unapplied.
    pub(crate) fn snapshot_candidate(
        &self,
        threshold: usize,
        outbox_notes: &BTreeSet<NoteId>,
    ) -> Option<NoteId> {
        self.notes
            .iter()
            .find(|(id, open)| {
                !open.unapplied
                    && !outbox_notes.contains(id)
                    && open.entries > open.trigger.unwrap_or(threshold)
            })
            .map(|(id, _)| *id)
    }

    /// Takes a snapshot of note `id` and records how far its stored log
    /// reaches, so entries stored after this point survive compaction.
    pub(crate) fn prepare_snapshot(
        &self,
        id: NoteId,
    ) -> Result<Option<PreparedSnapshot>, ReplicaError> {
        let open = self.notes.get(&id).ok_or(ReplicaError::NoteNotFound(id))?;
        let stored = self
            .store()?
            .load_note(id)?
            .ok_or(ReplicaError::NoteNotFound(id))?;
        let Some(through) = stored.entries.iter().map(|e| e.pos).max() else {
            return Ok(None);
        };
        let snapshot = open.doc.snapshot()?;
        Ok(Some(PreparedSnapshot { snapshot, through }))
    }

    /// Replaces note `id`'s stored log through `through` with `snapshot`.
    pub(crate) fn compact_note(
        &mut self,
        id: NoteId,
        snapshot: &EncryptedSnapshot,
        through: LogPos,
    ) -> Result<(), StorageError> {
        let store = self.store_mut()?;
        store.compact(id, snapshot.as_bytes(), through)?;
        let remaining = store.load_note(id)?.map_or(0, |n| n.entries.len());
        if let Some(open) = self.notes.get_mut(&id) {
            open.entries = remaining;
            open.trigger = None;
        }
        Ok(())
    }

    /// Holds off note `id`'s next snapshot until `threshold` more entries
    /// arrive.
    pub(crate) fn defer_snapshot(&mut self, id: NoteId, threshold: usize) {
        if let Some(open) = self.notes.get_mut(&id) {
            open.trigger = Some(open.entries + threshold);
        }
    }

    /// Each open note's wrapped key and its full state as one update.
    pub(crate) fn export_for_resync(
        &self,
    ) -> Result<Vec<(NoteId, Vec<u8>, EncryptedUpdate)>, ReplicaError> {
        self.notes
            .iter()
            .map(|(id, open)| Ok((*id, open.wrapped_key.clone(), open.doc.full_update()?)))
            .collect()
    }

    /// Rebuilds storage from memory after a server restore: clears the
    /// store, opens a fresh one with `reopen`, and stores every note again
    /// as one pending key and one queued full-state update, at cursor zero.
    /// The documents themselves are unchanged.
    pub(crate) fn rebuild_storage(
        &mut self,
        reopen: &mut dyn FnMut() -> Result<S, StorageError>,
    ) -> Result<(), ReplicaError> {
        let export = self.export_for_resync()?;
        if let Some(old) = self.storage.take() {
            old.clear()?;
        }
        self.unopenable.clear();
        let mut store = reopen()?;
        let mut result = Ok(());
        for (id, wrapped_key, update) in export {
            let stored = local_update(update)
                .map_err(ReplicaError::from)
                .and_then(|u| Ok(store.create_note(id, &wrapped_key, u)?));
            if let Err(err) = stored {
                result = Err(err);
                break;
            }
            if let Some(open) = self.notes.get_mut(&id) {
                open.entries = 1;
                open.trigger = None;
            }
        }
        self.storage = Some(store);
        result
    }
}

impl<S: Storage> fmt::Debug for Replica<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replica")
            .field("vault", &self.vault)
            .field("notes", &self.notes.len())
            .finish_non_exhaustive()
    }
}

fn apply_entry(doc: &mut NoteDocument, kind: EntryKind, bytes: &[u8]) -> Result<(), DocumentError> {
    match kind {
        EntryKind::Update => doc.apply_update(bytes),
        EntryKind::Snapshot => doc.load_snapshot(bytes),
    }
}

fn closed() -> StorageError {
    StorageError::Backend("store closed by a failed resync".into())
}

fn random_u128() -> Result<u128, CryptoError> {
    let mut bytes = [0; 16];
    fill_random(&mut bytes)?;
    Ok(u128::from_le_bytes(bytes))
}

/// A fresh random update ID.
pub(crate) fn new_update_id() -> Result<UpdateId, CryptoError> {
    let mut bytes = [0; 16];
    fill_random(&mut bytes)?;
    Ok(UpdateId(bytes))
}

fn local_update(update: EncryptedUpdate) -> Result<LocalUpdate, CryptoError> {
    Ok(LocalUpdate {
        id: new_update_id()?,
        bytes: update.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::PayloadKind;
    use crate::protocol::{Bytes, NoteEntry, OpId, Snapshot};
    use crate::storage::{MemoryStorage, OutboxEntry};
    use crate::sync::fake::vault_keys;

    const T1: Timestamp = Timestamp(1_700_000_000_000);
    const T2: Timestamp = Timestamp(1_700_000_001_000);

    fn vault() -> VaultId {
        VaultId::from_parts(1, 1)
    }

    /// Two replicas of one vault, each with its own store.
    fn pair() -> (Replica<MemoryStorage>, Replica<MemoryStorage>) {
        let [a, b] = vault_keys(vault());
        (
            Replica::open(MemoryStorage::new(vault()), a).unwrap(),
            Replica::open(MemoryStorage::new(vault()), b).unwrap(),
        )
    }

    /// A new replica on a copy of `r`'s store, as after a restart.
    fn reopen(r: &Replica<MemoryStorage>) -> Replica<MemoryStorage> {
        let [key] = vault_keys(vault());
        Replica::open(r.storage().unwrap().clone(), key).unwrap()
    }

    /// What a server would send for `r`'s outbox, numbered from `seq`.
    fn page_of_outbox(r: &Replica<MemoryStorage>, seq: u64) -> ChangesResponse {
        let store = r.storage().unwrap();
        let keys: BTreeMap<_, _> = store.pending_keys().unwrap().into_iter().collect();
        let mut notes: Vec<NoteEntry> = Vec::new();
        let mut entries = Vec::new();
        for (i, e) in store.outbox().unwrap().into_iter().enumerate() {
            if let Some(key) = keys.get(&e.note)
                && !notes.iter().any(|n| n.note == e.note)
            {
                notes.push(NoteEntry {
                    note: e.note,
                    key: Some(Bytes(key.clone())),
                    snapshot: None,
                });
            }
            entries.push(Entry::Update {
                seq: seq + i as u64,
                id: OpId::from(e.id),
                note: e.note,
                data: Bytes(e.bytes),
                note_key: None,
            });
        }
        let cursor = entries.last().map_or(seq, Entry::seq);
        ChangesResponse {
            notes,
            entries,
            cursor,
            more: false,
        }
    }

    /// A store whose next local append fails.
    struct Failing {
        inner: MemoryStorage,
        fail: bool,
    }

    impl Storage for Failing {
        fn vault_id(&self) -> VaultId {
            self.inner.vault_id()
        }
        fn list_notes(&self) -> Result<Vec<NoteId>, StorageError> {
            self.inner.list_notes()
        }
        fn load_note(&self, id: NoteId) -> Result<Option<StoredNote>, StorageError> {
            self.inner.load_note(id)
        }
        fn create_note(
            &mut self,
            id: NoteId,
            wrapped_key: &[u8],
            initial: LocalUpdate,
        ) -> Result<(), StorageError> {
            self.inner.create_note(id, wrapped_key, initial)
        }
        fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<(), StorageError> {
            if std::mem::take(&mut self.fail) {
                return Err(StorageError::Backend("disk full".into()));
            }
            self.inner.append_local(id, update)
        }
        fn outbox(&self) -> Result<Vec<OutboxEntry>, StorageError> {
            self.inner.outbox()
        }
        fn acknowledge(&mut self, update: UpdateId, seq: ServerSeq) -> Result<(), StorageError> {
            self.inner.acknowledge(update, seq)
        }
        fn pending_keys(&self) -> Result<Vec<(NoteId, Vec<u8>)>, StorageError> {
            self.inner.pending_keys()
        }
        fn mark_key_uploaded(&mut self, id: NoteId) -> Result<(), StorageError> {
            self.inner.mark_key_uploaded(id)
        }
        fn cursor(&self) -> Result<ServerSeq, StorageError> {
            self.inner.cursor()
        }
        fn apply_remote(&mut self, batch: RemoteBatch) -> Result<(), StorageError> {
            self.inner.apply_remote(batch)
        }
        fn compact(
            &mut self,
            id: NoteId,
            snapshot: &[u8],
            through: LogPos,
        ) -> Result<(), StorageError> {
            self.inner.compact(id, snapshot, through)
        }
        fn delete_note(&mut self, id: NoteId) -> Result<(), StorageError> {
            self.inner.delete_note(id)
        }
        fn clear(self) -> Result<(), StorageError> {
            self.inner.clear()
        }
        fn setting(&self, key: &str) -> Result<Option<String>, StorageError> {
            self.inner.setting(key)
        }
        fn set_setting(&mut self, key: &str, value: &str) -> Result<(), StorageError> {
            self.inner.set_setting(key, value)
        }
    }

    #[test]
    fn reopened_replica_has_the_same_notes() {
        let (mut a, _) = pair();
        let one = a.create_note("one #tag", T1).unwrap();
        let two = a.create_note("two", T1).unwrap();
        a.set_body(one, "one #tag edited", T2).unwrap();
        a.set_pinned(two, true).unwrap();
        a.set_archived(two, true).unwrap();
        a.set_trashed(one, true).unwrap();
        let again = reopen(&a);
        assert_eq!(again.notes(), a.notes());
        assert_eq!(again.notes().len(), 2);
        assert!(again.note(two).unwrap().is_pinned());
    }

    #[test]
    fn tampered_stored_entry_is_skipped_at_open() {
        let (mut a, _) = pair();
        let id = a.create_note("one", T1).unwrap();
        a.set_body(id, "one two", T2).unwrap();
        let before = a.note(id).unwrap();
        let mut bytes = a.storage().unwrap().load_note(id).unwrap().unwrap().entries[1]
            .bytes
            .clone();
        *bytes.last_mut().unwrap() ^= 1;
        let id_tampered = new_update_id().unwrap();
        a.storage_mut()
            .unwrap()
            .append_local(
                id,
                LocalUpdate {
                    id: id_tampered,
                    bytes,
                },
            )
            .unwrap();
        let mut again = reopen(&a);
        assert_eq!(again.note(id), Some(before));
        let issues = again.take_open_issues();
        assert_eq!(issues.integrity, BTreeSet::from([id]));
        assert!(!issues.upgrade_required);
    }

    #[test]
    fn edit_survives_a_restart_and_stays_queued() {
        let (mut a, _) = pair();
        let id = a.create_note("draft", T1).unwrap();
        a.set_body(id, "draft, edited offline", T2).unwrap();
        // The process dies here; only storage survives.
        let again = reopen(&a);
        assert_eq!(again.note(id).unwrap().body(), "draft, edited offline");
        assert_eq!(again.pending().unwrap(), 2);
    }

    #[test]
    fn storage_failure_rolls_back_the_edit() {
        let [key] = vault_keys(vault());
        let store = Failing {
            inner: MemoryStorage::new(vault()),
            fail: false,
        };
        let mut r = Replica::open(store, key).unwrap();
        let id = r.create_note("before", T1).unwrap();
        let note = r.note(id).unwrap();
        let outbox = r.storage().unwrap().outbox().unwrap();

        r.storage_mut().unwrap().fail = true;
        let err = r.set_body(id, "after", T2).unwrap_err();
        assert!(matches!(
            err,
            ReplicaError::Storage(StorageError::Backend(_))
        ));
        assert_eq!(r.note(id), Some(note));
        assert_eq!(r.storage().unwrap().outbox().unwrap(), outbox);

        // The rebuilt document still takes edits.
        r.set_body(id, "after", T2).unwrap();
        assert_eq!(r.note(id).unwrap().body(), "after");
        assert_eq!(r.pending().unwrap(), 2);
    }

    #[test]
    fn new_note_key_is_pending_and_initial_update_queued() {
        let (mut a, _) = pair();
        let id = a.create_note("new", T1).unwrap();
        let store = a.storage().unwrap();
        let pending = store.pending_keys().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, id);
        assert_eq!(store.outbox().unwrap().len(), 1);
    }

    #[test]
    fn unchanged_edit_stores_nothing() {
        let (mut a, _) = pair();
        let id = a.create_note("same", T1).unwrap();
        a.set_body(id, "same", T2).unwrap();
        a.set_pinned(id, false).unwrap();
        assert_eq!(a.pending().unwrap(), 1);
        let missing = NoteId::from_parts(9, 9);
        assert_eq!(
            a.set_body(missing, "x", T2),
            Err(ReplicaError::NoteNotFound(missing))
        );
    }

    #[test]
    fn remote_note_arrives_and_is_reported() {
        let (mut a, mut b) = pair();
        let id = a.create_note("from a", T1).unwrap();
        let applied = b.apply_page(&page_of_outbox(&a, 1)).unwrap();
        assert_eq!(applied.changed, BTreeSet::from([id]));
        assert_eq!(applied.issues, Issues::default());
        assert_eq!(b.note(id), a.note(id));
        assert_eq!(b.storage().unwrap().cursor().unwrap(), ServerSeq(1));
        // The key came from the server, so it isn't pending on b.
        assert!(b.storage().unwrap().pending_keys().unwrap().is_empty());
    }

    #[test]
    fn echo_of_own_push_is_not_a_change() {
        let (mut a, _) = pair();
        a.create_note("mine", T1).unwrap();
        let page = page_of_outbox(&a, 1);
        let applied = a.apply_page(&page).unwrap();
        assert!(applied.changed.is_empty());
        // Storage recognised the echo and acknowledged it.
        assert_eq!(a.pending().unwrap(), 0);
        assert_eq!(a.storage().unwrap().cursor().unwrap(), ServerSeq(1));
    }

    #[test]
    fn tampered_update_is_skipped_and_others_apply() {
        let (mut a, mut b) = pair();
        let one = a.create_note("one", T1).unwrap();
        let two = a.create_note("two", T1).unwrap();
        let mut page = page_of_outbox(&a, 1);
        if let Entry::Update { data, .. } = &mut page.entries[0] {
            data.0[40] ^= 1;
        }
        let applied = b.apply_page(&page).unwrap();
        assert_eq!(applied.changed, BTreeSet::from([two]));
        assert_eq!(applied.issues.integrity, BTreeSet::from([one]));
        assert_eq!(b.note(two), a.note(two));
        assert_eq!(b.note(one), None);
        assert_eq!(b.storage().unwrap().cursor().unwrap(), ServerSeq(2));
    }

    #[test]
    fn unknown_format_is_stored_but_not_applied() {
        let (mut a, mut b) = pair();
        let id = a.create_note("v1", T1).unwrap();
        b.apply_page(&page_of_outbox(&a, 1)).unwrap();
        let before = b.note(id);

        // A newer client's update, in format 2.
        let [key] = vault_keys(vault());
        let wrapped = a
            .storage()
            .unwrap()
            .load_note(id)
            .unwrap()
            .unwrap()
            .wrapped_key;
        let note_key = key.unwrap_note_key(vault(), id, &wrapped).unwrap();
        let data = note_key
            .encrypt(vault(), id, PayloadKind::CrdtUpdate, &[2, 0, 0])
            .unwrap();
        let page = ChangesResponse {
            notes: vec![],
            entries: vec![Entry::Update {
                seq: 2,
                id: OpId::from_parts(5, 5),
                note: id,
                data: Bytes(data),
                note_key: None,
            }],
            cursor: 2,
            more: false,
        };
        let applied = b.apply_page(&page).unwrap();
        assert!(applied.issues.upgrade_required);
        assert!(applied.changed.is_empty());
        assert_eq!(b.note(id), before);
        // Stored for a newer client, and never compacted away.
        let stored = b.storage().unwrap().load_note(id).unwrap().unwrap();
        assert_eq!(stored.entries.len(), 2);
        assert_eq!(b.snapshot_candidate(0, &BTreeSet::new()), None);
        assert!(reopen(&b).take_open_issues().upgrade_required);
    }

    #[test]
    fn compaction_catch_up_keeps_unpushed_edit() {
        let (mut a, mut b) = pair();
        let id = a.create_note("base", T1).unwrap();
        let page = page_of_outbox(&a, 1);
        a.apply_page(&page).unwrap();
        b.apply_page(&page).unwrap();

        // a edits offline; b edits and snapshots, and the server compacts.
        a.set_body(id, "base, a", T2).unwrap();
        b.set_body(id, "b, base", T2).unwrap();
        let snapshot = b.notes[&id].doc.snapshot().unwrap();
        let page = ChangesResponse {
            notes: vec![NoteEntry {
                note: id,
                key: None,
                snapshot: Some(Snapshot {
                    covers: 5,
                    data: Bytes(snapshot.into_bytes()),
                }),
            }],
            entries: vec![],
            cursor: 5,
            more: false,
        };
        let applied = a.apply_page(&page).unwrap();
        assert_eq!(applied.changed, BTreeSet::from([id]));
        assert_eq!(a.note(id).unwrap().body(), "b, base, a");
        assert_eq!(a.pending().unwrap(), 1);
    }

    #[test]
    fn purge_tombstone_removes_the_note() {
        let (mut a, mut b) = pair();
        let id = a.create_note("doomed", T1).unwrap();
        b.apply_page(&page_of_outbox(&a, 1)).unwrap();
        b.set_body(id, "doomed, edited", T2).unwrap();
        let other = b.create_note("other", T2).unwrap();
        let page = ChangesResponse {
            notes: vec![],
            entries: vec![Entry::Purge { seq: 3, note: id }],
            cursor: 3,
            more: false,
        };
        let applied = b.apply_page(&page).unwrap();
        assert_eq!(applied.changed, BTreeSet::from([id]));
        assert_eq!(b.note(id), None);
        let store = b.storage().unwrap();
        assert_eq!(store.load_note(id).unwrap(), None);
        // The purged note's queued edit is gone; the other note's isn't.
        let outbox = store.outbox().unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].note, other);
    }

    #[test]
    fn purge_in_the_same_page_wins_over_earlier_updates() {
        let (mut a, mut b) = pair();
        let id = a.create_note("short-lived", T1).unwrap();
        let mut page = page_of_outbox(&a, 1);
        page.entries.push(Entry::Purge { seq: 2, note: id });
        page.cursor = 2;
        let applied = b.apply_page(&page).unwrap();
        assert!(applied.changed.is_empty());
        assert_eq!(b.note(id), None);
        assert_eq!(b.storage().unwrap().load_note(id).unwrap(), None);
    }

    #[test]
    fn local_compaction_keeps_state_and_newer_entries() {
        let (mut a, _) = pair();
        let id = a.create_note("one", T1).unwrap();
        // Acknowledge the creation so compaction may drop it.
        a.apply_page(&page_of_outbox(&a, 1)).unwrap();
        a.set_body(id, "one two", T2).unwrap();
        let prepared = a.prepare_snapshot(id).unwrap().unwrap();
        // Stored after the snapshot was taken.
        a.set_body(id, "one two three", T2).unwrap();
        let note = a.note(id);
        a.compact_note(id, &prepared.snapshot, prepared.through)
            .unwrap();

        let stored = a.storage().unwrap().load_note(id).unwrap().unwrap();
        assert!(stored.base.is_some());
        // The queued edit and the newer entry survive; the acknowledged
        // creation is folded into the base.
        assert_eq!(stored.entries.len(), 2);
        assert_eq!(a.pending().unwrap(), 2);
        assert_eq!(a.note(id), note);
        assert_eq!(reopen(&a).note(id), note);
    }

    #[test]
    fn snapshot_candidate_follows_threshold_and_deferral() {
        let (mut a, _) = pair();
        let id = a.create_note("0", T1).unwrap();
        a.apply_page(&page_of_outbox(&a, 1)).unwrap();
        let none = BTreeSet::new();
        assert_eq!(a.snapshot_candidate(1, &none), None);
        a.set_body(id, "1", T2).unwrap();
        assert_eq!(a.snapshot_candidate(1, &none), Some(id));
        assert_eq!(a.snapshot_candidate(1, &BTreeSet::from([id])), None);
        a.defer_snapshot(id, 1);
        assert_eq!(a.snapshot_candidate(1, &none), None);
        // Deferred past the threshold's worth of new entries.
        a.set_body(id, "2", T2).unwrap();
        assert_eq!(a.snapshot_candidate(1, &none), None);
        a.set_body(id, "3", T2).unwrap();
        assert_eq!(a.snapshot_candidate(1, &none), Some(id));
        // Compaction keeps the three queued edits and resets the trigger.
        let prepared = a.prepare_snapshot(id).unwrap().unwrap();
        a.compact_note(id, &prepared.snapshot, prepared.through)
            .unwrap();
        assert_eq!(a.notes[&id].entries, 3);
        assert_eq!(a.notes[&id].trigger, None);
    }

    #[test]
    fn failed_rebuild_keeps_notes_in_memory_for_a_retry() {
        let (mut a, _) = pair();
        let id = a.create_note("keep me", T1).unwrap();
        let err = a.rebuild_storage(&mut || Err(StorageError::Backend("disk".into())));
        assert!(err.is_err());
        // The store is gone; an edit fails, but the note isn't dropped.
        assert!(a.set_body(id, "keep me, edited", T2).is_err());
        assert!(a.note(id).is_some());
        // A retry rebuilds from memory.
        a.rebuild_storage(&mut || Ok(MemoryStorage::new(vault())))
            .unwrap();
        assert_eq!(reopen(&a).notes(), a.notes());
    }

    #[test]
    fn key_and_snapshot_on_separate_pages_still_open_the_note() {
        let (mut a, mut b) = pair();
        let id = a.create_note("compacted note", T1).unwrap();
        let page = page_of_outbox(&a, 1);
        let key = page.notes[0].key.clone();
        let snapshot = a.notes[&id].doc.snapshot().unwrap().into_bytes();
        // Page 1: just the key (its update was compacted away).
        let first = ChangesResponse {
            notes: vec![NoteEntry {
                note: id,
                key,
                snapshot: None,
            }],
            entries: vec![],
            cursor: 1,
            more: true,
        };
        // Page 2: the snapshot, without the key.
        let second = ChangesResponse {
            notes: vec![NoteEntry {
                note: id,
                key: None,
                snapshot: Some(Snapshot {
                    covers: 50,
                    data: Bytes(snapshot),
                }),
            }],
            entries: vec![],
            cursor: 50,
            more: false,
        };
        b.apply_page(&first).unwrap();
        let applied = b.apply_page(&second).unwrap();
        assert_eq!(applied.changed, BTreeSet::from([id]));
        assert_eq!(b.note(id).unwrap().body(), "compacted note");
    }

    #[test]
    fn rebuild_storage_requeues_every_note() {
        let (mut a, _) = pair();
        let one = a.create_note("one", T1).unwrap();
        a.apply_page(&page_of_outbox(&a, 1)).unwrap();
        a.storage_mut().unwrap().mark_key_uploaded(one).unwrap();
        a.set_body(one, "one, unpushed", T2).unwrap();
        let notes = a.notes();

        a.rebuild_storage(&mut || Ok(MemoryStorage::new(vault())))
            .unwrap();
        let store = a.storage().unwrap();
        assert_eq!(store.cursor().unwrap(), ServerSeq(0));
        assert_eq!(store.pending_keys().unwrap().len(), 1);
        assert_eq!(store.outbox().unwrap().len(), 1);
        assert_eq!(a.notes(), notes);
        assert_eq!(reopen(&a).notes(), notes);
    }
}
