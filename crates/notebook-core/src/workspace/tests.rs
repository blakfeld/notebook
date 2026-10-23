//! Workspace tests over real replicas on in-memory stores.

use futures::channel::mpsc::UnboundedReceiver;

use super::*;
use crate::crypto::VaultId;
use crate::replica::vault_keys;
use crate::storage::MemoryStorage;

const T1: Timestamp = Timestamp(1_700_000_000_000);
const T2: Timestamp = Timestamp(1_700_000_001_000);
const T3: Timestamp = Timestamp(1_700_000_002_000);
const T4: Timestamp = Timestamp(1_700_000_003_000);

fn vault() -> VaultId {
    VaultId::from_parts(7, 7)
}

fn tag(path: &str) -> Selection {
    Selection::Tag(TagPath::parse(path).unwrap())
}

const NOTES: Selection = Selection::View(View::Notes);

struct Device {
    ws: Workspace<MemoryStorage>,
    changes: UnboundedReceiver<Changed>,
}

fn device() -> Device {
    device_on(MemoryStorage::new(vault()))
}

fn device_on(storage: MemoryStorage) -> Device {
    let [key] = vault_keys(vault());
    let replica = Shared::new(Replica::open(storage, key).unwrap());
    let (ws, changes) = Workspace::open(replica);
    Device { ws, changes }
}

impl Device {
    fn drain(&mut self) -> Vec<Changed> {
        let mut out = Vec::new();
        while let Ok(changed) = self.changes.try_recv() {
            out.push(changed);
        }
        out
    }

    fn titles(&self, selection: &Selection, query: &str) -> Vec<String> {
        self.ws
            .list(selection, query)
            .into_iter()
            .map(|e| e.title)
            .collect()
    }

    /// How many updates are stored across every note.
    fn stored_updates(&self) -> usize {
        let replica = self.ws.replica().lock();
        let store = replica.storage().unwrap();
        store
            .list_notes()
            .unwrap()
            .into_iter()
            .map(|id| store.load_note(id).unwrap().unwrap().entries.len())
            .sum()
    }

    fn body(&self, id: NoteId) -> String {
        self.ws.note(id).unwrap().body().to_owned()
    }
}

fn names(nodes: &[tags::TagNode]) -> Vec<String> {
    nodes.iter().map(|n| n.name.clone()).collect()
}

// ---- 3.1 open, all_notes, notifications ----

#[test]
fn opening_rebuilds_tree_and_index() {
    let mut a = device();
    let work = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(work, "Plan\n#work/acme plumber", T1).unwrap();
    let storage = a.ws.replica().lock().storage().unwrap().clone();

    let b = device_on(storage);
    assert_eq!(names(b.ws.tree().roots()), ["work"]);
    assert_eq!(names(&b.ws.tree().roots()[0].children), ["acme"]);
    assert_eq!(b.titles(&NOTES, "plumber"), ["Plan"]);
}

#[test]
fn all_notes_includes_archived_and_trashed() {
    let mut a = device();
    let kept = a.ws.create(&NOTES, T1).unwrap();
    let archived = a.ws.create(&NOTES, T1).unwrap();
    let trashed = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_archived(archived, true).unwrap();
    a.ws.set_trashed(trashed, true).unwrap();
    let mut ids: Vec<NoteId> = a.ws.all_notes().iter().map(Note::id).collect();
    ids.sort();
    let mut expected = vec![kept, archived, trashed];
    expected.sort();
    assert_eq!(ids, expected);
}

#[test]
fn local_edits_notify_with_tree_flag() {
    let mut a = device();
    a.drain();
    let id = a.ws.create(&NOTES, T1).unwrap();
    assert_eq!(
        a.drain(),
        [Changed {
            notes: vec![id],
            tree_changed: false
        }]
    );
    a.ws.set_body(id, "typing #ideas", T2).unwrap();
    assert_eq!(
        a.drain(),
        [Changed {
            notes: vec![id],
            tree_changed: true
        }]
    );
    assert_eq!(names(a.ws.tree().roots()), ["ideas"]);
    a.ws.set_body(id, "typing #ideas more", T3).unwrap();
    assert!(!a.drain()[0].tree_changed);
}

// ---- 3.2 list ----

#[test]
fn each_special_view() {
    let mut a = device();
    let tagged = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(tagged, "Tagged #work", T1).unwrap();
    let plain = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(plain, "Plain", T2).unwrap();
    let pinned = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(pinned, "Pinned", T3).unwrap();
    a.ws.set_pinned(pinned, true).unwrap();
    let archived = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(archived, "Archived", T1).unwrap();
    a.ws.set_archived(archived, true).unwrap();
    let trashed = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(trashed, "Trashed", T1).unwrap();
    a.ws.set_trashed(trashed, true).unwrap();

    let view = |v| Selection::View(v);
    assert_eq!(
        a.titles(&view(View::Notes), ""),
        ["Pinned", "Plain", "Tagged #work"]
    );
    assert_eq!(a.titles(&view(View::Untagged), ""), ["Pinned", "Plain"]);
    assert_eq!(a.titles(&view(View::Pinned), ""), ["Pinned"]);
    assert_eq!(a.titles(&view(View::Archive), ""), ["Archived"]);
    assert_eq!(a.titles(&view(View::Trash), ""), ["Trashed"]);
}

#[test]
fn parent_tag_includes_descendants_newest_first() {
    let mut a = device();
    let parent = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(parent, "Parent #work", T1).unwrap();
    let child = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(child, "Child #work/acme", T2).unwrap();
    let other = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(other, "Other #home", T3).unwrap();

    assert_eq!(
        a.titles(&tag("work"), ""),
        ["Child #work/acme", "Parent #work"]
    );
    assert_eq!(a.titles(&tag("Work/ACME"), ""), ["Child #work/acme"]);
    assert!(a.titles(&tag("nothing"), "").is_empty());

    a.ws.set_body(parent, "Parent #work edited", T4).unwrap();
    assert_eq!(
        a.titles(&tag("work"), ""),
        ["Parent #work edited", "Child #work/acme"]
    );
}

#[test]
fn entries_carry_title_preview_and_flags() {
    let mut a = device();
    let empty = a.ws.create(&NOTES, T1).unwrap();
    let full = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(
        full,
        "\n# Weekly plan\n\n## Groceries\n- [ ] eggs\n> quoted   text\n```\ncode\n```\n1. first",
        T2,
    )
    .unwrap();
    a.ws.set_pinned(full, true).unwrap();

    let entries = a.ws.list(&NOTES, "");
    assert_eq!(entries[0].id, full);
    assert_eq!(entries[0].title, "Weekly plan");
    assert_eq!(entries[0].preview, "Groceries eggs quoted text code first");
    assert_eq!(entries[0].updated, T2);
    assert!(entries[0].pinned && !entries[0].archived && !entries[0].trashed);
    assert_eq!(entries[1].id, empty);
    assert_eq!(entries[1].title, "");
    assert_eq!(entries[1].preview, "");
}

#[test]
fn long_previews_are_cut() {
    let mut a = device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, &format!("Title\n{}", "word ".repeat(100)), T1)
        .unwrap();
    let preview = &a.ws.list(&NOTES, "")[0].preview;
    assert!(preview.chars().count() <= PREVIEW_CHARS);
    assert!(preview.starts_with("word word"));
}

#[test]
fn search_is_scoped_to_the_selection_and_live() {
    let mut a = device();
    let archived = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(archived, "Old plumber sink", T1).unwrap();
    a.ws.set_archived(archived, true).unwrap();
    let active = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(active, "New plumber sink", T2).unwrap();

    assert_eq!(
        a.titles(&Selection::View(View::Archive), "SINK plumber"),
        ["Old plumber sink"]
    );
    assert_eq!(a.titles(&NOTES, "sink"), ["New plumber sink"]);
    a.ws.set_body(active, "New plumber", T3).unwrap();
    assert!(a.titles(&NOTES, "sink").is_empty());
}

// ---- 3.3 create, flags, trash, delete ----

#[test]
fn create_under_a_tag() {
    let mut a = device();
    let seed = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(seed, "#Work/Acme", T1).unwrap();

    let id = a.ws.create(&tag("work/acme"), T2).unwrap();
    // The tree's spelling is used.
    assert_eq!(a.body(id), "#Work/Acme\n");
    assert!(a.ws.list(&tag("work/acme"), "").iter().any(|e| e.id == id));

    let multi = a.ws.create(&tag("reading list/to buy"), T3).unwrap();
    assert_eq!(a.body(multi), "#reading list/to buy#\n");
    assert!(
        a.ws.list(&tag("reading list/to buy"), "")
            .iter()
            .any(|e| e.id == multi)
    );
}

#[test]
fn create_in_notes_is_empty_and_first() {
    let mut a = device();
    let old = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(old, "Old", T1).unwrap();
    let id = a.ws.create(&NOTES, T2).unwrap();
    assert_eq!(a.body(id), "");
    assert_eq!(a.ws.list(&NOTES, "")[0].id, id);
}

#[test]
fn view_membership_follows_flags() {
    let mut a = device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "Note #work", T1).unwrap();
    let in_view = |a: &Device, v: View| a.ws.list(&Selection::View(v), "").len();

    a.ws.set_pinned(id, true).unwrap();
    assert_eq!(
        (in_view(&a, View::Pinned), in_view(&a, View::Notes)),
        (1, 1)
    );
    a.ws.set_pinned(id, false).unwrap();
    assert_eq!(in_view(&a, View::Pinned), 0);

    a.ws.set_archived(id, true).unwrap();
    assert_eq!(
        (in_view(&a, View::Archive), in_view(&a, View::Notes)),
        (1, 0)
    );
    assert!(a.ws.list(&tag("work"), "").is_empty());
    assert!(a.ws.tree().roots().is_empty());

    // Trashing an archived note and restoring it returns it to Archive.
    a.ws.set_trashed(id, true).unwrap();
    assert_eq!(
        (in_view(&a, View::Trash), in_view(&a, View::Archive)),
        (1, 0)
    );
    a.ws.set_trashed(id, false).unwrap();
    assert_eq!(
        (in_view(&a, View::Trash), in_view(&a, View::Archive)),
        (0, 1)
    );

    a.ws.set_archived(id, false).unwrap();
    assert_eq!(in_view(&a, View::Notes), 1);
    assert_eq!(a.ws.list(&tag("work"), "").len(), 1);
}

#[test]
fn trashed_notes_reject_body_changes() {
    let mut a = device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "Keep", T1).unwrap();
    let (session, _) = a.ws.editor_attach(id).unwrap();
    a.ws.set_trashed(id, true).unwrap();
    let before = a.stored_updates();

    assert_eq!(
        a.ws.set_body(id, "Changed", T2),
        Err(WorkspaceError::Trashed(id))
    );
    let update = EditorUpdate::from_bytes(Vec::new());
    assert_eq!(
        a.ws.apply_editor_update(session, &update, T2),
        Err(WorkspaceError::Trashed(id))
    );
    assert_eq!(a.body(id), "Keep");
    assert_eq!(a.stored_updates(), before);

    a.ws.set_trashed(id, false).unwrap();
    a.ws.set_body(id, "Changed", T2).unwrap();
    assert_eq!(a.body(id), "Changed");
}

#[test]
fn delete_permanently_only_trashed_notes() {
    let [key] = vault_keys(vault());
    let replica = Shared::new(Replica::open(MemoryStorage::new(vault()), key).unwrap());
    let (mut ws, mut changes) = Workspace::open(replica.clone());

    let id = ws.create(&NOTES, T1).unwrap();
    ws.set_body(id, "Doomed #old", T1).unwrap();
    let kept = ws.create(&NOTES, T1).unwrap();
    assert_eq!(
        ws.delete_permanently(id),
        Err(WorkspaceError::NotTrashed(id))
    );
    let unknown = NoteId::from_parts(1, 1);
    assert_eq!(
        ws.delete_permanently(unknown),
        Err(WorkspaceError::NotFound(unknown))
    );

    ws.set_trashed(id, true).unwrap();
    while changes.try_recv().is_ok() {}
    ws.delete_permanently(id).unwrap();
    assert!(ws.note(id).is_none());
    assert!(ws.list(&Selection::View(View::Trash), "").is_empty());
    assert!(ws.note(kept).is_some());
    let changed = changes.try_recv().unwrap();
    assert_eq!(changed.notes, [id]);
    assert_eq!(
        replica.lock().storage().unwrap().load_note(id).unwrap(),
        None
    );
}

// ---- 3.4 rename ----

#[test]
fn rename_a_tag() {
    let mut a = device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "#work/acme notes", T1).unwrap();
    a.drain();

    assert_eq!(a.ws.rename_tag("work", "job", T2), Ok(1));
    assert_eq!(a.body(id), "#job/acme notes");
    assert_eq!(names(a.ws.tree().roots()), ["job"]);
    assert_eq!(names(&a.ws.tree().roots()[0].children), ["acme"]);
    assert_eq!(
        a.drain(),
        [Changed {
            notes: vec![id],
            tree_changed: true
        }]
    );
}

#[test]
fn rename_moves_under_a_new_parent() {
    let mut a = device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "for #acme", T1).unwrap();
    a.ws.rename_tag("acme", "clients/acme", T2).unwrap();
    assert_eq!(a.body(id), "for #clients/acme");
    assert_eq!(names(a.ws.tree().roots()), ["clients"]);
    assert_eq!(names(&a.ws.tree().roots()[0].children), ["acme"]);
}

#[test]
fn rename_into_an_existing_tag_merges() {
    let mut a = device();
    let job = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(job, "one #job", T1).unwrap();
    let work = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(work, "two #work", T1).unwrap();
    assert_eq!(a.ws.rename_tag("job", "work", T2), Ok(1));
    assert_eq!(names(a.ws.tree().roots()), ["work"]);
    assert_eq!(a.ws.list(&tag("work"), "").len(), 2);
}

#[test]
fn rename_rewrites_archived_and_trashed_notes() {
    let mut a = device();
    let archived = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(archived, "a #work", T1).unwrap();
    a.ws.set_archived(archived, true).unwrap();
    let trashed = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(trashed, "t #work/x", T1).unwrap();
    a.ws.set_trashed(trashed, true).unwrap();

    assert_eq!(a.ws.rename_tag("work", "job", T2), Ok(2));
    assert_eq!(a.body(archived), "a #job");
    assert_eq!(a.body(trashed), "t #job/x");
}

#[test]
fn invalid_rename_changes_nothing() {
    let mut a = device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "#work", T1).unwrap();
    let before = a.stored_updates();
    a.drain();
    assert_eq!(
        a.ws.rename_tag("work", "42", T2),
        Err(WorkspaceError::InvalidTagName("42".into()))
    );
    assert_eq!(a.body(id), "#work");
    assert_eq!(a.stored_updates(), before);
    assert!(a.drain().is_empty());
}

#[test]
fn rename_queues_one_update_per_affected_note() {
    let mut a = device();
    let mut ids = Vec::new();
    for body in ["#work one", "#work/acme two", "#home three"] {
        let id = a.ws.create(&NOTES, T1).unwrap();
        a.ws.set_body(id, body, T1).unwrap();
        ids.push(id);
    }
    let log_len = |a: &Device, id: NoteId| {
        let replica = a.ws.replica().lock();
        let stored = replica.storage().unwrap().load_note(id).unwrap();
        stored.unwrap().entries.len()
    };
    let before: Vec<usize> = ids.iter().map(|&id| log_len(&a, id)).collect();

    a.ws.rename_tag("work", "job", T2).unwrap();
    let after: Vec<usize> = ids.iter().map(|&id| log_len(&a, id)).collect();
    assert_eq!(after, [before[0] + 1, before[1] + 1, before[2]]);
}
