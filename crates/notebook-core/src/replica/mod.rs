//! The unlocked vault in memory: every note's document, kept in step with
//! storage.

use std::collections::BTreeMap;
use std::fmt;

use zeroize::Zeroizing;

use crate::backup::BackupNote;
use crate::crypto::{CryptoError, NoteKey, VaultId, VaultKey, fill_random};
use crate::document::{DocumentError, EditorSync, EditorUpdate, EncryptedUpdate, NoteDocument};
use crate::note::{Note, NoteId, Timestamp};
use crate::storage::{EntryKind, LocalUpdate, Storage, StorageError, StoredNote};

/// Compact a note once its log holds more entries than this.
const SNAPSHOT_THRESHOLD: usize = 200;

/// Why a replica operation failed. On error nothing changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaError {
    Storage(StorageError),
    Document(DocumentError),
    Crypto(CryptoError),
    NoteNotFound(NoteId),
    /// The editor session was never attached, was detached, or its note
    /// was deleted.
    EditorSessionNotFound(EditorSessionId),
}

impl fmt::Display for ReplicaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(err) => err.fmt(f),
            Self::Document(err) => err.fmt(f),
            Self::Crypto(err) => err.fmt(f),
            Self::NoteNotFound(id) => write!(f, "note {id} not found"),
            Self::EditorSessionNotFound(_) => f.write_str("editor session not found"),
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

/// One editor attached to a note, from [`Replica::editor_attach`]. IDs are
/// never reused within a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EditorSessionId(u64);

/// An attached editor: its note and what it has seen.
struct EditorSession {
    note: NoteId,
    sync: EditorSync,
}

/// One open note.
struct Open {
    doc: NoteDocument,
    wrapped_key: Vec<u8>,
    /// Log entries since the base, counted at open and on every append.
    entries: usize,
    /// Compact once `entries` exceeds this; `None` uses the threshold. Set
    /// after a failed compaction so the next attempt waits a while.
    trigger: Option<usize>,
    /// Holds an entry in a format this build can't apply. Compacting would
    /// drop it, so the note is never compacted.
    unapplied: bool,
}

/// The unlocked vault: storage plus every note's open document.
///
/// Edits go through here so each one is durable before it returns. The
/// host shares the replica through [`Shared`] and reads notes from it
/// directly.
pub struct Replica<S: Storage> {
    vault: VaultId,
    vault_key: VaultKey,
    storage: S,
    notes: BTreeMap<NoteId, Open>,
    /// Kept outside [`Open`] so they survive a document being rebuilt from
    /// storage.
    editors: BTreeMap<EditorSessionId, EditorSession>,
    next_editor: u64,
}

impl<S: Storage> Replica<S> {
    /// Opens every stored note: unwraps its key, loads its base, and replays
    /// its log. Data that fails decryption or decoding is skipped, and data
    /// in an unknown format is left unapplied. A note whose key won't unwrap
    /// stays in storage but isn't shown.
    pub fn open(storage: S, vault_key: VaultKey) -> Result<Self, StorageError> {
        let vault = storage.vault_id();
        let mut notes = BTreeMap::new();
        for id in storage.list_notes()? {
            if let Some(stored) = storage.load_note(id)?
                && let Some(open) = build(vault, &vault_key, id, &stored)
            {
                notes.insert(id, open);
            }
        }
        Ok(Self {
            vault,
            vault_key,
            storage,
            notes,
            editors: BTreeMap::new(),
            next_editor: 0,
        })
    }

    pub fn vault_id(&self) -> VaultId {
        self.vault
    }

    /// Every readable note, in ID order.
    pub fn notes(&self) -> Vec<Note> {
        self.notes.values().filter_map(|n| n.doc.note()).collect()
    }

    pub fn note(&self, id: NoteId) -> Option<Note> {
        self.notes.get(&id).and_then(|n| n.doc.note())
    }

    /// Creates a note with a fresh key, stored before this returns.
    pub fn create_note(&mut self, body: &str, now: Timestamp) -> Result<NoteId, ReplicaError> {
        let id = NoteId::from_parts(now.as_millis().max(0) as u64, random_u128()?);
        let key = NoteKey::generate()?;
        let wrapped_key = self.vault_key.wrap_note_key(self.vault, id, &key)?;
        let (doc, update) = NoteDocument::create(self.vault, id, key, body, now)?;
        self.storage
            .create_note(id, &wrapped_key, local_update(update))?;
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
        Ok(id)
    }

    /// Sets note `id`'s body. Does nothing if it's unchanged.
    pub fn set_body(&mut self, id: NoteId, body: &str, now: Timestamp) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_body(body, now)).map(drop)
    }

    pub fn set_pinned(&mut self, id: NoteId, pinned: bool) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_pinned(pinned)).map(drop)
    }

    pub fn set_archived(&mut self, id: NoteId, archived: bool) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_archived(archived)).map(drop)
    }

    pub fn set_trashed(&mut self, id: NoteId, trashed: bool) -> Result<(), ReplicaError> {
        self.edit(id, |doc| doc.set_trashed(trashed)).map(drop)
    }

    /// Attaches an editor to note `id`. Returns its session and the note's
    /// full state for the editor's replica.
    pub fn editor_attach(
        &mut self,
        id: NoteId,
    ) -> Result<(EditorSessionId, EditorUpdate), ReplicaError> {
        let open = self.notes.get(&id).ok_or(ReplicaError::NoteNotFound(id))?;
        let (state, sync) = open.doc.editor_attach();
        let session = EditorSessionId(self.next_editor);
        self.next_editor += 1;
        self.editors
            .insert(session, EditorSession { note: id, sync });
        Ok((session, state))
    }

    /// Applies an update made in `session`'s editor as a local edit, stored
    /// like [`set_body`](Self::set_body). Returns whether it changed
    /// anything. On a storage failure the note is rebuilt from storage and
    /// the call fails; the editor resends, which is harmless if part of it
    /// was already applied.
    pub fn apply_editor_update(
        &mut self,
        session: EditorSessionId,
        update: &EditorUpdate,
        now: Timestamp,
    ) -> Result<bool, ReplicaError> {
        let editor = self
            .editors
            .get(&session)
            .ok_or(ReplicaError::EditorSessionNotFound(session))?;
        let (note, mut sync) = (editor.note, editor.sync.clone());
        let result = self.edit(note, |doc| doc.apply_editor_update(&mut sync, update, now));
        if let Some(editor) = self.editors.get_mut(&session) {
            editor.sync = sync;
        }
        result
    }

    /// Everything `session`'s editor lacks (core-side edits such as a tag
    /// rename, updated-time bumps), or `None` if it has it all. Hosts call
    /// this whenever the note changes.
    pub fn editor_pending(
        &mut self,
        session: EditorSessionId,
    ) -> Result<Option<EditorUpdate>, ReplicaError> {
        let editor = self
            .editors
            .get_mut(&session)
            .ok_or(ReplicaError::EditorSessionNotFound(session))?;
        let open = self
            .notes
            .get(&editor.note)
            .ok_or(ReplicaError::NoteNotFound(editor.note))?;
        Ok(open.doc.editor_pending(&mut editor.sync))
    }

    /// Detaches `session`'s editor. Unknown sessions are ignored.
    pub fn editor_detach(&mut self, session: EditorSessionId) {
        self.editors.remove(&session);
    }

    /// Writes the device-local setting `key` (see
    /// [`Storage::set_setting`]). Settings are stored unencrypted.
    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<(), StorageError> {
        self.storage.set_setting(key, value)
    }

    /// A setting written by [`set_sealed_setting`](Self::set_sealed_setting).
    /// One that doesn't decrypt (another vault's, or damaged) reads as unset.
    pub fn sealed_setting(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, StorageError> {
        use base64::Engine;
        let Some(stored) = self.storage.setting(key)? else {
            return Ok(None);
        };
        Ok(base64::engine::general_purpose::STANDARD
            .decode(stored)
            .ok()
            .and_then(|sealed| self.vault_key.open_setting(self.vault, &sealed).ok()))
    }

    /// Writes a device-local setting encrypted under the vault key, for
    /// values that would reveal vault content, such as tag names.
    pub fn set_sealed_setting(&mut self, key: &str, value: &[u8]) -> Result<(), ReplicaError> {
        use base64::Engine;
        let sealed = self.vault_key.seal_setting(self.vault, value)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(sealed);
        Ok(self.storage.set_setting(key, &encoded)?)
    }

    /// Every readable note as a backup holds it: its wrapped key and a
    /// snapshot of its current state, in ID order. Notes with no readable
    /// state (see [`notes`](Self::notes)) are left out, since a backup
    /// can only restore notes that load.
    pub fn backup_notes(&self) -> Result<Vec<BackupNote>, ReplicaError> {
        self.notes
            .iter()
            .filter(|(_, open)| open.doc.note().is_some())
            .map(|(id, open)| {
                Ok(BackupNote {
                    id: *id,
                    wrapped_key: open.wrapped_key.clone(),
                    snapshot: open.doc.snapshot()?,
                })
            })
            .collect()
    }

    /// Read access to the underlying store (for export and diagnostics).
    /// Writes go through the replica so memory and storage stay in step.
    pub fn storage(&self) -> Result<&S, StorageError> {
        Ok(&self.storage)
    }

    /// Deletes note `id` from storage and memory, detaching its editors.
    /// Returns whether it was open.
    pub fn delete_note(&mut self, id: NoteId) -> Result<bool, StorageError> {
        self.storage.delete_note(id)?;
        self.editors.retain(|_, editor| editor.note != id);
        Ok(self.notes.remove(&id).is_some())
    }

    /// Applies a local change, then stores it. Returns whether there was
    /// anything to store. If storing fails, the document is rebuilt from
    /// storage, since a document can't undo. A malformed change is rejected
    /// before it touches the document, so that needs no rebuild.
    fn edit(
        &mut self,
        id: NoteId,
        change: impl FnOnce(&mut NoteDocument) -> Result<Option<EncryptedUpdate>, DocumentError>,
    ) -> Result<bool, ReplicaError> {
        let open = self
            .notes
            .get_mut(&id)
            .ok_or(ReplicaError::NoteNotFound(id))?;
        let stored = match change(&mut open.doc) {
            Ok(None) => return Ok(false),
            Err(DocumentError::Malformed) => return Err(DocumentError::Malformed.into()),
            Ok(Some(update)) => self
                .storage
                .append_local(id, local_update(update))
                .map_err(ReplicaError::from),
            Err(err) => Err(err.into()),
        };
        match stored {
            Ok(()) => {
                if let Some(open) = self.notes.get_mut(&id) {
                    open.entries += 1;
                }
                self.compact_if_due(id);
                Ok(true)
            }
            Err(err) => {
                self.reload(id);
                Err(err)
            }
        }
    }

    /// Folds note `id`'s log into a new base snapshot once the log is long.
    /// The edit that triggered it is already stored, so a failure here is
    /// not an error: the log just stays long, and the next attempt waits
    /// for another threshold's worth of entries.
    ///
    /// Everything stored for the note is applied to the document first, so
    /// the snapshot covers every entry it replaces, including any another
    /// process wrote to the same store since this one loaded the note.
    fn compact_if_due(&mut self, id: NoteId) {
        let Some(open) = self.notes.get(&id) else {
            return;
        };
        if open.unapplied || open.entries <= open.trigger.unwrap_or(SNAPSHOT_THRESHOLD) {
            return;
        }
        let stored = self
            .storage
            .load_note(id)
            .and_then(|stored| stored.ok_or(StorageError::NoteNotFound(id)));
        let Some(open) = self.notes.get_mut(&id) else {
            return;
        };
        let compacted = stored.map_err(ReplicaError::from).and_then(|stored| {
            let through = stored
                .entries
                .last()
                .map(|e| e.pos)
                .ok_or(ReplicaError::NoteNotFound(id))?;
            let base = stored.base.iter().map(|b| (EntryKind::Snapshot, b));
            let log = stored.entries.iter().map(|e| (e.kind, &e.bytes));
            for (kind, bytes) in base.chain(log) {
                if let Err(err) = apply_entry(&mut open.doc, kind, bytes) {
                    open.unapplied |= needs_newer_build(&err);
                }
            }
            // Another process stored something this build can't apply;
            // compacting would drop it, so the note is never compacted.
            if open.unapplied {
                return Ok(false);
            }
            let snapshot = open.doc.snapshot()?;
            self.storage.compact(id, snapshot.as_bytes(), through)?;
            Ok(true)
        });
        match compacted {
            Ok(true) => {
                open.entries = 0;
                open.trigger = None;
            }
            Ok(false) => {}
            Err(_) => open.trigger = Some(open.entries + SNAPSHOT_THRESHOLD),
        }
    }

    /// Rebuilds note `id` from storage. If storage can't produce it, the
    /// document in memory is kept: losing it is worse than a document that
    /// is one failed edit ahead of storage.
    fn reload(&mut self, id: NoteId) {
        let stored = self.storage.load_note(id).ok().flatten();
        if let Some(open) =
            stored.and_then(|stored| build(self.vault, &self.vault_key, id, &stored))
        {
            self.notes.insert(id, open);
        }
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

/// Opens a note's document from its stored data, or `None` if its key won't
/// unwrap.
fn build(vault: VaultId, vault_key: &VaultKey, id: NoteId, stored: &StoredNote) -> Option<Open> {
    let key = vault_key
        .unwrap_note_key(vault, id, &stored.wrapped_key)
        .ok()?;
    let mut doc = NoteDocument::open(vault, id, key).ok()?;
    let mut unapplied = false;
    let base = stored.base.iter().map(|b| (EntryKind::Snapshot, b));
    let log = stored.entries.iter().map(|e| (e.kind, &e.bytes));
    for (kind, bytes) in base.chain(log) {
        if let Err(err) = apply_entry(&mut doc, kind, bytes) {
            unapplied |= needs_newer_build(&err);
        }
    }
    Some(Open {
        doc,
        wrapped_key: stored.wrapped_key.clone(),
        entries: stored.entries.len(),
        trigger: None,
        unapplied,
    })
}

/// Whether `err` means the data was written by a newer build (an unknown
/// document format, envelope version, or algorithm) rather than damaged.
/// Such data must be kept for a build that can read it.
fn needs_newer_build(err: &DocumentError) -> bool {
    matches!(
        err,
        DocumentError::UnsupportedFormat(_)
            | DocumentError::Crypto(
                CryptoError::UnsupportedVersion(_) | CryptoError::UnsupportedAlgorithm(_)
            )
    )
}

fn apply_entry(doc: &mut NoteDocument, kind: EntryKind, bytes: &[u8]) -> Result<(), DocumentError> {
    match kind {
        EntryKind::Update => doc.apply_update(bytes),
        EntryKind::Snapshot => doc.load_snapshot(bytes),
    }
}

fn random_u128() -> Result<u128, CryptoError> {
    let mut bytes = [0; 16];
    fill_random(&mut bytes)?;
    Ok(u128::from_le_bytes(bytes))
}

fn local_update(update: EncryptedUpdate) -> LocalUpdate {
    LocalUpdate {
        bytes: update.into_bytes(),
    }
}

/// A value shared by the host and the workspace: `Arc<Mutex<_>>` natively
/// and `Rc<RefCell<_>>` in the browser. Locks are only ever held inside
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

/// A value shared by the host and the workspace: `Arc<Mutex<_>>` natively
/// and `Rc<RefCell<_>>` in the browser. Locks are only ever held inside
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

/// `N` copies of one vault key per vault, for tests that open a vault more
/// than once.
#[cfg(test)]
pub(crate) fn vault_keys<const N: usize>(vault: VaultId) -> [VaultKey; N] {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static KEYS: RefCell<HashMap<VaultId, Zeroizing<[u8; 32]>>> = RefCell::new(HashMap::new());
    }
    KEYS.with(|keys| {
        let mut keys = keys.borrow_mut();
        let bytes = keys.entry(vault).or_insert_with(|| {
            VaultKey::generate()
                .expect("random source")
                .to_secret_bytes()
        });
        std::array::from_fn(|_| VaultKey::from_secret_bytes(&bytes[..]).expect("valid key"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::PayloadKind;
    use crate::storage::{LogPos, MemoryStorage};

    const T1: Timestamp = Timestamp(1_700_000_000_000);
    const T2: Timestamp = Timestamp(1_700_000_001_000);

    fn vault() -> VaultId {
        VaultId::from_parts(1, 1)
    }

    fn replica() -> Replica<MemoryStorage> {
        let [key] = vault_keys(vault());
        Replica::open(MemoryStorage::new(vault()), key).unwrap()
    }

    /// A new replica on a copy of `r`'s store, as after a restart.
    fn reopen(r: &Replica<MemoryStorage>) -> Replica<MemoryStorage> {
        let [key] = vault_keys(vault());
        Replica::open(r.storage().unwrap().clone(), key).unwrap()
    }

    fn stored(r: &Replica<impl Storage>, id: NoteId) -> StoredNote {
        r.storage().unwrap().load_note(id).unwrap().unwrap()
    }

    /// Backup notes import into an empty store and reopen as the same notes.
    #[test]
    fn backup_notes_import_as_the_same_notes() {
        let mut a = replica();
        let first = a.create_note("first", T1).unwrap();
        let second = a.create_note("second", T1).unwrap();
        a.set_body(second, "second, edited", T2).unwrap();
        a.set_trashed(first, true).unwrap();

        let notes = a.backup_notes().unwrap();
        assert_eq!(
            notes.iter().map(|n| n.id).collect::<Vec<_>>(),
            [first.min(second), first.max(second)]
        );
        let mut store = MemoryStorage::new(vault());
        for note in &notes {
            store
                .import_note(note.id, &note.wrapped_key, note.snapshot.as_bytes())
                .unwrap();
        }
        let [key] = vault_keys(vault());
        let restored = Replica::open(store, key).unwrap();
        assert_eq!(restored.notes(), a.notes());
        assert!(restored.note(first).unwrap().is_trashed());
    }

    #[test]
    fn settings_write_through_to_storage() {
        let mut a = replica();
        assert_eq!(
            a.storage().unwrap().setting("editor.vimMode").unwrap(),
            None
        );
        a.set_setting("editor.vimMode", "true").unwrap();
        a.set_setting("editor.vimMode", "false").unwrap();
        assert_eq!(
            a.storage().unwrap().setting("editor.vimMode").unwrap(),
            Some("false".into())
        );
        let reopened = reopen(&a);
        assert_eq!(
            reopened
                .storage()
                .unwrap()
                .setting("editor.vimMode")
                .unwrap(),
            Some("false".into())
        );
        // Settings aren't note data.
        assert!(reopened.notes().is_empty());
    }

    #[test]
    fn sealed_settings_are_encrypted_at_rest() {
        let mut a = replica();
        let mut b = replica();
        a.set_sealed_setting("sidebar.collapsedTags", b"[\"secret plans\"]")
            .unwrap();
        let raw = a
            .storage()
            .unwrap()
            .setting("sidebar.collapsedTags")
            .unwrap()
            .unwrap();
        assert!(!raw.contains("secret"));
        assert_eq!(
            a.sealed_setting("sidebar.collapsedTags")
                .unwrap()
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"[\"secret plans\"]"[..])
        );
        assert!(
            reopen(&a)
                .sealed_setting("sidebar.collapsedTags")
                .unwrap()
                .is_some()
        );
        // Another key can't read it; it reads as unset.
        b.set_setting("sidebar.collapsedTags", &raw).unwrap();
        let other =
            Replica::open(b.storage().unwrap().clone(), VaultKey::generate().unwrap()).unwrap();
        assert!(
            other
                .sealed_setting("sidebar.collapsedTags")
                .unwrap()
                .is_none()
        );
        assert!(a.sealed_setting("missing").unwrap().is_none());
    }

    /// A store whose next local append fails when `fail` is set, and whose
    /// compactions fail while `fail_compact` is set, counted in
    /// `compactions`.
    struct Failing {
        inner: MemoryStorage,
        fail: bool,
        fail_compact: bool,
        compactions: usize,
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
        fn import_note(
            &mut self,
            id: NoteId,
            wrapped_key: &[u8],
            snapshot: &[u8],
        ) -> Result<(), StorageError> {
            self.inner.import_note(id, wrapped_key, snapshot)
        }
        fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<(), StorageError> {
            if std::mem::take(&mut self.fail) {
                return Err(StorageError::Backend("disk full".into()));
            }
            self.inner.append_local(id, update)
        }
        fn compact(
            &mut self,
            id: NoteId,
            snapshot: &[u8],
            through: LogPos,
        ) -> Result<(), StorageError> {
            self.compactions += 1;
            if self.fail_compact {
                return Err(StorageError::Backend("disk full".into()));
            }
            self.inner.compact(id, snapshot, through)
        }
        fn delete_note(&mut self, id: NoteId) -> Result<(), StorageError> {
            self.inner.delete_note(id)
        }
        fn setting(&self, key: &str) -> Result<Option<String>, StorageError> {
            self.inner.setting(key)
        }
        fn set_setting(&mut self, key: &str, value: &str) -> Result<(), StorageError> {
            self.inner.set_setting(key, value)
        }
    }

    fn failing() -> Replica<Failing> {
        let [key] = vault_keys(vault());
        let store = Failing {
            inner: MemoryStorage::new(vault()),
            fail: false,
            fail_compact: false,
            compactions: 0,
        };
        Replica::open(store, key).unwrap()
    }

    #[test]
    fn reopened_replica_has_the_same_notes() {
        let mut a = replica();
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
        let mut a = replica();
        let id = a.create_note("one", T1).unwrap();
        a.set_body(id, "one two", T2).unwrap();
        let before = a.note(id).unwrap();
        let mut bytes = stored(&a, id).entries[1].bytes.clone();
        *bytes.last_mut().unwrap() ^= 1;
        a.storage.append_local(id, LocalUpdate { bytes }).unwrap();
        assert_eq!(reopen(&a).note(id), Some(before));
    }

    #[test]
    fn edit_survives_a_restart() {
        let mut a = replica();
        let id = a.create_note("draft", T1).unwrap();
        a.set_body(id, "draft, edited", T2).unwrap();
        // The process dies here; only storage survives.
        let again = reopen(&a);
        assert_eq!(again.note(id).unwrap().body(), "draft, edited");
        assert_eq!(stored(&again, id).entries.len(), 2);
    }

    #[test]
    fn storage_failure_rolls_back_the_edit() {
        let mut r = failing();
        let id = r.create_note("before", T1).unwrap();
        let note = r.note(id).unwrap();
        let log = stored(&r, id);

        r.storage.fail = true;
        let err = r.set_body(id, "after", T2).unwrap_err();
        assert!(matches!(
            err,
            ReplicaError::Storage(StorageError::Backend(_))
        ));
        assert_eq!(r.note(id), Some(note));
        assert_eq!(stored(&r, id), log);

        // The rebuilt document still takes edits.
        r.set_body(id, "after", T2).unwrap();
        assert_eq!(r.note(id).unwrap().body(), "after");
        assert_eq!(stored(&r, id).entries.len(), 2);
    }

    #[test]
    fn new_note_stores_its_key_and_initial_update() {
        let mut a = replica();
        let id = a.create_note("new", T1).unwrap();
        let stored = stored(&a, id);
        assert_eq!(stored.base, None);
        assert_eq!(stored.entries.len(), 1);
        let [key] = vault_keys(vault());
        assert!(
            key.unwrap_note_key(vault(), id, &stored.wrapped_key)
                .is_ok()
        );
    }

    #[test]
    fn unchanged_edit_stores_nothing() {
        let mut a = replica();
        let id = a.create_note("same", T1).unwrap();
        a.set_body(id, "same", T2).unwrap();
        a.set_pinned(id, false).unwrap();
        assert_eq!(stored(&a, id).entries.len(), 1);
        let missing = NoteId::from_parts(9, 9);
        assert_eq!(
            a.set_body(missing, "x", T2),
            Err(ReplicaError::NoteNotFound(missing))
        );
    }

    #[test]
    fn unknown_format_is_kept_and_never_compacted() {
        let mut a = replica();
        let id = a.create_note("v1", T1).unwrap();
        let before = a.note(id);

        // A newer client's update, in format 2.
        let [key] = vault_keys(vault());
        let note_key = key
            .unwrap_note_key(vault(), id, &stored(&a, id).wrapped_key)
            .unwrap();
        let bytes = note_key
            .encrypt(vault(), id, PayloadKind::CrdtUpdate, &[2, 0, 0])
            .unwrap();
        a.storage.append_local(id, LocalUpdate { bytes }).unwrap();

        let mut b = reopen(&a);
        assert_eq!(b.note(id), before);
        for i in 0..=SNAPSHOT_THRESHOLD {
            b.set_body(id, &format!("v1 {i}"), T2).unwrap();
        }
        let after = stored(&b, id);
        assert_eq!(after.base, None);
        assert_eq!(after.entries.len(), SNAPSHOT_THRESHOLD + 3);
    }

    #[test]
    fn long_log_is_compacted_and_reloads_the_same() {
        let mut a = replica();
        let id = a.create_note("0", T1).unwrap();
        let other = a.create_note("other", T1).unwrap();
        // The creation plus this many edits is exactly the threshold.
        for i in 1..SNAPSHOT_THRESHOLD {
            a.set_body(id, &i.to_string(), T2).unwrap();
        }
        assert_eq!(stored(&a, id).base, None);
        assert_eq!(stored(&a, id).entries.len(), SNAPSHOT_THRESHOLD);

        a.set_body(id, "past the threshold", T2).unwrap();
        let compacted = stored(&a, id);
        assert!(compacted.base.is_some());
        assert!(compacted.entries.is_empty());
        assert_eq!(a.note(id).unwrap().body(), "past the threshold");
        assert_eq!(reopen(&a).note(id), a.note(id));
        // Other notes are left alone.
        assert_eq!(stored(&a, other).entries.len(), 1);

        // Later edits go on the new base and count from it.
        a.set_body(id, "after", T2).unwrap();
        let next = stored(&a, id);
        assert_eq!(next.base, compacted.base);
        assert_eq!(next.entries.len(), 1);
        assert_eq!(reopen(&a).note(id).unwrap().body(), "after");
    }

    #[test]
    fn unknown_envelope_version_is_kept_and_never_compacted() {
        let mut a = replica();
        let id = a.create_note("v1", T1).unwrap();
        // A newer build's envelope, version 2.
        let mut bytes = stored(&a, id).entries[0].bytes.clone();
        bytes[0] = 2;
        a.storage
            .append_local(
                id,
                LocalUpdate {
                    bytes: bytes.clone(),
                },
            )
            .unwrap();

        let mut b = reopen(&a);
        for i in 0..=SNAPSHOT_THRESHOLD {
            b.set_body(id, &format!("v1 {i}"), T2).unwrap();
        }
        let after = stored(&b, id);
        assert_eq!(after.base, None);
        assert_eq!(after.entries[1].bytes, bytes);
    }

    #[test]
    fn failed_compaction_keeps_the_edit_and_waits_a_threshold() {
        let mut r = failing();
        let id = r.create_note("0", T1).unwrap();
        r.storage.fail_compact = true;
        for i in 1..=SNAPSHOT_THRESHOLD {
            r.set_body(id, &i.to_string(), T2).unwrap();
        }
        // The edit past the threshold is stored even though compacting failed.
        assert_eq!(r.storage.compactions, 1);
        assert_eq!(stored(&r, id).entries.len(), SNAPSHOT_THRESHOLD + 1);
        assert_eq!(r.note(id).unwrap().body(), SNAPSHOT_THRESHOLD.to_string());

        // No retry until another threshold's worth of entries.
        r.storage.fail_compact = false;
        for i in 0..SNAPSHOT_THRESHOLD {
            r.set_body(id, &format!("again {i}"), T2).unwrap();
        }
        assert_eq!(r.storage.compactions, 1);
        r.set_body(id, "retry", T2).unwrap();
        assert_eq!(r.storage.compactions, 2);
        assert!(stored(&r, id).entries.is_empty());
    }

    #[test]
    fn compaction_keeps_entries_this_replica_never_loaded() {
        let mut a = replica();
        let id = a.create_note("shared", T1).unwrap();
        // A second process on the same store pins the note after `a`
        // loaded it.
        let mut b = reopen(&a);
        b.set_pinned(id, true).unwrap();
        let pin = stored(&b, id).entries.last().unwrap().bytes.clone();
        a.storage
            .append_local(id, LocalUpdate { bytes: pin })
            .unwrap();
        assert!(!a.note(id).unwrap().is_pinned());

        for i in 0..SNAPSHOT_THRESHOLD {
            a.set_body(id, &i.to_string(), T2).unwrap();
        }
        assert!(stored(&a, id).entries.is_empty());
        let again = reopen(&a).note(id).unwrap();
        assert!(again.is_pinned());
        assert_eq!(again.body(), (SNAPSHOT_THRESHOLD - 1).to_string());
    }

    #[test]
    fn long_log_from_an_older_version_compacts_on_the_next_edit() {
        let mut a = replica();
        let id = a.create_note("0", T1).unwrap();
        // Written by a version that never compacted locally.
        a.notes.get_mut(&id).unwrap().trigger = Some(usize::MAX);
        for i in 1..=SNAPSHOT_THRESHOLD + 10 {
            a.set_body(id, &i.to_string(), T2).unwrap();
        }
        assert_eq!(stored(&a, id).entries.len(), SNAPSHOT_THRESHOLD + 11);

        let mut b = reopen(&a);
        b.set_body(id, "edited", T2).unwrap();
        assert!(stored(&b, id).entries.is_empty());
        assert_eq!(reopen(&b).note(id).unwrap().body(), "edited");
    }

    /// The editor's side of a session: a plain Yrs doc fed from the replica.
    fn editor(state: &EditorUpdate) -> yrs::Doc {
        use yrs::updates::decoder::Decode;
        use yrs::{Options, Transact, Update};
        let mut options = Options::with_client_id(yrs::ClientID::new(4242));
        options.offset_kind = yrs::OffsetKind::Utf16;
        let doc = yrs::Doc::with_options(options);
        doc.get_or_insert_text("body");
        let update = Update::decode_v1(state.as_bytes()).unwrap();
        doc.transact_mut().apply_update(update).unwrap();
        doc
    }

    fn type_text(doc: &yrs::Doc, text: &str) -> EditorUpdate {
        use yrs::{Text, Transact};
        let body = doc.get_or_insert_text("body");
        let mut txn = doc.transact_mut();
        let end = body.len(&txn);
        body.insert(&mut txn, end, text);
        EditorUpdate::from_bytes(txn.encode_update_v1())
    }

    fn editor_body(doc: &yrs::Doc, pending: &EditorUpdate) -> String {
        use yrs::types::GetString;
        use yrs::updates::decoder::Decode;
        use yrs::{Transact, Update};
        let update = Update::decode_v1(pending.as_bytes()).unwrap();
        doc.transact_mut().apply_update(update).unwrap();
        doc.get_or_insert_text("body").get_string(&doc.transact())
    }

    #[test]
    fn editor_update_is_stored_once() {
        let mut a = replica();
        let id = a.create_note("Hi", T1).unwrap();
        let (session, state) = a.editor_attach(id).unwrap();
        let doc = editor(&state);
        let update = type_text(&doc, "!");
        assert_eq!(a.apply_editor_update(session, &update, T2), Ok(true));
        assert_eq!(a.note(id).unwrap().body(), "Hi!");
        assert_eq!(a.note(id).unwrap().updated(), T2);
        assert_eq!(stored(&a, id).entries.len(), 2);
        assert_eq!(reopen(&a).note(id), a.note(id));

        // The same update again changes nothing and stores nothing.
        assert_eq!(a.apply_editor_update(session, &update, T2), Ok(false));
        assert_eq!(stored(&a, id).entries.len(), 2);
        // The `updated` bump comes back to the editor.
        assert!(a.editor_pending(session).unwrap().is_some());
        assert_eq!(a.editor_pending(session), Ok(None));
    }

    #[test]
    fn editor_update_after_a_storage_failure() {
        let mut r = failing();
        let id = r.create_note("Hi", T1).unwrap();
        let (session, state) = r.editor_attach(id).unwrap();
        let doc = editor(&state);
        let update = type_text(&doc, "!");

        r.storage.fail = true;
        let err = r.apply_editor_update(session, &update, T2).unwrap_err();
        assert!(matches!(err, ReplicaError::Storage(_)));
        // Memory matches storage again.
        assert_eq!(r.note(id).unwrap().body(), "Hi");
        assert_eq!(stored(&r, id).entries.len(), 1);

        // The editor resends, and it lands.
        assert_eq!(r.apply_editor_update(session, &update, T2), Ok(true));
        assert_eq!(r.note(id).unwrap().body(), "Hi!");
        assert_eq!(stored(&r, id).entries.len(), 2);
    }

    #[test]
    fn core_side_edits_reach_the_editor() {
        let mut a = replica();
        let id = a.create_note("one\ntwo", T1).unwrap();
        let (session, state) = a.editor_attach(id).unwrap();
        let doc = editor(&state);
        assert_eq!(a.editor_pending(session), Ok(None));

        a.set_body(id, "one\nTWO", T2).unwrap();
        let pending = a.editor_pending(session).unwrap().unwrap();
        assert_eq!(editor_body(&doc, &pending), "one\nTWO");
    }

    #[test]
    fn garbage_from_the_editor() {
        let mut a = replica();
        let id = a.create_note("x", T1).unwrap();
        let (session, _) = a.editor_attach(id).unwrap();
        let garbage = EditorUpdate::from_bytes(vec![0xff; 8]);
        assert_eq!(
            a.apply_editor_update(session, &garbage, T2),
            Err(ReplicaError::Document(DocumentError::Malformed))
        );
        assert_eq!(a.note(id).unwrap().body(), "x");
        assert_eq!(stored(&a, id).entries.len(), 1);
    }

    #[test]
    fn editor_sessions_that_are_gone() {
        let mut a = replica();
        let id = a.create_note("x", T1).unwrap();
        let missing = NoteId::from_parts(9, 9);
        assert_eq!(
            a.editor_attach(missing).map(drop),
            Err(ReplicaError::NoteNotFound(missing))
        );

        let (first, state) = a.editor_attach(id).unwrap();
        let (second, _) = a.editor_attach(id).unwrap();
        assert_ne!(first, second);
        let update = type_text(&editor(&state), "y");
        a.editor_detach(first);
        assert_eq!(
            a.apply_editor_update(first, &update, T2),
            Err(ReplicaError::EditorSessionNotFound(first))
        );
        assert_eq!(
            a.editor_pending(first),
            Err(ReplicaError::EditorSessionNotFound(first))
        );

        assert_eq!(a.delete_note(id), Ok(true));
        assert_eq!(a.storage().unwrap().load_note(id).unwrap(), None);
        assert_eq!(
            a.apply_editor_update(second, &update, T2),
            Err(ReplicaError::EditorSessionNotFound(second))
        );
        assert_eq!(
            a.editor_pending(second),
            Err(ReplicaError::EditorSessionNotFound(second))
        );
        // Detaching twice is harmless.
        a.editor_detach(second);
        assert_eq!(a.delete_note(id), Ok(false));
    }
}
