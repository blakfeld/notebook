//! Behavior every [`Storage`] implementation must share.
//!
//! Each function takes a fresh, empty store and panics on a violation.
//! [`run_all`] runs them all. Behavior that only exists on disk (reopening,
//! durability, vault mismatch, schema upgrades, corruption, and crash
//! atomicity) is tested by the native implementation.
//!
//! Payload bytes here are arbitrary: storage never inspects them.

use super::{EntryKind, LocalUpdate, LogPos, Storage, StorageError, StoredNote};
use crate::note::NoteId;

/// Runs every conformance check, each against a new store from `make`.
pub fn run_all<S: Storage>(mut make: impl FnMut() -> S) {
    create_note(make());
    append_local(make());
    import_note(make());
    load_note(make());
    list_notes(make());
    compaction(make());
    delete_note(make());
    settings(make());
}

fn note(n: u128) -> NoteId {
    NoteId::from_parts(1_000, n)
}

fn local(n: u8) -> LocalUpdate {
    LocalUpdate { bytes: vec![n; 4] }
}

fn load<S: Storage>(s: &S, id: NoteId) -> StoredNote {
    s.load_note(id).unwrap().expect("note is stored")
}

fn entry_bytes(stored: &StoredNote) -> Vec<Vec<u8>> {
    stored.entries.iter().map(|e| e.bytes.clone()).collect()
}

fn last_pos(stored: &StoredNote) -> LogPos {
    stored.entries.last().expect("note has entries").pos
}

/// Creating a note stores its key and initial update; a duplicate note ID
/// fails without changing anything.
pub fn create_note<S: Storage>(mut s: S) {
    s.create_note(note(1), b"key-1", local(1)).unwrap();
    let stored = load(&s, note(1));
    assert_eq!(stored.wrapped_key, b"key-1");
    assert_eq!(stored.base, None);
    assert_eq!(stored.entries.len(), 1);
    assert_eq!(stored.entries[0].kind, EntryKind::Update);
    assert_eq!(stored.entries[0].bytes, local(1).bytes);

    assert_eq!(
        s.create_note(note(1), b"other", local(2)),
        Err(StorageError::DuplicateNote(note(1)))
    );
    assert_eq!(load(&s, note(1)), stored);
    assert_eq!(s.list_notes().unwrap(), [note(1)]);
}

/// A local update goes to the end of the note's log; an unknown note fails.
pub fn append_local<S: Storage>(mut s: S) {
    s.create_note(note(1), b"key", local(1)).unwrap();
    s.append_local(note(1), local(2)).unwrap();
    let stored = load(&s, note(1));
    assert_eq!(entry_bytes(&stored), [local(1).bytes, local(2).bytes]);

    assert_eq!(
        s.append_local(note(9), local(3)),
        Err(StorageError::NoteNotFound(note(9)))
    );
    assert_eq!(load(&s, note(1)), stored);
    assert_eq!(s.list_notes().unwrap(), [note(1)]);
}

/// An imported note loads with its key and base snapshot and no entries, and
/// is listed; later local updates follow the base. A duplicate ID fails
/// without changing the existing note.
pub fn import_note<S: Storage>(mut s: S) {
    let n = note(1);
    s.import_note(n, b"key-1", b"snap-1").unwrap();
    let stored = load(&s, n);
    assert_eq!(stored.wrapped_key, b"key-1");
    assert_eq!(stored.base.as_deref(), Some(&b"snap-1"[..]));
    assert!(stored.entries.is_empty());
    assert_eq!(s.list_notes().unwrap(), [n]);

    s.create_note(note(2), b"key-2", local(1)).unwrap();
    for id in [n, note(2)] {
        let before = load(&s, id);
        assert_eq!(
            s.import_note(id, b"other", b"other"),
            Err(StorageError::DuplicateNote(id))
        );
        assert_eq!(load(&s, id), before);
    }
    assert_eq!(s.list_notes().unwrap(), [n, note(2)]);

    s.append_local(n, local(2)).unwrap();
    let stored = load(&s, n);
    assert_eq!(stored.base.as_deref(), Some(&b"snap-1"[..]));
    assert_eq!(entry_bytes(&stored), [local(2).bytes]);
}

/// Entries load in stored order with increasing positions, interleaved with
/// other notes' appends; an absent note is `None`.
pub fn load_note<S: Storage>(mut s: S) {
    let n = note(1);
    s.create_note(n, b"key", local(1)).unwrap();
    s.create_note(note(2), b"other", local(9)).unwrap();
    s.append_local(n, local(2)).unwrap();
    s.append_local(note(2), local(9)).unwrap();
    s.append_local(n, local(3)).unwrap();

    let stored = load(&s, n);
    assert_eq!(
        entry_bytes(&stored),
        [local(1).bytes, local(2).bytes, local(3).bytes]
    );
    assert!(stored.entries.iter().all(|e| e.kind == EntryKind::Update));
    assert!(stored.entries.windows(2).all(|w| w[0].pos < w[1].pos));

    assert_eq!(s.load_note(note(3)).unwrap(), None);
}

/// Every stored note is listed, created or imported; deleted ones aren't.
pub fn list_notes<S: Storage>(mut s: S) {
    assert!(s.list_notes().unwrap().is_empty());
    s.create_note(note(1), b"a", local(1)).unwrap();
    s.create_note(note(2), b"b", local(2)).unwrap();
    s.import_note(note(3), b"c", b"snap").unwrap();
    assert_eq!(s.list_notes().unwrap(), [note(1), note(2), note(3)]);
    s.delete_note(note(2)).unwrap();
    assert_eq!(s.list_notes().unwrap(), [note(1), note(3)]);
}

/// Compaction folds entries up to the cutoff into the base, keeps entries
/// after it, and leaves other notes alone.
pub fn compaction<S: Storage>(mut s: S) {
    let a = note(1);
    s.create_note(a, b"a", local(1)).unwrap();
    s.append_local(a, local(2)).unwrap();
    let b = note(2);
    s.create_note(b, b"b", local(11)).unwrap();

    // Every entry is folded in.
    let through = last_pos(&load(&s, a));
    s.compact(a, b"snap-a", through).unwrap();
    let stored = load(&s, a);
    assert_eq!(stored.base.as_deref(), Some(&b"snap-a"[..]));
    assert!(stored.entries.is_empty());
    assert_eq!(stored.wrapped_key, b"a");

    // An entry stored after the snapshot was taken is kept, and a new base
    // replaces the old one.
    s.append_local(a, local(3)).unwrap();
    let through = last_pos(&load(&s, a));
    s.append_local(a, local(4)).unwrap();
    s.compact(a, b"snap-a2", through).unwrap();
    let stored = load(&s, a);
    assert_eq!(stored.base.as_deref(), Some(&b"snap-a2"[..]));
    assert_eq!(entry_bytes(&stored), [local(4).bytes]);

    // Other notes are untouched.
    assert_eq!(load(&s, b).base, None);
    assert_eq!(entry_bytes(&load(&s, b)), [local(11).bytes]);

    assert_eq!(
        s.compact(note(9), b"x", through),
        Err(StorageError::NoteNotFound(note(9)))
    );
}

/// Deleting a note removes everything for it; deleting an absent note does
/// nothing, and the ID can be used again.
pub fn delete_note<S: Storage>(mut s: S) {
    s.create_note(note(1), b"a", local(1)).unwrap();
    s.create_note(note(2), b"b", local(2)).unwrap();
    s.append_local(note(1), local(3)).unwrap();
    s.compact(note(1), b"snap", last_pos(&load(&s, note(1))))
        .unwrap();
    s.append_local(note(1), local(4)).unwrap();
    let other = load(&s, note(2));
    s.delete_note(note(1)).unwrap();

    assert_eq!(s.list_notes().unwrap(), [note(2)]);
    assert_eq!(s.load_note(note(1)).unwrap(), None);
    assert_eq!(load(&s, note(2)), other);

    s.delete_note(note(1)).unwrap();
    s.delete_note(note(9)).unwrap();
    assert_eq!(s.list_notes().unwrap(), [note(2)]);

    s.create_note(note(1), b"new", local(5)).unwrap();
    let stored = load(&s, note(1));
    assert_eq!(stored.wrapped_key, b"new");
    assert_eq!(stored.base, None);
    assert_eq!(entry_bytes(&stored), [local(5).bytes]);
}

/// Settings round-trip and overwrite; unwritten ones are absent.
pub fn settings<S: Storage>(mut s: S) {
    assert_eq!(s.setting("editor.vim").unwrap(), None);
    s.set_setting("editor.vim", "true").unwrap();
    assert_eq!(s.setting("editor.vim").unwrap().as_deref(), Some("true"));
    s.set_setting("editor.vim", "false").unwrap();
    assert_eq!(s.setting("editor.vim").unwrap().as_deref(), Some("false"));
    assert_eq!(s.setting("editor.theme").unwrap(), None);
}
