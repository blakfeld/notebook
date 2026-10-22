//! Behavior every [`Storage`] implementation must share.
//!
//! Each function takes a fresh, empty store and panics on a violation.
//! [`run_all`] runs them all. Behavior that only exists on disk (reopening,
//! vault mismatch, schema upgrades, corruption, crash atomicity, and settings
//! surviving [`Storage::clear`]) is tested by the native implementation.
//!
//! Payload bytes here are arbitrary: storage never inspects them.

use super::{
    EntryKind, LocalUpdate, LogPos, RemoteBatch, RemoteEntry, ServerSeq, Storage, StorageError,
    StoredNote, UpdateId,
};
use crate::note::NoteId;

/// Runs every conformance check, each against a new store from `make`.
pub fn run_all<S: Storage>(mut make: impl FnMut() -> S) {
    create_note(make());
    append_local(make());
    load_note(make());
    list_notes(make());
    outbox_order(make());
    duplicate_update_id(make());
    acknowledge(make());
    cursor(make());
    remote_batch_with_new_note(make());
    remote_batch_rolls_back(make());
    remote_echoes(make());
    remote_key_kept(make());
    pending_keys(make());
    compaction(make());
    delete_note(make());
    clear(make());
    settings(make());
}

fn note(n: u128) -> NoteId {
    NoteId::from_parts(1_000, n)
}

fn uid(n: u8) -> UpdateId {
    UpdateId([n; 16])
}

fn local(n: u8) -> LocalUpdate {
    LocalUpdate {
        id: uid(n),
        bytes: vec![n; 4],
    }
}

fn remote(note: NoteId, n: u8, seq: u64) -> RemoteEntry {
    RemoteEntry {
        note,
        kind: EntryKind::Update,
        id: Some(uid(n)),
        seq: ServerSeq(seq),
        bytes: vec![n; 4],
    }
}

fn remote_snapshot(note: NoteId, n: u8, seq: u64) -> RemoteEntry {
    RemoteEntry {
        note,
        kind: EntryKind::Snapshot,
        id: None,
        seq: ServerSeq(seq),
        bytes: vec![n; 8],
    }
}

fn batch(keys: Vec<(NoteId, Vec<u8>)>, entries: Vec<RemoteEntry>, cursor: u64) -> RemoteBatch {
    RemoteBatch {
        keys,
        entries,
        cursor: ServerSeq(cursor),
    }
}

fn load<S: Storage>(s: &S, id: NoteId) -> StoredNote {
    s.load_note(id).unwrap().expect("note is stored")
}

fn entry_ids(stored: &StoredNote) -> Vec<Option<UpdateId>> {
    stored.entries.iter().map(|e| e.id).collect()
}

fn outbox_ids<S: Storage>(s: &S) -> Vec<UpdateId> {
    s.outbox().unwrap().into_iter().map(|e| e.id).collect()
}

fn pos_of<S: Storage>(s: &S, id: NoteId, update: UpdateId) -> LogPos {
    load(s, id)
        .entries
        .iter()
        .find(|e| e.id == Some(update))
        .expect("entry is stored")
        .pos
}

/// Creating a note stores its key and initial update and queues the update;
/// a duplicate note or update ID fails without changing anything.
pub fn create_note<S: Storage>(mut s: S) {
    s.create_note(note(1), b"key-1", local(1)).unwrap();
    let stored = load(&s, note(1));
    assert_eq!(stored.wrapped_key, b"key-1");
    assert_eq!(stored.base, None);
    assert_eq!(stored.entries.len(), 1);
    let entry = &stored.entries[0];
    assert_eq!(entry.id, Some(uid(1)));
    assert_eq!(entry.kind, EntryKind::Update);
    assert_eq!(entry.seq, None);
    assert_eq!(entry.bytes, local(1).bytes);
    let outbox = s.outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_eq!(
        (outbox[0].note, outbox[0].id, &outbox[0].bytes),
        (note(1), uid(1), &local(1).bytes)
    );
    assert_eq!(outbox[0].pos, entry.pos);

    assert_eq!(
        s.create_note(note(1), b"other", local(2)),
        Err(StorageError::DuplicateNote(note(1)))
    );
    assert_eq!(load(&s, note(1)), stored);
    assert_eq!(outbox_ids(&s), [uid(1)]);

    assert_eq!(
        s.create_note(note(2), b"key-2", local(1)),
        Err(StorageError::DuplicateUpdate)
    );
    assert_eq!(s.load_note(note(2)).unwrap(), None);
    assert_eq!(s.list_notes().unwrap(), [note(1)]);
    assert_eq!(outbox_ids(&s), [uid(1)]);
}

/// A local update goes to the end of the note's log and the outbox; an
/// unknown note fails.
pub fn append_local<S: Storage>(mut s: S) {
    s.create_note(note(1), b"key", local(1)).unwrap();
    s.append_local(note(1), local(2)).unwrap();
    let stored = load(&s, note(1));
    assert_eq!(entry_ids(&stored), [Some(uid(1)), Some(uid(2))]);
    assert_eq!(stored.entries[1].bytes, local(2).bytes);
    assert_eq!(outbox_ids(&s), [uid(1), uid(2)]);

    assert_eq!(
        s.append_local(note(9), local(3)),
        Err(StorageError::NoteNotFound(note(9)))
    );
    assert_eq!(outbox_ids(&s), [uid(1), uid(2)]);
}

/// Entries load in stored order with increasing positions and their kinds;
/// an absent note is `None`.
pub fn load_note<S: Storage>(mut s: S) {
    let n = note(1);
    s.apply_remote(batch(vec![(n, b"key".to_vec())], vec![remote(n, 1, 1)], 1))
        .unwrap();
    s.append_local(n, local(2)).unwrap();
    s.apply_remote(batch(vec![], vec![remote_snapshot(n, 3, 2)], 2))
        .unwrap();

    let stored = load(&s, n);
    assert_eq!(entry_ids(&stored), [Some(uid(1)), Some(uid(2)), None]);
    let kinds: Vec<_> = stored.entries.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [EntryKind::Update, EntryKind::Update, EntryKind::Snapshot]
    );
    let seqs: Vec<_> = stored.entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [Some(ServerSeq(1)), None, Some(ServerSeq(2))]);
    assert_eq!(stored.entries[2].bytes, remote_snapshot(n, 3, 2).bytes);
    assert!(stored.entries.windows(2).all(|w| w[0].pos < w[1].pos));

    assert_eq!(s.load_note(note(2)).unwrap(), None);
}

/// Locally created and remotely received notes are all listed.
pub fn list_notes<S: Storage>(mut s: S) {
    assert!(s.list_notes().unwrap().is_empty());
    s.create_note(note(1), b"a", local(1)).unwrap();
    s.create_note(note(2), b"b", local(2)).unwrap();
    let c = note(3);
    s.apply_remote(batch(vec![(c, b"c".to_vec())], vec![remote(c, 3, 1)], 1))
        .unwrap();
    assert_eq!(s.list_notes().unwrap(), [note(1), note(2), c]);
}

/// The outbox keeps append order across notes.
pub fn outbox_order<S: Storage>(mut s: S) {
    s.create_note(note(1), b"a", local(1)).unwrap();
    s.create_note(note(2), b"b", local(2)).unwrap();
    s.acknowledge(uid(1), ServerSeq(1)).unwrap();
    s.acknowledge(uid(2), ServerSeq(2)).unwrap();
    s.append_local(note(1), local(3)).unwrap();
    s.append_local(note(2), local(4)).unwrap();
    s.append_local(note(1), local(5)).unwrap();

    let outbox = s.outbox().unwrap();
    let got: Vec<_> = outbox.iter().map(|e| (e.note, e.id)).collect();
    assert_eq!(
        got,
        [(note(1), uid(3)), (note(2), uid(4)), (note(1), uid(5))]
    );
    assert!(outbox.windows(2).all(|w| w[0].pos < w[1].pos));
}

/// A local update with a stored ID fails and leaves the log unchanged, even
/// when the ID is on another note or was acknowledged.
pub fn duplicate_update_id<S: Storage>(mut s: S) {
    s.create_note(note(1), b"a", local(1)).unwrap();
    s.create_note(note(2), b"b", local(2)).unwrap();
    s.acknowledge(uid(1), ServerSeq(1)).unwrap();
    let before = (load(&s, note(1)), load(&s, note(2)), s.outbox().unwrap());

    for id in [uid(1), uid(2)] {
        let update = LocalUpdate {
            id,
            bytes: b"new".to_vec(),
        };
        assert_eq!(
            s.append_local(note(1), update),
            Err(StorageError::DuplicateUpdate)
        );
    }
    assert_eq!(
        (load(&s, note(1)), load(&s, note(2)), s.outbox().unwrap()),
        before
    );
}

/// Acknowledging removes from the outbox and records the sequence; repeating
/// it, or acknowledging an unknown ID, changes nothing.
pub fn acknowledge<S: Storage>(mut s: S) {
    s.create_note(note(1), b"key", local(1)).unwrap();
    s.append_local(note(1), local(2)).unwrap();
    s.acknowledge(uid(1), ServerSeq(42)).unwrap();

    assert_eq!(outbox_ids(&s), [uid(2)]);
    let stored = load(&s, note(1));
    assert_eq!(entry_ids(&stored), [Some(uid(1)), Some(uid(2))]);
    assert_eq!(stored.entries[0].seq, Some(ServerSeq(42)));
    assert_eq!(stored.entries[1].seq, None);

    s.acknowledge(uid(1), ServerSeq(50)).unwrap();
    s.acknowledge(uid(9), ServerSeq(51)).unwrap();
    assert_eq!(load(&s, note(1)), stored);
    assert_eq!(outbox_ids(&s), [uid(2)]);
}

/// The cursor starts at zero, accepts equal batches, and never regresses.
pub fn cursor<S: Storage>(mut s: S) {
    assert_eq!(s.cursor().unwrap(), ServerSeq(0));
    let n = note(1);
    s.apply_remote(batch(
        vec![(n, b"key".to_vec())],
        vec![remote(n, 1, 10)],
        10,
    ))
    .unwrap();
    assert_eq!(s.cursor().unwrap(), ServerSeq(10));

    assert_eq!(
        s.apply_remote(batch(vec![], vec![remote(n, 2, 7)], 7)),
        Err(StorageError::CursorRegressed {
            current: ServerSeq(10),
            requested: ServerSeq(7),
        })
    );
    assert_eq!(s.cursor().unwrap(), ServerSeq(10));
    assert_eq!(entry_ids(&load(&s, n)), [Some(uid(1))]);

    s.apply_remote(batch(vec![], vec![remote(n, 1, 10)], 10))
        .unwrap();
    assert_eq!(s.cursor().unwrap(), ServerSeq(10));
    assert_eq!(entry_ids(&load(&s, n)), [Some(uid(1))]);
}

/// A batch carrying a new note's key and first update stores both and
/// advances the cursor.
pub fn remote_batch_with_new_note<S: Storage>(mut s: S) {
    let n = note(1);
    s.apply_remote(batch(vec![(n, b"key".to_vec())], vec![remote(n, 1, 5)], 5))
        .unwrap();
    assert_eq!(s.list_notes().unwrap(), [n]);
    let stored = load(&s, n);
    assert_eq!(stored.wrapped_key, b"key");
    assert_eq!(entry_ids(&stored), [Some(uid(1))]);
    assert_eq!(stored.entries[0].seq, Some(ServerSeq(5)));
    assert_eq!(stored.entries[0].bytes, remote(n, 1, 5).bytes);
    assert_eq!(s.cursor().unwrap(), ServerSeq(5));
    assert!(s.outbox().unwrap().is_empty());
}

/// A batch with an entry for an unknown note stores nothing from the batch:
/// no keys, no entries, no echo acknowledgments, no cursor move.
pub fn remote_batch_rolls_back<S: Storage>(mut s: S) {
    s.create_note(note(1), b"mine", local(1)).unwrap();
    let before = (load(&s, note(1)), s.outbox().unwrap(), s.cursor().unwrap());

    let new = note(2);
    let unknown = note(3);
    let result = s.apply_remote(batch(
        vec![(new, b"key".to_vec())],
        vec![
            remote(new, 2, 1),
            remote(note(1), 1, 2),
            remote(unknown, 3, 3),
        ],
        3,
    ));
    assert_eq!(result, Err(StorageError::NoteNotFound(unknown)));
    assert_eq!(s.list_notes().unwrap(), [note(1)]);
    assert_eq!(s.load_note(new).unwrap(), None);
    assert_eq!(
        (load(&s, note(1)), s.outbox().unwrap(), s.cursor().unwrap()),
        before
    );
}

/// A device's own update coming back is stored once and acknowledged; an
/// already acknowledged echo, or a repeat within one batch, is skipped.
pub fn remote_echoes<S: Storage>(mut s: S) {
    let n = note(1);
    s.create_note(n, b"key", local(1)).unwrap();
    s.append_local(n, local(2)).unwrap();
    s.acknowledge(uid(1), ServerSeq(3)).unwrap();

    s.apply_remote(batch(
        vec![],
        vec![remote(n, 1, 8), remote(n, 2, 9), remote(n, 2, 9)],
        9,
    ))
    .unwrap();
    let stored = load(&s, n);
    assert_eq!(entry_ids(&stored), [Some(uid(1)), Some(uid(2))]);
    assert_eq!(stored.entries[0].seq, Some(ServerSeq(3)));
    assert_eq!(stored.entries[1].seq, Some(ServerSeq(9)));
    assert!(s.outbox().unwrap().is_empty());

    s.apply_remote(batch(vec![], vec![remote(n, 4, 10), remote(n, 4, 10)], 10))
        .unwrap();
    assert_eq!(
        entry_ids(&load(&s, n)),
        [Some(uid(1)), Some(uid(2)), Some(uid(4))]
    );
}

/// A key for a stored note in a later batch doesn't replace the stored key or
/// its pending state.
pub fn remote_key_kept<S: Storage>(mut s: S) {
    let n = note(1);
    s.apply_remote(batch(
        vec![(n, b"first".to_vec())],
        vec![remote(n, 1, 1)],
        1,
    ))
    .unwrap();
    s.apply_remote(batch(
        vec![(n, b"second".to_vec())],
        vec![remote(n, 2, 2)],
        2,
    ))
    .unwrap();
    assert_eq!(load(&s, n).wrapped_key, b"first");

    let m = note(2);
    s.create_note(m, b"local", local(3)).unwrap();
    s.apply_remote(batch(vec![(m, b"remote".to_vec())], vec![], 3))
        .unwrap();
    assert_eq!(load(&s, m).wrapped_key, b"local");
    assert_eq!(s.pending_keys().unwrap(), [(m, b"local".to_vec())]);
}

/// Local keys are pending until marked uploaded; remote keys never are.
pub fn pending_keys<S: Storage>(mut s: S) {
    s.create_note(note(1), b"a", local(1)).unwrap();
    s.create_note(note(2), b"b", local(2)).unwrap();
    let r = note(3);
    s.apply_remote(batch(vec![(r, b"r".to_vec())], vec![remote(r, 3, 1)], 1))
        .unwrap();
    assert_eq!(
        s.pending_keys().unwrap(),
        [(note(1), b"a".to_vec()), (note(2), b"b".to_vec())]
    );

    s.mark_key_uploaded(note(1)).unwrap();
    s.mark_key_uploaded(note(9)).unwrap();
    assert_eq!(s.pending_keys().unwrap(), [(note(2), b"b".to_vec())]);
    s.mark_key_uploaded(note(2)).unwrap();
    assert!(s.pending_keys().unwrap().is_empty());
}

/// Compaction folds acknowledged entries into the base, keeps pending ones,
/// and keeps entries after the cutoff.
pub fn compaction<S: Storage>(mut s: S) {
    // Acknowledged entries are folded in.
    let a = note(1);
    s.create_note(a, b"a", local(1)).unwrap();
    s.append_local(a, local(2)).unwrap();
    s.acknowledge(uid(1), ServerSeq(1)).unwrap();
    s.acknowledge(uid(2), ServerSeq(2)).unwrap();
    let through = pos_of(&s, a, uid(2));
    s.compact(a, b"snap-a", through).unwrap();
    let stored = load(&s, a);
    assert_eq!(stored.base.as_deref(), Some(&b"snap-a"[..]));
    assert!(stored.entries.is_empty());
    assert_eq!(stored.wrapped_key, b"a");

    // Pending entries survive.
    let b = note(2);
    s.create_note(b, b"b", local(11)).unwrap();
    s.acknowledge(uid(11), ServerSeq(3)).unwrap();
    s.append_local(b, local(12)).unwrap();
    s.append_local(b, local(13)).unwrap();
    s.acknowledge(uid(13), ServerSeq(4)).unwrap();
    s.compact(b, b"snap-b", pos_of(&s, b, uid(13))).unwrap();
    let stored = load(&s, b);
    assert_eq!(stored.base.as_deref(), Some(&b"snap-b"[..]));
    assert_eq!(entry_ids(&stored), [Some(uid(12))]);
    assert_eq!(outbox_ids(&s), [uid(12)]);

    // Entries stored after the snapshot was taken are kept, and a new base
    // replaces the old one.
    s.apply_remote(batch(vec![], vec![remote(a, 3, 5)], 5))
        .unwrap();
    s.compact(a, b"snap-a2", through).unwrap();
    let stored = load(&s, a);
    assert_eq!(stored.base.as_deref(), Some(&b"snap-a2"[..]));
    assert_eq!(entry_ids(&stored), [Some(uid(3))]);

    // Other notes are untouched.
    assert_eq!(entry_ids(&load(&s, b)), [Some(uid(12))]);

    assert_eq!(
        s.compact(note(9), b"x", through),
        Err(StorageError::NoteNotFound(note(9)))
    );
}

/// Deleting a note removes everything for it, including outbox entries;
/// deleting an absent note does nothing.
pub fn delete_note<S: Storage>(mut s: S) {
    s.create_note(note(1), b"a", local(1)).unwrap();
    s.create_note(note(2), b"b", local(2)).unwrap();
    s.append_local(note(1), local(3)).unwrap();
    s.delete_note(note(1)).unwrap();

    assert_eq!(s.list_notes().unwrap(), [note(2)]);
    assert_eq!(s.load_note(note(1)).unwrap(), None);
    assert_eq!(outbox_ids(&s), [uid(2)]);
    assert_eq!(s.pending_keys().unwrap(), [(note(2), b"b".to_vec())]);

    s.delete_note(note(1)).unwrap();
    s.delete_note(note(9)).unwrap();
    assert_eq!(s.list_notes().unwrap(), [note(2)]);

    // The deleted note's update IDs are free again.
    s.create_note(note(1), b"a", local(1)).unwrap();
}

/// Clearing a populated store succeeds.
pub fn clear<S: Storage>(mut s: S) {
    s.create_note(note(1), b"a", local(1)).unwrap();
    let n = note(2);
    s.apply_remote(batch(
        vec![(n, b"key".to_vec())],
        vec![remote(n, 2, 12)],
        12,
    ))
    .unwrap();
    s.set_setting("editor.vim", "true").unwrap();
    s.clear().unwrap();
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
