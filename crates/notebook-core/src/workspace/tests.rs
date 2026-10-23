//! Workspace tests over real replicas and engines against the fake server.

use std::time::Duration;

use futures::channel::mpsc::UnboundedReceiver;
use futures::executor::{LocalPool, LocalSpawner};
use futures::task::LocalSpawnExt;

use super::*;
use crate::crypto::VaultId;
use crate::storage::MemoryStorage;
use crate::sync::fake::{FakeServer, FakeSession, ManualTimer, vault_keys};
use crate::sync::{SyncConfig, SyncEngine};

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

struct Sim {
    pool: LocalPool,
    spawner: LocalSpawner,
    timer: ManualTimer,
    server: FakeServer,
}

struct Device {
    ws: Workspace<MemoryStorage>,
    changes: UnboundedReceiver<Changed>,
    /// Engine events, for the test to pass to the workspace as a host would.
    events: UnboundedReceiver<SyncEvent>,
}

impl Sim {
    fn new() -> Self {
        let pool = LocalPool::new();
        let spawner = pool.spawner();
        let timer = ManualTimer::new();
        let server = FakeServer::new(timer.clone());
        Self {
            pool,
            spawner,
            timer,
            server,
        }
    }

    fn device(&mut self) -> Device {
        self.device_on(MemoryStorage::new(vault()))
    }

    fn device_on(&mut self, storage: MemoryStorage) -> Device {
        let [key] = vault_keys(vault());
        let replica = Shared::new(Replica::open(storage, key).unwrap());
        let (engine, handle, events) = SyncEngine::new(
            replica.clone(),
            self.server.clone(),
            self.timer.clone(),
            FakeSession::new(&self.server, Duration::from_secs(900)),
            SyncConfig {
                jitter: false,
                ..SyncConfig::default()
            },
            Box::new(|| Ok(MemoryStorage::new(vault()))),
        );
        self.spawner.spawn_local(engine.run()).unwrap();
        let (ws, changes) = Workspace::open(replica, handle);
        let mut device = Device {
            ws,
            changes,
            events,
        };
        self.settle(&mut device);
        device
    }

    /// Lets every engine catch up, pushing anything debounced.
    fn sync(&mut self, devices: &mut [&mut Device]) {
        for _ in 0..3 {
            self.pool.run_until_stalled();
            self.timer.advance(Duration::from_secs(5));
            self.pool.run_until_stalled();
            for device in devices.iter_mut() {
                self.settle(device);
            }
        }
    }

    /// Passes the device's queued engine events to its workspace.
    fn settle(&mut self, device: &mut Device) {
        self.pool.run_until_stalled();
        while let Ok(event) = device.events.try_recv() {
            device.ws.apply_sync_event(&event);
        }
    }

    fn run<T>(&mut self, future: impl Future<Output = T>) -> T {
        self.pool.run_until(future)
    }
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

    fn outbox(&self) -> usize {
        self.ws.replica().lock().pending().unwrap()
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
    let mut sim = Sim::new();
    let mut a = sim.device();
    let work = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(work, "Plan\n#work/acme plumber", T1).unwrap();
    let storage = a.ws.replica().lock().storage().unwrap().clone();

    let b = sim.device_on(storage);
    assert_eq!(names(b.ws.tree().roots()), ["work"]);
    assert_eq!(names(&b.ws.tree().roots()[0].children), ["acme"]);
    assert_eq!(b.titles(&NOTES, "plumber"), ["Plan"]);
}

#[test]
fn all_notes_includes_archived_and_trashed() {
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
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

#[test]
fn remote_update_produces_a_notification() {
    let mut sim = Sim::new();
    let mut a = sim.device();
    let mut b = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "Shopping\n#home", T1).unwrap();
    sim.sync(&mut [&mut a, &mut b]);
    b.drain();

    a.ws.set_body(id, "Groceries\n#home", T2).unwrap();
    sim.sync(&mut [&mut a, &mut b]);
    let changed = b.drain();
    assert!(changed.iter().any(|c| c.notes.contains(&id)), "{changed:?}");
    assert_eq!(b.titles(&NOTES, ""), ["Groceries"]);
    assert_eq!(b.titles(&tag("home"), "groceries"), ["Groceries"]);
}

// ---- 3.2 list ----

#[test]
fn each_special_view() {
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, &format!("Title\n{}", "word ".repeat(100)), T1)
        .unwrap();
    let preview = &a.ws.list(&NOTES, "")[0].preview;
    assert!(preview.chars().count() <= PREVIEW_CHARS);
    assert!(preview.starts_with("word word"));
}

#[test]
fn search_is_scoped_to_the_selection_and_live() {
    let mut sim = Sim::new();
    let mut a = sim.device();
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

// ---- 3.3 create, flags, trash, purge ----

#[test]
fn create_under_a_tag() {
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
    let old = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(old, "Old", T1).unwrap();
    let id = a.ws.create(&NOTES, T2).unwrap();
    assert_eq!(a.body(id), "");
    assert_eq!(a.ws.list(&NOTES, "")[0].id, id);
}

#[test]
fn view_membership_follows_flags() {
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "Keep", T1).unwrap();
    let (session, _) = a.ws.editor_attach(id).unwrap();
    a.ws.set_trashed(id, true).unwrap();
    let before = a.outbox();

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
    assert_eq!(a.outbox(), before);

    a.ws.set_trashed(id, false).unwrap();
    a.ws.set_body(id, "Changed", T2).unwrap();
    assert_eq!(a.body(id), "Changed");
}

#[test]
fn purge_only_trashed_notes() {
    let mut sim = Sim::new();
    let mut a = sim.device();
    let mut b = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "Doomed", T1).unwrap();
    sim.sync(&mut [&mut a, &mut b]);

    assert_eq!(a.ws.purge(id).err(), Some(WorkspaceError::NotTrashed(id)));

    a.ws.set_trashed(id, true).unwrap();
    let purge = a.ws.purge(id).unwrap();
    sim.run(purge).unwrap();
    a.ws.refresh(&[id]);
    assert!(a.ws.note(id).is_none());
    assert!(a.ws.list(&Selection::View(View::Trash), "").is_empty());

    sim.sync(&mut [&mut a, &mut b]);
    assert!(b.ws.note(id).is_none());
    assert!(b.ws.all_notes().is_empty());
}

#[test]
fn purge_while_offline_leaves_the_note_in_trash() {
    let mut sim = Sim::new();
    let mut a = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_trashed(id, true).unwrap();
    sim.sync(&mut [&mut a]);

    sim.server.set_reachable(false);
    let purge = a.ws.purge(id).unwrap();
    assert_eq!(
        sim.run(purge),
        Err(WorkspaceError::Sync(SyncError::Offline))
    );
    assert_eq!(a.ws.list(&Selection::View(View::Trash), "").len(), 1);
}

// ---- 3.4 rename ----

#[test]
fn rename_a_tag() {
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "for #acme", T1).unwrap();
    a.ws.rename_tag("acme", "clients/acme", T2).unwrap();
    assert_eq!(a.body(id), "for #clients/acme");
    assert_eq!(names(a.ws.tree().roots()), ["clients"]);
    assert_eq!(names(&a.ws.tree().roots()[0].children), ["acme"]);
}

#[test]
fn rename_into_an_existing_tag_merges() {
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
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
    let mut sim = Sim::new();
    let mut a = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.ws.set_body(id, "#work", T1).unwrap();
    let before = a.outbox();
    a.drain();
    assert_eq!(
        a.ws.rename_tag("work", "42", T2),
        Err(WorkspaceError::InvalidTagName("42".into()))
    );
    assert_eq!(a.body(id), "#work");
    assert_eq!(a.outbox(), before);
    assert!(a.drain().is_empty());
}

#[test]
fn rename_queues_one_update_per_affected_note() {
    let mut sim = Sim::new();
    let mut a = sim.device();
    let mut ids = Vec::new();
    for body in ["#work one", "#work/acme two", "#home three"] {
        let id = a.ws.create(&NOTES, T1).unwrap();
        a.ws.set_body(id, body, T1).unwrap();
        ids.push(id);
    }
    sim.sync(&mut [&mut a]);
    assert_eq!(a.outbox(), 0);

    a.ws.rename_tag("work", "job", T2).unwrap();
    let outbox = a.ws.replica().lock().storage().unwrap().outbox().unwrap();
    let mut notes: Vec<NoteId> = outbox.iter().map(|e| e.note).collect();
    notes.sort();
    let mut expected = vec![ids[0], ids[1]];
    expected.sort();
    assert_eq!(notes, expected);
}

// ---- reload ----

#[test]
fn reload_reports_everything() {
    let mut sim = Sim::new();
    let mut a = sim.device();
    let id = a.ws.create(&NOTES, T1).unwrap();
    a.drain();
    a.ws.reload();
    assert_eq!(
        a.drain(),
        [Changed {
            notes: vec![id],
            tree_changed: true
        }]
    );
}
