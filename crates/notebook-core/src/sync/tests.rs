//! Engine tests: whole engines against the fake server, on one thread, with
//! a manual clock.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use futures::StreamExt;
use futures::executor::{LocalPool, LocalSpawner};
use futures::task::LocalSpawnExt;

use super::fake::{Endpoint, FakeServer, FakeSession, Fault, ManualTimer, NoteData, vault_keys};
use super::*;
use crate::crypto::VaultId;
use crate::document::NoteDocument;
use crate::note::{Note, Timestamp};
use crate::protocol::{self, CLOSE_RESYNC, CLOSE_SHUTDOWN};
use crate::storage::{MemoryStorage, Storage};

const TOKEN_TTL: Duration = Duration::from_secs(900);

fn vault() -> VaultId {
    VaultId::from_parts(7, 7)
}

fn config() -> SyncConfig {
    SyncConfig {
        jitter: false,
        ..SyncConfig::default()
    }
}

struct Sim {
    pool: LocalPool,
    spawner: LocalSpawner,
    timer: ManualTimer,
    server: FakeServer,
}

struct Device {
    replica: Shared<Replica<MemoryStorage>>,
    handle: SyncHandle,
    events: Rc<RefCell<Vec<SyncEvent>>>,
    session: FakeSession,
    timer: ManualTimer,
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
        self.device_with(config())
    }

    fn device_with(&mut self, config: SyncConfig) -> Device {
        let session = FakeSession::new(&self.server, TOKEN_TTL);
        self.start(MemoryStorage::new(vault()), session, config)
    }

    fn start(
        &mut self,
        storage: MemoryStorage,
        session: FakeSession,
        config: SyncConfig,
    ) -> Device {
        let [key] = vault_keys(vault());
        let replica = Shared::new(Replica::open(storage, key).unwrap());
        let (engine, handle, mut events_rx) = SyncEngine::new(
            replica.clone(),
            self.server.clone(),
            self.timer.clone(),
            session.clone(),
            config,
            Box::new(|| Ok(MemoryStorage::new(vault()))),
        );
        let events = Rc::new(RefCell::new(Vec::new()));
        let log = events.clone();
        self.spawner.spawn_local(engine.run()).unwrap();
        self.spawner
            .spawn_local(async move {
                while let Some(event) = events_rx.next().await {
                    log.borrow_mut().push(event);
                }
            })
            .unwrap();
        let device = Device {
            replica,
            handle,
            events,
            session,
            timer: self.timer.clone(),
        };
        self.run();
        device
    }

    /// Stops `device` and starts a new engine and replica on its storage,
    /// as after a process restart.
    fn restart(&mut self, device: Device, config: SyncConfig) -> Device {
        device.handle.shutdown();
        self.run();
        let storage = device.replica.lock().storage().unwrap().clone();
        self.start(storage, device.session, config)
    }

    fn run(&mut self) {
        self.pool.run_until_stalled();
    }

    /// Moves the clock forward by `by`, running every engine at each
    /// deadline on the way.
    fn advance(&mut self, by: Duration) {
        let target = self.timer.now() + by.as_millis() as u64;
        loop {
            self.run();
            match self.timer.next_deadline() {
                Some(at) if at <= target => self.timer.set(at),
                _ => break,
            }
        }
        self.timer.set(target);
        self.run();
    }

    fn flush(&mut self, device: &Device) -> Result<(), SyncError> {
        self.request(device.handle.clone(), |h| async move { h.flush().await })
    }

    fn purge(&mut self, device: &Device, note: NoteId) -> Result<(), SyncError> {
        self.request(device.handle.clone(), move |h| async move {
            h.purge(note).await
        })
    }

    fn request<F>(
        &mut self,
        handle: SyncHandle,
        call: impl FnOnce(SyncHandle) -> F,
    ) -> Result<(), SyncError>
    where
        F: Future<Output = Result<(), SyncError>> + 'static,
    {
        let result = Rc::new(RefCell::new(None));
        let out = result.clone();
        let fut = call(handle);
        self.spawner
            .spawn_local(async move {
                *out.borrow_mut() = Some(fut.await);
            })
            .unwrap();
        for _ in 0..200 {
            self.advance(Duration::from_millis(500));
            if let Some(result) = result.borrow_mut().take() {
                return result;
            }
        }
        panic!("request never completed");
    }

    /// Builds a snapshot from the server's data with the vault key and
    /// stores it, as another client compacting the note would.
    fn compact_on_server(&self, note: NoteId) {
        let NoteData { key, snapshot, ops } = self.server.note_data(note);
        let Some(key) = key else { return };
        let [vault_key] = vault_keys(vault());
        let note_key = vault_key.unwrap_note_key(vault(), note, &key).unwrap();
        let mut doc = NoteDocument::open(vault(), note, note_key).unwrap();
        if let Some(snapshot) = snapshot {
            doc.load_snapshot(&snapshot).unwrap();
        }
        for op in ops {
            doc.apply_update(&op).unwrap();
        }
        let data = doc.snapshot().unwrap().into_bytes();
        let _ = self.server.put_snapshot(note, self.server.head(), data);
    }
}

impl Device {
    fn now(&self) -> Timestamp {
        Timestamp(1_700_000_000_000 + self.timer.now() as i64)
    }

    fn create(&self, body: &str) -> NoteId {
        let now = self.now();
        self.replica.lock().create_note(body, now).unwrap()
    }

    fn set_body(&self, id: NoteId, body: &str) {
        let now = self.now();
        self.replica.lock().set_body(id, body, now).unwrap();
    }

    fn body(&self, id: NoteId) -> Option<String> {
        self.replica.lock().note(id).map(|n| n.body().to_owned())
    }

    fn notes(&self) -> Vec<Note> {
        self.replica.lock().notes()
    }

    fn pending(&self) -> usize {
        self.replica.lock().pending().unwrap()
    }

    fn cursor(&self) -> u64 {
        self.replica.lock().storage().unwrap().cursor().unwrap().0
    }

    fn status(&self) -> SyncStatus {
        self.events
            .borrow()
            .iter()
            .rev()
            .find_map(|e| match e {
                SyncEvent::Status(s) => Some(*s),
                _ => None,
            })
            .expect("a status was emitted")
    }

    fn statuses(&self) -> Vec<SyncStatus> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                SyncEvent::Status(s) => Some(*s),
                _ => None,
            })
            .collect()
    }

    fn changes(&self) -> Vec<Vec<NoteId>> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                SyncEvent::NotesChanged(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    fn warnings(&self) -> Vec<NoteId> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                SyncEvent::IntegrityWarning(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    fn clear_events(&self) {
        self.events.borrow_mut().clear();
    }
}

fn synced(pending: usize) -> SyncStatus {
    SyncStatus {
        state: SyncState::Synced,
        pending,
        attention: None,
        integrity_warnings: 0,
    }
}

// ---- 4.1 ----

#[test]
fn engine_starts_reports_status_and_shuts_down() {
    let mut sim = Sim::new();
    let a = sim.device();
    let first = a.statuses()[0];
    assert_eq!(first.state, SyncState::Syncing);
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.status(), synced(0));
    assert_eq!(sim.server.connections(), 1);
    a.handle.shutdown();
    sim.run();
    assert_eq!(sim.server.connections(), 0);
    assert_eq!(sim.flush(&a), Err(SyncError::Stopped));
}

// ---- 4.2 Pull and bootstrap ----

#[test]
fn new_device_sees_the_whole_vault() {
    let mut sim = Sim::new();
    sim.server.set_page_groups(2);
    let a = sim.device();
    let ids = [a.create("one"), a.create("two"), a.create("three")];
    sim.advance(Duration::from_secs(3));
    assert_eq!(a.pending(), 0);

    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.notes(), a.notes());
    for id in ids {
        assert!(b.body(id).is_some());
    }
    assert_eq!(b.cursor(), sim.server.head());
}

#[test]
fn interrupted_bootstrap_resumes_from_the_last_stored_page() {
    let mut sim = Sim::new();
    sim.server.set_page_groups(1);
    let a = sim.device();
    for i in 0..4 {
        a.create(&format!("note {i}"));
    }
    sim.advance(Duration::from_secs(3));

    // b's third page request fails after two pages are stored.
    sim.server
        .inject_nth(Endpoint::Changes, 2, Fault::DropRequest);
    let before = sim.server.count(Endpoint::Changes);
    let b = sim.device();
    let partial = b.cursor();
    assert!(partial > 0 && partial < sim.server.head(), "{partial}");
    assert_eq!(b.status().state, SyncState::Offline);
    sim.advance(Duration::from_secs(2));
    assert_eq!(b.notes(), a.notes());
    assert_eq!(b.notes().len(), 4);
    // Four pages plus the failed request: it resumed rather than restarting.
    assert_eq!(sim.server.count(Endpoint::Changes) - before, 5);
}

#[test]
fn crash_during_pull_repulls_the_page() {
    let mut sim = Sim::new();
    sim.server.set_page_groups(1);
    let a = sim.device();
    a.create("one");
    a.create("two");
    sim.advance(Duration::from_secs(3));

    // b gets page 1; page 2's response never arrives, and b dies.
    sim.server
        .inject_nth(Endpoint::Changes, 1, Fault::DropResponse);
    let b = sim.device();
    let partial = b.cursor();
    assert!(partial > 0 && partial < sim.server.head());
    let b = sim.restart(b, config());
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.notes(), a.notes());
    assert_eq!(b.cursor(), sim.server.head());
}

#[test]
fn remote_edit_arrives_and_moves_the_cursor() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    let id = a.create("hello");
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.body(id).as_deref(), Some("hello"));
    a.set_body(id, "hello world");
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.body(id).as_deref(), Some("hello world"));
    assert_eq!(b.cursor(), sim.server.head());
}

// ---- 4.3 Push ----

#[test]
fn failed_push_is_retried_with_the_same_ids() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.inject(Endpoint::LivePush, Fault::DropRequest);
    let id = a.create("draft");
    let queued: Vec<_> = a.replica.lock().storage().unwrap().outbox().unwrap();
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.pending(), 1);
    // After the push result timeout, it goes again (over HTTP).
    sim.advance(Duration::from_secs(40));
    assert_eq!(a.pending(), 0);
    let ops = sim.server.ops();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].1, queued[0].id.into());
    assert_eq!(ops[0].2, id);
}

#[test]
fn lost_acknowledgment_is_stored_once() {
    let mut sim = Sim::new();
    // No live channel: pushes go over HTTP.
    sim.server.set_live_refused(true);
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.status(), synced(0));
    sim.server.inject(Endpoint::Push, Fault::DropResponse);
    a.create("once");
    sim.advance(Duration::from_secs(1));
    assert_eq!(sim.server.count(Endpoint::Push), 1);
    assert_eq!(a.pending(), 1);
    sim.advance(Duration::from_secs(120));
    assert_eq!(a.pending(), 0);
    assert_eq!(sim.server.ops().len(), 1);
    assert_eq!(a.status(), synced(0));
}

#[test]
fn polling_mode_pushes_and_pulls_over_http() {
    let mut sim = Sim::new();
    sim.server.set_live_refused(true);
    let a = sim.device();
    let b = sim.device();
    let id = a.create("polled");
    sim.advance(Duration::from_millis(500));
    assert!(sim.server.live_notes().contains(&id));
    sim.advance(Duration::from_secs(30));
    assert_eq!(b.body(id).as_deref(), Some("polled"));
    assert_eq!(sim.server.connections(), 0);
    assert_eq!(b.status(), synced(0));
}

#[test]
fn echo_before_acknowledgment_clears_the_outbox() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.inject(Endpoint::LivePush, Fault::DropResponse);
    a.create("echoed");
    a.clear_events();
    sim.advance(Duration::from_secs(1));
    // The echo acknowledged it; no second push was needed.
    assert_eq!(a.pending(), 0);
    assert_eq!(sim.server.count(Endpoint::LivePush), 1);
    assert_eq!(sim.server.count(Endpoint::Push), 0);
    assert!(a.changes().is_empty());
    // The abandoned push result doesn't cause a retry later either.
    sim.advance(Duration::from_secs(60));
    assert_eq!(sim.server.ops().len(), 1);
}

#[test]
fn key_upload_survives_a_failed_push_and_restart() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.set_reachable(false);
    let id = a.create("new note");
    sim.advance(Duration::from_secs(5));
    let pending_keys = a.replica.lock().storage().unwrap().pending_keys().unwrap();
    assert_eq!(pending_keys.len(), 1);

    let a = sim.restart(a, config());
    sim.server.set_reachable(true);
    sim.advance(Duration::from_secs(70));
    assert_eq!(a.pending(), 0);
    assert!(
        a.replica
            .lock()
            .storage()
            .unwrap()
            .pending_keys()
            .unwrap()
            .is_empty()
    );
    assert!(sim.server.live_notes().contains(&id));
    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.body(id).as_deref(), Some("new note"));
}

#[test]
fn typing_burst_is_coalesced() {
    let mut sim = Sim::new();
    let a = sim.device();
    let id = a.create("");
    sim.advance(Duration::from_secs(3));
    let before = sim.server.count(Endpoint::LivePush) + sim.server.count(Endpoint::Push);
    let mut text = String::new();
    for i in 0..20 {
        text.push(char::from(b'a' + i as u8));
        a.set_body(id, &text);
        sim.advance(Duration::from_millis(25));
    }
    sim.advance(Duration::from_secs(3));
    assert_eq!(a.pending(), 0);
    let pushes = sim.server.count(Endpoint::LivePush) + sim.server.count(Endpoint::Push) - before;
    assert!(pushes < 20, "{pushes} pushes");
    assert!(pushes >= 1);
}

#[test]
fn quota_full_pauses_pushes_but_pulls_continue() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.inject(Endpoint::LivePush, Fault::QuotaExceeded);
    let mine = a.create("over quota");
    sim.advance(Duration::from_secs(1));
    let status = a.status();
    assert_eq!(status.attention, Some(Attention::QuotaFull));
    assert_eq!(status.pending, 1);
    assert_eq!(sim.flush(&a), Err(SyncError::QuotaFull));

    let theirs = b.create("still arrives");
    sim.advance(Duration::from_secs(2));
    assert_eq!(a.body(theirs).as_deref(), Some("still arrives"));
    assert!(!sim.server.live_notes().contains(&mine));

    // Retried after the quota retry interval.
    sim.advance(Duration::from_secs(601));
    assert_eq!(a.pending(), 0);
    assert_eq!(a.status().attention, None);
}

#[test]
fn update_too_large_splits_batches_then_pauses() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.set_max_update_bytes(2000);
    let small = a.create("small");
    let big = a.create(&"x".repeat(4000));
    sim.advance(Duration::from_secs(5));
    // The small note got through on its own; the big one is stuck.
    assert!(sim.server.live_notes().contains(&small));
    assert!(!sim.server.live_notes().contains(&big));
    assert_eq!(a.status().attention, Some(Attention::UpdateTooLarge(big)));
    assert_eq!(a.pending(), 1);
}

#[test]
fn too_large_note_purged_elsewhere_resumes_pushing() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    let big = a.create("big");
    sim.advance(Duration::from_secs(1));
    sim.server.set_max_update_bytes(2000);
    a.set_body(big, &"x".repeat(4000));
    sim.advance(Duration::from_secs(2));
    assert_eq!(a.status().attention, Some(Attention::UpdateTooLarge(big)));

    b.replica.lock().set_trashed(big, true).unwrap();
    sim.advance(Duration::from_secs(1));
    assert_eq!(sim.purge(&b, big), Ok(()));
    let later = a.create("later");
    sim.advance(Duration::from_secs(2));
    assert_eq!(a.body(big), None);
    assert_eq!(a.status().attention, None);
    assert_eq!(a.pending(), 0);
    assert!(sim.server.live_notes().contains(&later));
}

#[test]
fn offline_edit_to_a_note_purged_elsewhere_is_discarded() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    let doomed = a.create("doomed");
    sim.advance(Duration::from_secs(1));
    assert!(b.body(doomed).is_some());

    // b goes offline and edits; a purges.
    b.handle.shutdown();
    sim.run();
    b.set_body(doomed, "doomed, edited offline");
    let kept = b.create("kept");
    a.replica.lock().set_trashed(doomed, true).unwrap();
    sim.advance(Duration::from_secs(1));
    assert_eq!(sim.purge(&a, doomed), Ok(()));

    // b comes back: its catch-up pull sees the tombstone before it pushes.
    let b = sim.restart(b, config());
    sim.advance(Duration::from_secs(3));
    assert_eq!(b.body(doomed), None);
    assert_eq!(b.pending(), 0);
    assert!(sim.server.live_notes().contains(&kept));
    assert!(b.changes().iter().flatten().any(|n| *n == doomed));
}

#[test]
fn push_rejected_as_purged_isolates_and_deletes_the_note() {
    let mut sim = Sim::new();
    let a = sim.device();
    let x = a.create("x");
    let y = a.create("y");
    sim.advance(Duration::from_secs(1));
    // A purge raced the push: the two-note batch is rejected, then x's own.
    sim.server.inject(
        Endpoint::LivePush,
        Fault::Reject(TransportError::NotePurged),
    );
    sim.server.inject(
        Endpoint::LivePush,
        Fault::Reject(TransportError::NotePurged),
    );
    a.set_body(x, "x edited");
    a.set_body(y, "y edited");
    a.clear_events();
    sim.advance(Duration::from_secs(2));
    assert_eq!(a.body(x), None);
    assert_eq!(a.body(y).as_deref(), Some("y edited"));
    assert_eq!(a.pending(), 0);
    assert_eq!(a.changes(), vec![vec![x]]);
}

#[test]
fn unknown_note_on_push_triggers_a_resync() {
    let mut sim = Sim::new();
    let a = sim.device();
    let id = a.create("registered");
    sim.advance(Duration::from_secs(1));
    sim.server.inject(
        Endpoint::LivePush,
        Fault::Reject(TransportError::UnknownNote),
    );
    a.set_body(id, "registered, edited");
    sim.advance(Duration::from_secs(3));
    // Storage was rebuilt and everything re-sent; nothing was lost.
    assert_eq!(a.pending(), 0);
    assert_eq!(a.cursor(), sim.server.head());
    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.body(id).as_deref(), Some("registered, edited"));
}

#[test]
fn conflict_blocks_pushes_with_a_warning() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.inject(
        Endpoint::LivePush,
        Fault::Reject(TransportError::OpIdConflict),
    );
    let id = a.create("conflicted");
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.warnings(), vec![id]);
    assert_eq!(a.pending(), 1);
    assert_eq!(sim.flush(&a), Err(SyncError::PushBlocked));
    sim.advance(Duration::from_secs(60));
    assert_eq!(sim.server.count(Endpoint::LivePush), 1);
}

#[test]
fn conflict_shows_attention_until_the_note_is_purged_elsewhere() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    let id = a.create("shared");
    sim.advance(Duration::from_secs(1));
    sim.server.inject(
        Endpoint::LivePush,
        Fault::Reject(TransportError::OpIdConflict),
    );
    a.set_body(id, "shared, conflicted");
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.status().attention, Some(Attention::PushBlocked));

    b.replica.lock().set_trashed(id, true).unwrap();
    sim.advance(Duration::from_secs(1));
    assert_eq!(sim.purge(&b, id), Ok(()));
    let later = a.create("later");
    sim.advance(Duration::from_secs(2));
    assert_eq!(a.status().attention, None);
    assert_eq!(a.pending(), 0);
    assert!(sim.server.live_notes().contains(&later));
}

#[test]
fn bootstrap_opens_a_compacted_note_split_across_pages() {
    let mut sim = Sim::new();
    let a = sim.device();
    let n1 = a.create("first");
    sim.advance(Duration::from_secs(1));
    let n2 = a.create("second");
    a.set_body(n2, "second, edited");
    sim.advance(Duration::from_secs(1));
    // n1's key stays at its first seq; its snapshot moves to the head.
    sim.compact_on_server(n1);
    sim.server.set_page_groups(1);
    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.cursor(), sim.server.head());
    assert_eq!(b.body(n1).as_deref(), Some("first"));
    assert_eq!(b.notes(), a.notes());
}

#[test]
fn repeated_resync_requests_back_off() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    for _ in 0..50 {
        sim.server.inject(
            Endpoint::LivePush,
            Fault::Reject(TransportError::UnknownNote),
        );
    }
    let connects = sim.server.count(Endpoint::Connect);
    let id = a.create("stuck");
    sim.advance(Duration::from_secs(1));
    // One resync right away, then backoff: not hundreds.
    assert!(sim.server.count(Endpoint::Connect) - connects <= 3);
    sim.advance(Duration::from_secs(120));
    assert!(sim.server.count(Endpoint::Connect) - connects <= 10);
    assert!(a.body(id).is_some());
}

#[test]
fn rate_limited_catch_up_with_pending_edits_retries_without_spinning() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    // The reconnect's catch-up pull is rate limited; the socket stays open.
    sim.server.inject(
        Endpoint::Changes,
        Fault::RateLimited(Duration::from_secs(30)),
    );
    sim.server.close_connections(CLOSE_SHUTDOWN);
    sim.advance(Duration::from_secs(2));
    let id = a.create("x");
    sim.advance(Duration::from_secs(31));
    assert!(sim.server.live_notes().contains(&id));
    assert_eq!(a.status(), synced(0));
}

#[test]
fn quota_full_status_stays_until_a_push_succeeds() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.inject(Endpoint::LivePush, Fault::QuotaExceeded);
    sim.server.inject(Endpoint::LivePush, Fault::QuotaExceeded);
    let id = a.create("full");
    sim.advance(Duration::from_secs(1));
    a.set_body(id, "full, still");
    sim.advance(Duration::from_millis(10));
    assert_eq!(a.status().attention, Some(Attention::QuotaFull));
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.status().attention, Some(Attention::QuotaFull));
    // A later edit's push gets through and clears it.
    a.set_body(id, "room again");
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.status(), synced(0));
}

#[test]
fn auth_close_right_after_connecting_backs_off() {
    let mut sim = Sim::new();
    let _a = sim.device();
    sim.advance(Duration::from_secs(1));
    let connects = sim.server.count(Endpoint::Connect);
    sim.server.close_connections(protocol::CLOSE_AUTH);
    sim.advance(Duration::from_millis(100));
    assert_eq!(sim.server.count(Endpoint::Connect), connects);
    sim.advance(Duration::from_secs(2));
    assert_eq!(sim.server.count(Endpoint::Connect), connects + 1);
}

// ---- 4.4 Live mode ----

#[test]
fn edit_appears_without_polling() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    let id = a.create("live");
    sim.advance(Duration::from_secs(1));
    let polls = sim.server.count(Endpoint::Changes);
    a.set_body(id, "live edit");
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.body(id).as_deref(), Some("live edit"));
    assert_eq!(sim.server.count(Endpoint::Changes), polls);
}

#[test]
fn server_asks_to_resynchronize() {
    let mut sim = Sim::new();
    let a = sim.device();
    // Long enough open that the close doesn't count as a flapping socket.
    sim.advance(Duration::from_secs(11));
    let pulls = sim.server.count(Endpoint::Changes);
    let connects = sim.server.count(Endpoint::Connect);
    sim.server.close_connections(CLOSE_RESYNC);
    sim.advance(Duration::from_millis(100));
    assert!(sim.server.count(Endpoint::Changes) > pulls);
    assert_eq!(sim.server.count(Endpoint::Connect), connects + 1);
    assert_eq!(sim.server.connections(), 1);
    assert_eq!(a.status(), synced(0));
}

#[test]
fn safety_pull_runs_while_live() {
    let mut sim = Sim::new();
    let _a = sim.device();
    sim.advance(Duration::from_secs(1));
    let pulls = sim.server.count(Endpoint::Changes);
    sim.advance(Duration::from_secs(301));
    assert_eq!(sim.server.count(Endpoint::Changes), pulls + 1);
}

// ---- 4.5 Backoff ----

#[test]
fn unreachable_server_backs_off_to_the_cap() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.set_reachable(false);
    let start = sim.server.requests().len();
    sim.advance(Duration::from_secs(600));
    assert_eq!(a.status().state, SyncState::Offline);
    let connects: Vec<u64> = sim.server.requests()[start..]
        .iter()
        .filter(|(_, e)| *e == Endpoint::Connect)
        .map(|(t, _)| *t)
        .collect();
    let gaps: Vec<u64> = connects.windows(2).map(|w| (w[1] - w[0]) / 1000).collect();
    assert!(gaps.len() >= 8, "{gaps:?}");
    assert!(gaps.windows(2).all(|w| w[1] >= w[0]), "{gaps:?}");
    assert!(gaps.iter().all(|g| *g <= 60), "{gaps:?}");
    assert_eq!(*gaps.last().unwrap(), 60);
}

#[test]
fn rate_limit_holds_off_every_request() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.close_connections(CLOSE_SHUTDOWN);
    sim.server.inject(
        Endpoint::Changes,
        Fault::RateLimited(Duration::from_secs(30)),
    );
    let at = sim.timer.now();
    sim.advance(Duration::from_millis(10));
    let limited = sim.server.requests().len();
    sim.advance(Duration::from_secs(29));
    assert_eq!(sim.server.requests().len(), limited);
    sim.advance(Duration::from_secs(40));
    let next = sim.server.requests()[limited].0;
    assert!(
        next >= at + 30_000,
        "next request at {next}, limited at {at}"
    );
    assert_eq!(a.status(), synced(0));
}

#[test]
fn restored_connection_catches_up_and_resets_backoff() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.set_reachable(false);
    let offline = a.create("written offline");
    sim.advance(Duration::from_secs(200));
    assert_eq!(a.status().state, SyncState::Offline);
    sim.server.set_reachable(true);
    let theirs = b.create("from b");
    sim.advance(Duration::from_secs(61));
    // Pulled, pushed, live again.
    assert_eq!(a.body(theirs).as_deref(), Some("from b"));
    assert!(sim.server.live_notes().contains(&offline));
    assert_eq!(a.status(), synced(0));
    // Open for 10 s resets the backoff: the next drop retries after 1 s.
    sim.advance(Duration::from_secs(11));
    let before = sim.server.count(Endpoint::Connect);
    sim.server.drop_connections();
    sim.advance(Duration::from_millis(900));
    assert_eq!(sim.server.count(Endpoint::Connect), before);
    sim.advance(Duration::from_millis(200));
    // Both devices reconnect.
    assert_eq!(sim.server.count(Endpoint::Connect), before + 2);
}

// ---- 4.6 Sessions ----

#[test]
fn expired_access_token_is_refreshed() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.revoke_tokens();
    sim.server.drop_connections();
    let id = a.create("after expiry");
    sim.advance(Duration::from_secs(5));
    assert!(a.session.refreshes() >= 1);
    assert!(sim.server.live_notes().contains(&id));
    assert_eq!(a.status(), synced(0));
}

#[test]
fn live_connection_renews_auth_without_reconnecting() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    let connects = sim.server.count(Endpoint::Connect);
    for _ in 0..4 {
        sim.advance(Duration::from_secs(300));
        sim.server.check_expiry();
    }
    sim.advance(Duration::from_secs(1));
    assert_eq!(sim.server.count(Endpoint::Connect), connects);
    assert_eq!(sim.server.connections(), 1);
    assert!(sim.server.auth_frames()[0] >= 2);
    assert!(a.session.refreshes() >= 1);
}

#[test]
fn revoked_session_requires_sign_in_and_keeps_edits() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    a.session.revoke();
    sim.server.revoke_tokens();
    sim.server.drop_connections();
    let id = a.create("queued");
    sim.advance(Duration::from_secs(10));
    let status = a.status();
    assert_eq!(status.attention, Some(Attention::SignInRequired));
    assert_eq!(status.pending, 1);
    let requests = sim.server.requests().len();
    sim.advance(Duration::from_secs(300));
    assert_eq!(sim.server.requests().len(), requests);

    a.session.sign_in();
    a.handle.set_session();
    sim.advance(Duration::from_secs(2));
    assert!(sim.server.live_notes().contains(&id));
    assert_eq!(a.status(), synced(0));
}

// ---- 4.7 Snapshots ----

fn small_snapshots() -> SyncConfig {
    SyncConfig {
        snapshot_threshold: 5,
        ..config()
    }
}

#[test]
fn snapshot_uploaded_at_threshold_and_log_compacted() {
    let mut sim = Sim::new();
    let a = sim.device_with(small_snapshots());
    let id = a.create("0");
    for i in 1..=6 {
        sim.advance(Duration::from_secs(1));
        a.set_body(id, &i.to_string());
    }
    sim.advance(Duration::from_secs(3));
    assert_eq!(sim.server.count(Endpoint::Snapshot), 1);
    assert!(sim.server.snapshot_covers(id).is_some());
    let stored = a
        .replica
        .lock()
        .storage()
        .unwrap()
        .load_note(id)
        .unwrap()
        .unwrap();
    assert!(stored.base.is_some());
    assert!(stored.entries.len() <= 1);
    assert_eq!(a.body(id).as_deref(), Some("6"));
}

#[test]
fn rejected_snapshot_keeps_the_log_until_more_entries_arrive() {
    let mut sim = Sim::new();
    let a = sim.device_with(small_snapshots());
    // Another device's snapshot got there first.
    sim.server.inject(
        Endpoint::Snapshot,
        Fault::Reject(TransportError::SnapshotStale),
    );
    let id = a.create("0");
    for i in 1..=5 {
        sim.advance(Duration::from_secs(1));
        a.set_body(id, &i.to_string());
    }
    sim.advance(Duration::from_secs(3));
    assert_eq!(sim.server.count(Endpoint::Snapshot), 1);
    let stored = a
        .replica
        .lock()
        .storage()
        .unwrap()
        .load_note(id)
        .unwrap()
        .unwrap();
    assert!(stored.base.is_none());
    assert_eq!(stored.entries.len(), 6);

    // No retry while nothing new arrives...
    sim.advance(Duration::from_secs(600));
    assert_eq!(sim.server.count(Endpoint::Snapshot), 1);
    // ...and more than another threshold's worth of entries tries again.
    for i in 6..=11 {
        a.set_body(id, &i.to_string());
        sim.advance(Duration::from_secs(1));
    }
    sim.advance(Duration::from_secs(1));
    assert_eq!(sim.server.count(Endpoint::Snapshot), 2);
    let stored = a
        .replica
        .lock()
        .storage()
        .unwrap()
        .load_note(id)
        .unwrap()
        .unwrap();
    assert!(stored.base.is_some());
}

#[test]
fn snapshot_does_not_change_content() {
    let mut sim = Sim::new();
    let a = sim.device_with(small_snapshots());
    let id = a.create("start");
    for i in 0..8 {
        sim.advance(Duration::from_secs(1));
        a.set_body(id, &format!("start {i}"));
    }
    a.replica.lock().set_pinned(id, true).unwrap();
    sim.advance(Duration::from_secs(3));
    assert!(sim.server.snapshot_covers(id).is_some());
    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.notes(), a.notes());
}

// ---- 4.8 Purge ----

#[test]
fn purge_propagates() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    let id = a.create("bye");
    sim.advance(Duration::from_secs(1));
    a.replica.lock().set_trashed(id, true).unwrap();
    sim.advance(Duration::from_secs(1));
    b.clear_events();
    assert_eq!(sim.purge(&a, id), Ok(()));
    sim.advance(Duration::from_secs(1));
    for d in [&a, &b] {
        assert_eq!(d.body(id), None);
        assert_eq!(
            d.replica.lock().storage().unwrap().load_note(id).unwrap(),
            None
        );
    }
    assert_eq!(b.changes(), vec![vec![id]]);
}

#[test]
fn purging_a_live_note_fails() {
    let mut sim = Sim::new();
    let a = sim.device();
    let id = a.create("keep");
    sim.advance(Duration::from_secs(1));
    assert_eq!(sim.purge(&a, id), Err(SyncError::NotTrashed(id)));
    assert!(a.body(id).is_some());
    assert_eq!(sim.server.count(Endpoint::Purge), 0);
}

#[test]
fn purge_while_offline_fails_and_changes_nothing() {
    let mut sim = Sim::new();
    let a = sim.device();
    let id = a.create("later");
    sim.advance(Duration::from_secs(1));
    a.replica.lock().set_trashed(id, true).unwrap();
    sim.advance(Duration::from_secs(1));
    sim.server.set_reachable(false);
    assert_eq!(sim.purge(&a, id), Err(SyncError::Offline));
    assert!(a.replica.lock().note(id).unwrap().is_trashed());
    sim.server.set_reachable(true);
    sim.advance(Duration::from_secs(70));
    assert_eq!(sim.purge(&a, id), Ok(()));
    assert_eq!(a.body(id), None);
}

#[test]
fn purge_already_done_elsewhere_succeeds() {
    let mut sim = Sim::new();
    let a = sim.device();
    let id = a.create("gone");
    sim.advance(Duration::from_secs(1));
    a.replica.lock().set_trashed(id, true).unwrap();
    sim.advance(Duration::from_secs(1));
    sim.server
        .inject(Endpoint::Purge, Fault::Reject(TransportError::NotePurged));
    assert_eq!(sim.purge(&a, id), Ok(()));
    assert_eq!(a.body(id), None);
    assert_eq!(a.status().state, SyncState::Synced);
}

#[test]
fn rejected_purge_keeps_the_connection() {
    let mut sim = Sim::new();
    let a = sim.device();
    let id = a.create("kept");
    sim.advance(Duration::from_secs(1));
    a.replica.lock().set_trashed(id, true).unwrap();
    sim.advance(Duration::from_secs(1));
    let connections = sim.server.count(Endpoint::Connect);
    sim.server
        .inject(Endpoint::Purge, Fault::Reject(TransportError::BadRequest));
    assert_eq!(
        sim.purge(&a, id),
        Err(SyncError::Rejected(TransportError::BadRequest))
    );
    sim.advance(Duration::from_secs(1));
    assert!(a.replica.lock().note(id).unwrap().is_trashed());
    assert_eq!(a.status().state, SyncState::Synced);
    assert_eq!(sim.server.count(Endpoint::Connect), connections);
}

#[test]
fn never_synced_note_is_purged_locally() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.inject(Endpoint::LivePush, Fault::QuotaExceeded);
    let id = a.create("never pushed");
    a.replica.lock().set_trashed(id, true).unwrap();
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.pending(), 2);
    assert_eq!(sim.purge(&a, id), Ok(()));
    assert_eq!(a.body(id), None);
    assert_eq!(a.pending(), 0);
}

// ---- 4.9 Resync ----

#[test]
fn server_restored_from_backup_gets_notes_back() {
    let mut sim = Sim::new();
    let a = sim.device();
    let old = a.create("before backup");
    sim.advance(Duration::from_secs(1));
    let backup = sim.server.backup();
    let new = a.create("after backup");
    a.set_body(old, "before backup, edited after");
    sim.advance(Duration::from_secs(1));
    assert_eq!(a.pending(), 0);

    sim.server.restore(backup);
    // A pull now finds a's cursor ahead of the server.
    sim.advance(Duration::from_secs(70));
    assert!(sim.server.live_notes().contains(&new));
    assert_eq!(a.pending(), 0);
    assert_eq!(a.cursor(), sim.server.head());

    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    assert_eq!(b.body(new).as_deref(), Some("after backup"));
    // The edit the restored server lost is back too.
    assert_eq!(b.body(old).as_deref(), Some("before backup, edited after"));
    assert_eq!(b.notes(), a.notes());
}

// ---- 4.10 Status, notifications, flush ----

#[test]
fn offline_with_pending_edits() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.set_reachable(false);
    let id = a.create("one");
    a.set_body(id, "one two");
    a.set_body(id, "one two three");
    sim.advance(Duration::from_secs(5));
    let status = a.status();
    assert_eq!(status.state, SyncState::Offline);
    assert_eq!(status.pending, 3);

    sim.server.set_reachable(true);
    sim.advance(Duration::from_secs(70));
    assert_eq!(a.status(), synced(0));
}

#[test]
fn one_pull_reports_both_notes_together() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    let x = a.create("x");
    let y = a.create("y");
    sim.advance(Duration::from_secs(1));
    b.handle.shutdown();
    sim.run();
    a.set_body(x, "x2");
    a.set_body(y, "y2");
    sim.advance(Duration::from_secs(1));
    let b = sim.restart(b, config());
    sim.advance(Duration::from_secs(1));
    let mut both = vec![x, y];
    both.sort();
    assert_eq!(b.changes(), vec![both]);
}

#[test]
fn local_edits_are_not_reported_as_changes() {
    let mut sim = Sim::new();
    let a = sim.device();
    let id = a.create("mine");
    a.set_body(id, "mine, edited");
    sim.advance(Duration::from_secs(5));
    assert!(a.changes().is_empty());
}

#[test]
fn flush_pushes_everything() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    let id = a.create("one");
    a.set_body(id, "one two");
    assert_eq!(a.pending(), 2);
    assert_eq!(sim.flush(&a), Ok(()));
    assert_eq!(a.pending(), 0);
}

#[test]
fn flush_while_offline_fails_and_keeps_updates() {
    let mut sim = Sim::new();
    let a = sim.device();
    sim.advance(Duration::from_secs(1));
    sim.server.set_reachable(false);
    a.create("stuck");
    assert_eq!(sim.flush(&a), Err(SyncError::Offline));
    assert_eq!(a.pending(), 1);
}

#[test]
fn tampered_update_raises_a_warning_and_sync_continues() {
    let mut sim = Sim::new();
    let a = sim.device();
    let b = sim.device();
    sim.advance(Duration::from_secs(1));
    // a writes a bad update straight into its outbox, then a good note.
    let bad = a.create("bad");
    sim.advance(Duration::from_secs(1));
    {
        let mut replica = a.replica.lock();
        let store = replica.storage_mut().unwrap();
        let mut bytes = store.load_note(bad).unwrap().unwrap().entries[0]
            .bytes
            .clone();
        bytes[30] ^= 1;
        store
            .append_local(
                bad,
                crate::storage::LocalUpdate {
                    id: crate::storage::UpdateId([9; 16]),
                    bytes,
                },
            )
            .unwrap();
    }
    a.handle.wake();
    let good = a.create("good");
    sim.advance(Duration::from_secs(2));
    assert_eq!(b.body(good).as_deref(), Some("good"));
    assert_eq!(b.warnings(), vec![bad]);
    assert_eq!(b.status().integrity_warnings, 1);
    assert_eq!(b.status().state, SyncState::Synced);
}

// ---- 4.11 Convergence ----

/// xorshift64*, seeded, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn three_clients_converge_under_faults() {
    let mut sim = Sim::new();
    let devices = [sim.device(), sim.device(), sim.device()];
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let mut notes: Vec<NoteId> = Vec::new();
    for round in 0..300 {
        let d = &devices[rng.below(3)];
        match rng.below(10) {
            0 => notes.push(d.create(&format!("note {round}"))),
            1..=5 if !notes.is_empty() => {
                let id = notes[rng.below(notes.len())];
                if let Some(body) = d.body(id) {
                    let mut chars: Vec<char> = body.chars().collect();
                    let at = rng.below(chars.len() + 1);
                    chars.insert(at, char::from(b'a' + rng.below(26) as u8));
                    let body: String = chars.into_iter().collect();
                    d.set_body(id, &body);
                }
            }
            6 => sim.server.drop_connections(),
            7 => {
                let endpoint = if rng.below(2) == 0 {
                    Endpoint::Push
                } else {
                    Endpoint::LivePush
                };
                sim.server.inject(endpoint, Fault::DropResponse);
            }
            8 => sim.server.redeliver_recent(1 + rng.below(4)),
            9 if !notes.is_empty() => sim.compact_on_server(notes[rng.below(notes.len())]),
            _ => {}
        }
        sim.advance(Duration::from_millis(rng.below(3000) as u64));
    }
    sim.advance(Duration::from_secs(120));
    for d in &devices {
        assert_eq!(sim.flush(d), Ok(()));
    }
    sim.advance(Duration::from_secs(5));
    let expect = devices[0].notes();
    assert!(!expect.is_empty());
    for d in &devices {
        assert_eq!(d.pending(), 0);
        assert_eq!(d.notes(), expect);
    }
    let bodies: BTreeMap<NoteId, String> = expect
        .iter()
        .map(|n| (n.id(), n.body().to_owned()))
        .collect();
    assert_eq!(bodies.len(), notes.len());
}
