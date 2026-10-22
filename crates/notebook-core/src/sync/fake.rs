//! Test doubles: an in-memory server that follows the `sync-server` design,
//! a manual clock, and a scripted session, plus fault injection. The whole
//! engine runs against these deterministically, on one thread.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc;

use super::session::{AccessToken, RefreshError, SessionSource};
use super::timer::Timer;
use super::transport::{LiveChannel, LiveEvent, Transport, TransportError};
use crate::auth::api::Token;
use crate::crypto::{AuthSecret, KeyBundle, RecoveryKey, VaultId, VaultKey};
use crate::note::NoteId;
use crate::protocol::{
    Bytes, CLOSE_AUTH, CLOSE_TOO_MANY, ChangesResponse, ClientFrame, Entry, MAX_NOTE_KEY_BYTES,
    MAX_PUSH_UPDATES, NoteEntry, OpId, PushRequest, PushResponse, PushUpdate, Snapshot,
};

/// `N` copies of one vault key for `vault`. Every call on the same thread
/// returns the same key, so a test can reopen a store or start a second
/// device of the vault.
pub fn vault_keys<const N: usize>(vault: VaultId) -> [VaultKey; N] {
    thread_local! {
        static BUNDLES: RefCell<HashMap<VaultId, KeyBundle>> = RefCell::new(HashMap::new());
    }
    let auth = AuthSecret::from_bytes(&[7; 32]).expect("valid auth secret");
    BUNDLES.with(|bundles| {
        let mut bundles = bundles.borrow_mut();
        let bundle = bundles.entry(vault).or_insert_with(|| {
            let key = VaultKey::generate().expect("random source");
            let recovery = RecoveryKey::generate().expect("random source");
            KeyBundle::create(vault, &key, &auth, &recovery).expect("bundle")
        });
        std::array::from_fn(|_| bundle.unlock_with_auth(&auth).expect("unlock"))
    })
}

// ---- Manual timer ----

#[derive(Default)]
struct Clock {
    now: u64,
    next_id: u64,
    sleepers: BTreeMap<u64, (u64, Option<Waker>)>,
}

/// A clock that moves only when the test says so.
#[derive(Clone, Default)]
pub struct ManualTimer(Rc<RefCell<Clock>>);

impl ManualTimer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Moves the clock forward by `by`, waking every sleep that is due.
    pub fn advance(&self, by: Duration) {
        let to = self.now() + duration_ms(by);
        self.set(to);
    }

    /// Moves the clock to `to` (never backwards), waking every sleep that is
    /// due.
    pub fn set(&self, to: u64) {
        let wakers: Vec<Waker> = {
            let mut clock = self.0.borrow_mut();
            clock.now = clock.now.max(to);
            let now = clock.now;
            clock
                .sleepers
                .values_mut()
                .filter(|(deadline, _)| *deadline <= now)
                .filter_map(|(_, waker)| waker.take())
                .collect()
        };
        wakers.into_iter().for_each(Waker::wake);
    }

    /// The earliest deadline of a pending sleep.
    pub fn next_deadline(&self) -> Option<u64> {
        let clock = self.0.borrow();
        clock
            .sleepers
            .values()
            .map(|(deadline, _)| *deadline)
            .filter(|d| *d > clock.now)
            .min()
    }
}

impl Timer for ManualTimer {
    fn now(&self) -> u64 {
        self.0.borrow().now
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        let mut clock = self.0.borrow_mut();
        let id = clock.next_id;
        clock.next_id += 1;
        let deadline = clock.now.saturating_add(duration_ms(duration));
        clock.sleepers.insert(id, (deadline, None));
        Sleep {
            clock: self.0.clone(),
            id,
            deadline,
        }
    }
}

struct Sleep {
    clock: Rc<RefCell<Clock>>,
    id: u64,
    deadline: u64,
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut clock = self.clock.borrow_mut();
        if clock.now >= self.deadline {
            return Poll::Ready(());
        }
        if let Some(entry) = clock.sleepers.get_mut(&self.id) {
            entry.1 = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        self.clock.borrow_mut().sleepers.remove(&self.id);
    }
}

fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

// ---- Fake server ----

/// A request the fake server saw, for assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Push,
    Changes,
    Snapshot,
    Purge,
    Connect,
    /// A `push` frame on the live channel.
    LivePush,
}

/// A one-shot failure for the next request to an endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// Fail with a network error before the server does anything.
    DropRequest,
    /// Do the work, then fail with a network error (for a live push: send
    /// no `push_result`).
    DropResponse,
    Unauthorized,
    RateLimited(Duration),
    /// The body limit's `413`.
    TooLarge,
    QuotaExceeded,
    /// Reject with this error without doing anything.
    Reject(TransportError),
}

#[derive(Debug, Clone)]
struct Op {
    id: OpId,
    note: NoteId,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ServerNote {
    key: Option<Vec<u8>>,
    created_seq: u64,
    purged_seq: Option<u64>,
    snapshot: Option<(u64, Vec<u8>)>,
}

/// One note's stored data, from [`FakeServer::note_data`].
#[derive(Debug, Clone, Default)]
pub struct NoteData {
    pub key: Option<Vec<u8>>,
    pub snapshot: Option<Vec<u8>>,
    /// Remaining updates in sequence order.
    pub ops: Vec<Vec<u8>>,
}

/// The server's durable state: what a backup holds.
#[derive(Debug, Clone, Default)]
pub struct Backup {
    head: u64,
    ops: BTreeMap<u64, Op>,
    notes: BTreeMap<NoteId, ServerNote>,
}

struct Conn {
    tx: mpsc::UnboundedSender<LiveEvent>,
    opened_at: u64,
    /// Token expiry once authenticated.
    authed_until: Option<u64>,
    auth_frames: usize,
}

struct State {
    data: Backup,
    timer: ManualTimer,
    reachable: bool,
    live_refused: bool,
    /// Each fault with how many matching requests to let through first.
    faults: VecDeque<(Endpoint, usize, Fault)>,
    tokens: HashMap<String, u64>,
    next_token: u64,
    conns: BTreeMap<u64, Conn>,
    next_conn: u64,
    requests: Vec<(u64, Endpoint)>,
    page_groups: usize,
    max_update_bytes: usize,
    max_body_bytes: usize,
}

/// An in-memory `/sync/v1` server for one vault. Clones share the server;
/// each engine under test uses a clone as its [`Transport`].
#[derive(Clone)]
pub struct FakeServer(Rc<RefCell<State>>);

impl FakeServer {
    pub fn new(timer: ManualTimer) -> Self {
        Self(Rc::new(RefCell::new(State {
            data: Backup::default(),
            timer,
            reachable: true,
            live_refused: false,
            faults: VecDeque::new(),
            tokens: HashMap::new(),
            next_token: 0,
            conns: BTreeMap::new(),
            next_conn: 0,
            requests: Vec::new(),
            page_groups: 1000,
            max_update_bytes: 256 * 1024,
            max_body_bytes: 1024 * 1024,
        })))
    }

    /// Limits pages to `groups` sequence-number groups, to exercise paging.
    pub fn set_page_groups(&self, groups: usize) {
        self.0.borrow_mut().page_groups = groups.max(1);
    }

    pub fn set_max_update_bytes(&self, bytes: usize) {
        self.0.borrow_mut().max_update_bytes = bytes;
    }

    /// While unreachable, every request fails with a network error and live
    /// connections drop.
    pub fn set_reachable(&self, reachable: bool) {
        self.0.borrow_mut().reachable = reachable;
        if !reachable {
            self.drop_connections();
        }
    }

    /// Queues `fault` for the next request to `endpoint`.
    pub fn inject(&self, endpoint: Endpoint, fault: Fault) {
        self.inject_nth(endpoint, 0, fault);
    }

    /// Queues `fault` for the request to `endpoint` after the next `skip`.
    pub fn inject_nth(&self, endpoint: Endpoint, skip: usize, fault: Fault) {
        self.0
            .borrow_mut()
            .faults
            .push_back((endpoint, skip, fault));
    }

    /// While refused, live connections are closed with `4003` (too many
    /// connections) right after authenticating, so clients poll.
    pub fn set_live_refused(&self, refused: bool) {
        self.0.borrow_mut().live_refused = refused;
    }

    /// Every request so far, with the clock time it arrived.
    pub fn requests(&self) -> Vec<(u64, Endpoint)> {
        self.0.borrow().requests.clone()
    }

    pub fn count(&self, endpoint: Endpoint) -> usize {
        let state = self.0.borrow();
        state
            .requests
            .iter()
            .filter(|(_, e)| *e == endpoint)
            .count()
    }

    /// A new access token that expires `ttl` from now.
    pub fn issue_token(&self, ttl: Duration) -> AccessToken {
        let mut state = self.0.borrow_mut();
        state.next_token += 1;
        let token = format!("token-{}", state.next_token);
        let expires_at = state.timer.now() + duration_ms(ttl);
        state.tokens.insert(token.clone(), expires_at);
        AccessToken {
            token: Token(token),
            expires_at,
        }
    }

    pub fn revoke_tokens(&self) {
        self.0.borrow_mut().tokens.clear();
    }

    pub fn head(&self) -> u64 {
        self.0.borrow().data.head
    }

    /// Stored updates as `(seq, op ID, note)`.
    pub fn ops(&self) -> Vec<(u64, OpId, NoteId)> {
        let state = self.0.borrow();
        state
            .data
            .ops
            .iter()
            .map(|(seq, op)| (*seq, op.id, op.note))
            .collect()
    }

    /// The stored snapshot's `covers` for `note`.
    pub fn snapshot_covers(&self, note: NoteId) -> Option<u64> {
        let state = self.0.borrow();
        state.data.notes.get(&note)?.snapshot.as_ref().map(|s| s.0)
    }

    /// Notes registered and not purged.
    pub fn live_notes(&self) -> BTreeSet<NoteId> {
        let state = self.0.borrow();
        state
            .data
            .notes
            .iter()
            .filter(|(_, n)| n.purged_seq.is_none())
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn backup(&self) -> Backup {
        self.0.borrow().data.clone()
    }

    /// Replaces the state with `backup`, as an operator restoring it would.
    /// The restart drops every live connection.
    pub fn restore(&self, backup: Backup) {
        self.0.borrow_mut().data = backup;
        self.drop_connections();
    }

    /// Open live connections (authenticated or not).
    pub fn connections(&self) -> usize {
        self.0.borrow().conns.len()
    }

    /// `auth` frames received on each open connection.
    pub fn auth_frames(&self) -> Vec<usize> {
        let state = self.0.borrow();
        state.conns.values().map(|c| c.auth_frames).collect()
    }

    /// Fails every live connection, as a network drop would.
    pub fn drop_connections(&self) {
        let conns = std::mem::take(&mut self.0.borrow_mut().conns);
        for conn in conns.into_values() {
            let _ = conn
                .tx
                .unbounded_send(LiveEvent::Error(TransportError::Network("dropped".into())));
        }
    }

    /// Sends the last `n` stored updates to every live connection again, as
    /// duplicate deliveries.
    pub fn redeliver_recent(&self, n: usize) {
        let entries: Vec<Entry> = {
            let state = self.0.borrow();
            state
                .data
                .ops
                .iter()
                .rev()
                .take(n)
                .map(|(seq, op)| Entry::Update {
                    seq: *seq,
                    id: op.id,
                    note: op.note,
                    data: Bytes(op.data.clone()),
                    note_key: None,
                })
                .collect()
        };
        for entry in entries.into_iter().rev() {
            self.broadcast(entry);
        }
    }

    /// Closes every live connection with `code`.
    pub fn close_connections(&self, code: u16) {
        let conns = std::mem::take(&mut self.0.borrow_mut().conns);
        for conn in conns.into_values() {
            let _ = conn.tx.unbounded_send(LiveEvent::Closed(Some(code)));
        }
    }

    /// Closes connections whose token expired, or that sent no `auth` frame
    /// within 10 seconds, with `4001`. The real server does this on its own
    /// timers; tests call it after moving the clock.
    pub fn check_expiry(&self) {
        let mut state = self.0.borrow_mut();
        let now = state.timer.now();
        let expired: Vec<u64> = state
            .conns
            .iter()
            .filter(|(_, c)| match c.authed_until {
                Some(until) => until <= now,
                None => c.opened_at + 10_000 <= now,
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(conn) = state.conns.remove(&id) {
                let _ = conn.tx.unbounded_send(LiveEvent::Closed(Some(CLOSE_AUTH)));
            }
        }
    }

    /// Stores a snapshot directly, as another client would upload it.
    pub fn put_snapshot(
        &self,
        note: NoteId,
        covers: u64,
        data: Vec<u8>,
    ) -> Result<(), TransportError> {
        self.0.borrow_mut().data.snapshot(note, covers, data)
    }

    /// Everything stored for `note`, so a test holding the vault key can
    /// build a snapshot to force compaction.
    pub fn note_data(&self, note: NoteId) -> NoteData {
        let state = self.0.borrow();
        let Some(n) = state.data.notes.get(&note) else {
            return NoteData::default();
        };
        let ops = state
            .data
            .ops
            .values()
            .filter(|op| op.note == note)
            .map(|op| op.data.clone())
            .collect();
        NoteData {
            key: n.key.clone(),
            snapshot: n.snapshot.as_ref().map(|s| s.1.clone()),
            ops,
        }
    }

    /// Records the request and applies reachability, token, and injected
    /// faults. Returns the fault to apply after doing the work, if any.
    fn begin(
        &self,
        endpoint: Endpoint,
        token: Option<&Token>,
    ) -> Result<Option<Fault>, TransportError> {
        let mut state = self.0.borrow_mut();
        let now = state.timer.now();
        state.requests.push((now, endpoint));
        if !state.reachable {
            return Err(TransportError::Network("unreachable".into()));
        }
        // Only the first fault queued for this endpoint counts down.
        let fault = match state.faults.iter().position(|(e, _, _)| *e == endpoint) {
            Some(i) if state.faults[i].1 > 0 => {
                state.faults[i].1 -= 1;
                None
            }
            Some(i) => state.faults.remove(i).map(|(_, _, f)| f),
            None => None,
        };
        match fault {
            Some(Fault::DropRequest) => return Err(TransportError::Network("dropped".into())),
            Some(Fault::Unauthorized) => return Err(TransportError::InvalidToken),
            Some(Fault::RateLimited(after)) => {
                return Err(TransportError::RateLimited {
                    retry_after: Some(after),
                });
            }
            Some(Fault::TooLarge) => return Err(TransportError::UpdateTooLarge),
            Some(Fault::QuotaExceeded) => return Err(TransportError::QuotaExceeded),
            Some(Fault::Reject(err)) => return Err(err),
            _ => {}
        }
        if let Some(token) = token
            && !state.tokens.get(&token.0).is_some_and(|exp| *exp > now)
        {
            return Err(TransportError::InvalidToken);
        }
        Ok(fault)
    }

    fn do_push(&self, updates: &[PushUpdate]) -> Result<PushResponse, TransportError> {
        let entries;
        let response;
        {
            let mut state = self.0.borrow_mut();
            let body: usize = updates.iter().map(|u| u.data.0.len() * 4 / 3 + 100).sum();
            if body > state.max_body_bytes {
                return Err(TransportError::UpdateTooLarge);
            }
            let max = state.max_update_bytes;
            // All or nothing: work on a copy and keep it only on success.
            let mut data = state.data.clone();
            let (resp, new) = data.push(updates, max)?;
            state.data = data;
            entries = new;
            response = resp;
        }
        for entry in entries {
            self.broadcast(entry);
        }
        Ok(response)
    }

    fn broadcast(&self, entry: Entry) {
        let state = self.0.borrow();
        for conn in state.conns.values().filter(|c| c.authed_until.is_some()) {
            let _ = conn.tx.unbounded_send(LiveEvent::Entry(entry.clone()));
        }
    }

    fn send_to(&self, conn: u64, event: LiveEvent) {
        if let Some(c) = self.0.borrow().conns.get(&conn) {
            let _ = c.tx.unbounded_send(event);
        }
    }

    fn close(&self, conn: u64, code: u16) {
        if let Some(c) = self.0.borrow_mut().conns.remove(&conn) {
            let _ = c.tx.unbounded_send(LiveEvent::Closed(Some(code)));
        }
    }

    fn handle_frame(&self, conn: u64, frame: &ClientFrame) -> Result<(), TransportError> {
        if !self.0.borrow().reachable {
            return Err(TransportError::Network("unreachable".into()));
        }
        if !self.0.borrow().conns.contains_key(&conn) {
            return Err(TransportError::Network("connection closed".into()));
        }
        match frame {
            ClientFrame::Auth { token } => {
                let (expiry, first) = {
                    let mut state = self.0.borrow_mut();
                    let now = state.timer.now();
                    let expiry = state.tokens.get(&token.0).copied().filter(|e| *e > now);
                    let c = state.conns.get_mut(&conn).expect("checked above");
                    c.auth_frames += 1;
                    let first = c.authed_until.is_none();
                    if let Some(expiry) = expiry {
                        c.authed_until = Some(expiry);
                    }
                    (expiry, first)
                };
                match expiry {
                    None => self.close(conn, CLOSE_AUTH),
                    Some(_) if self.0.borrow().live_refused => self.close(conn, CLOSE_TOO_MANY),
                    Some(_) if first => {
                        let head = self.head();
                        self.send_to(conn, LiveEvent::Ready { head });
                    }
                    Some(_) => {}
                }
            }
            ClientFrame::Push {
                request_id,
                updates,
            } => {
                if self.0.borrow().conns[&conn].authed_until.is_none() {
                    self.close(conn, CLOSE_AUTH);
                    return Ok(());
                }
                let fault = match self.begin(Endpoint::LivePush, None) {
                    Ok(fault) => fault,
                    Err(TransportError::Network(_)) => return Ok(()),
                    Err(err) => {
                        self.send_to(
                            conn,
                            LiveEvent::PushResult {
                                request_id: request_id.clone(),
                                result: Err(err),
                            },
                        );
                        return Ok(());
                    }
                };
                let result = self.do_push(updates);
                if fault != Some(Fault::DropResponse) {
                    self.send_to(
                        conn,
                        LiveEvent::PushResult {
                            request_id: request_id.clone(),
                            result,
                        },
                    );
                }
            }
            ClientFrame::Unknown => {}
        }
        Ok(())
    }
}

impl Backup {
    fn push(
        &mut self,
        updates: &[PushUpdate],
        max_update_bytes: usize,
    ) -> Result<(PushResponse, Vec<Entry>), TransportError> {
        if updates.is_empty() || updates.len() > MAX_PUSH_UPDATES {
            return Err(TransportError::BadRequest);
        }
        if updates
            .iter()
            .filter_map(|u| u.note_key.as_ref())
            .any(|k| k.0.len() > MAX_NOTE_KEY_BYTES)
        {
            return Err(TransportError::BadRequest);
        }
        if updates.iter().any(|u| u.data.0.len() > max_update_bytes) {
            return Err(TransportError::UpdateTooLarge);
        }
        let ids: BTreeSet<OpId> = updates.iter().map(|u| u.id).collect();
        if ids.len() != updates.len() {
            return Err(TransportError::BadRequest);
        }
        let mut seqs = Vec::new();
        let mut entries = Vec::new();
        for update in updates {
            let registers = match self.notes.get(&update.note) {
                Some(n) if n.purged_seq.is_some() => return Err(TransportError::NotePurged),
                Some(n) => {
                    if let Some(key) = &update.note_key
                        && n.key.as_deref() != Some(key.0.as_slice())
                    {
                        return Err(TransportError::NoteKeyConflict);
                    }
                    false
                }
                None if update.note_key.is_none() => return Err(TransportError::MissingNoteKey),
                None => true,
            };
            if let Some((seq, op)) = self.ops.iter().find(|(_, op)| op.id == update.id) {
                if op.note != update.note || op.data != update.data.0 {
                    return Err(TransportError::OpIdConflict);
                }
                seqs.push(*seq);
                continue;
            }
            self.head += 1;
            let seq = self.head;
            if registers {
                self.notes.insert(
                    update.note,
                    ServerNote {
                        key: update.note_key.as_ref().map(|k| k.0.clone()),
                        created_seq: seq,
                        purged_seq: None,
                        snapshot: None,
                    },
                );
            }
            self.ops.insert(
                seq,
                Op {
                    id: update.id,
                    note: update.note,
                    data: update.data.0.clone(),
                },
            );
            seqs.push(seq);
            entries.push(Entry::Update {
                seq,
                id: update.id,
                note: update.note,
                data: update.data.clone(),
                note_key: if registers {
                    update.note_key.clone()
                } else {
                    None
                },
            });
        }
        Ok((
            PushResponse {
                seqs,
                head: self.head,
            },
            entries,
        ))
    }

    fn snapshot(&mut self, note: NoteId, covers: u64, data: Vec<u8>) -> Result<(), TransportError> {
        let head = self.head;
        let n = self
            .notes
            .get_mut(&note)
            .ok_or(TransportError::UnknownNote)?;
        if n.purged_seq.is_some() {
            return Err(TransportError::NotePurged);
        }
        if covers > head || covers < n.created_seq {
            return Err(TransportError::BadRequest);
        }
        if n.snapshot.as_ref().is_some_and(|(c, _)| *c >= covers) {
            return Err(TransportError::SnapshotStale);
        }
        n.snapshot = Some((covers, data));
        self.ops.retain(|seq, op| op.note != note || *seq > covers);
        Ok(())
    }

    fn purge(&mut self, note: NoteId) -> Result<Option<Entry>, TransportError> {
        let head = self.head;
        let n = self
            .notes
            .get_mut(&note)
            .ok_or(TransportError::UnknownNote)?;
        if n.purged_seq.is_some() {
            return Ok(None);
        }
        let seq = head + 1;
        n.purged_seq = Some(seq);
        n.key = None;
        n.snapshot = None;
        self.ops.retain(|_, op| op.note != note);
        self.head = seq;
        Ok(Some(Entry::Purge { seq, note }))
    }

    fn changes(&self, after: u64, page_groups: usize) -> Result<ChangesResponse, TransportError> {
        if after > self.head {
            return Err(TransportError::CursorAhead);
        }
        let mut keys: BTreeSet<u64> = self.ops.range(after + 1..).map(|(seq, _)| *seq).collect();
        for n in self.notes.values() {
            if n.key.is_some() && n.created_seq > after {
                keys.insert(n.created_seq);
            }
            if let Some((covers, _)) = &n.snapshot
                && *covers > after
            {
                keys.insert(*covers);
            }
            if let Some(purged) = n.purged_seq
                && purged > after
            {
                keys.insert(purged);
            }
        }
        let Some(&end) = keys.iter().take(page_groups).next_back() else {
            return Ok(ChangesResponse {
                notes: Vec::new(),
                entries: Vec::new(),
                cursor: self.head,
                more: false,
            });
        };
        let more = keys.len() > page_groups;
        let in_page = |seq: u64| seq > after && seq <= end;

        let mut entries: Vec<Entry> = self
            .ops
            .range(after + 1..=end)
            .map(|(seq, op)| Entry::Update {
                seq: *seq,
                id: op.id,
                note: op.note,
                data: Bytes(op.data.clone()),
                note_key: None,
            })
            .collect();
        let mut notes: BTreeMap<(u64, NoteId), NoteEntry> = BTreeMap::new();
        for (id, n) in &self.notes {
            if let Some(purged) = n.purged_seq.filter(|s| in_page(*s)) {
                entries.push(Entry::Purge {
                    seq: purged,
                    note: *id,
                });
            }
            let key = n.key.as_ref().filter(|_| in_page(n.created_seq));
            let snapshot = n.snapshot.as_ref().filter(|(c, _)| in_page(*c));
            if key.is_none() && snapshot.is_none() {
                continue;
            }
            let first = if key.is_some() {
                n.created_seq
            } else {
                snapshot.map_or(0, |s| s.0)
            };
            notes.insert(
                (first, *id),
                NoteEntry {
                    note: *id,
                    key: key.map(|k| Bytes(k.clone())),
                    snapshot: snapshot.map(|(covers, data)| Snapshot {
                        covers: *covers,
                        data: Bytes(data.clone()),
                    }),
                },
            );
        }
        entries.sort_by_key(Entry::seq);
        Ok(ChangesResponse {
            notes: notes.into_values().collect(),
            entries,
            cursor: if more { end } else { self.head },
            more,
        })
    }
}

impl Transport for FakeServer {
    type Live = FakeLive;

    async fn push(
        &self,
        token: &Token,
        request: &PushRequest,
    ) -> Result<PushResponse, TransportError> {
        let fault = self.begin(Endpoint::Push, Some(token))?;
        let result = self.do_push(&request.updates);
        if fault == Some(Fault::DropResponse) {
            return Err(TransportError::Network("response lost".into()));
        }
        result
    }

    async fn changes(&self, token: &Token, after: u64) -> Result<ChangesResponse, TransportError> {
        let fault = self.begin(Endpoint::Changes, Some(token))?;
        let result = {
            let state = self.0.borrow();
            state.data.changes(after, state.page_groups)
        };
        if fault == Some(Fault::DropResponse) {
            return Err(TransportError::Network("response lost".into()));
        }
        result
    }

    async fn upload_snapshot(
        &self,
        token: &Token,
        note: NoteId,
        covers: u64,
        data: &[u8],
    ) -> Result<(), TransportError> {
        let fault = self.begin(Endpoint::Snapshot, Some(token))?;
        let result = self.put_snapshot(note, covers, data.to_vec());
        if fault == Some(Fault::DropResponse) {
            return Err(TransportError::Network("response lost".into()));
        }
        result
    }

    async fn purge(&self, token: &Token, note: NoteId) -> Result<(), TransportError> {
        let fault = self.begin(Endpoint::Purge, Some(token))?;
        let entry = self.0.borrow_mut().data.purge(note)?;
        if let Some(entry) = entry {
            self.broadcast(entry);
        }
        if fault == Some(Fault::DropResponse) {
            return Err(TransportError::Network("response lost".into()));
        }
        Ok(())
    }

    async fn connect_live(&self) -> Result<FakeLive, TransportError> {
        self.begin(Endpoint::Connect, None)?;
        let (tx, rx) = mpsc::unbounded();
        let mut state = self.0.borrow_mut();
        let id = state.next_conn;
        state.next_conn += 1;
        let opened_at = state.timer.now();
        state.conns.insert(
            id,
            Conn {
                tx,
                opened_at,
                authed_until: None,
                auth_frames: 0,
            },
        );
        Ok(FakeLive {
            server: self.clone(),
            id,
            rx,
            ended: None,
        })
    }
}

/// A live connection to a [`FakeServer`].
pub struct FakeLive {
    server: FakeServer,
    id: u64,
    rx: mpsc::UnboundedReceiver<LiveEvent>,
    ended: Option<LiveEvent>,
}

impl LiveChannel for FakeLive {
    async fn next(&mut self) -> LiveEvent {
        if let Some(ended) = &self.ended {
            return ended.clone();
        }
        let event = self.rx.next().await.unwrap_or(LiveEvent::Closed(None));
        if matches!(event, LiveEvent::Closed(_) | LiveEvent::Error(_)) {
            self.ended = Some(event.clone());
        }
        event
    }

    async fn send(&mut self, frame: &ClientFrame) -> Result<(), TransportError> {
        if self.ended.is_some() {
            return Err(TransportError::Network("connection closed".into()));
        }
        self.server.handle_frame(self.id, frame)
    }
}

impl Drop for FakeLive {
    fn drop(&mut self) {
        if let Ok(mut state) = self.server.0.try_borrow_mut() {
            state.conns.remove(&self.id);
        }
    }
}

// ---- Fake session ----

struct SessionState {
    token: Option<AccessToken>,
    ttl: Duration,
    revoked: bool,
    unavailable: bool,
    refreshes: usize,
}

/// A session whose refreshes get tokens from a [`FakeServer`].
#[derive(Clone)]
pub struct FakeSession {
    server: FakeServer,
    state: Rc<RefCell<SessionState>>,
}

impl FakeSession {
    /// Signed in with a token that lives `ttl`; refreshes issue the same.
    pub fn new(server: &FakeServer, ttl: Duration) -> Self {
        let token = server.issue_token(ttl);
        Self {
            server: server.clone(),
            state: Rc::new(RefCell::new(SessionState {
                token: Some(token),
                ttl,
                revoked: false,
                unavailable: false,
                refreshes: 0,
            })),
        }
    }

    /// Makes refreshes fail as if the session had been revoked.
    pub fn revoke(&self) {
        self.state.borrow_mut().revoked = true;
    }

    /// Makes refreshes fail transiently.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.state.borrow_mut().unavailable = unavailable;
    }

    /// Signs in again: a fresh token, and refreshes work again.
    pub fn sign_in(&self) {
        let ttl = self.state.borrow().ttl;
        let token = self.server.issue_token(ttl);
        let mut state = self.state.borrow_mut();
        state.revoked = false;
        state.token = Some(token);
    }

    pub fn refreshes(&self) -> usize {
        self.state.borrow().refreshes
    }
}

impl SessionSource for FakeSession {
    fn access_token(&self) -> Option<AccessToken> {
        self.state.borrow().token.clone()
    }

    async fn refresh(&self) -> Result<(), RefreshError> {
        let (revoked, unavailable, ttl) = {
            let state = self.state.borrow();
            (state.revoked, state.unavailable, state.ttl)
        };
        if revoked {
            return Err(RefreshError::SessionEnded);
        }
        if unavailable {
            return Err(RefreshError::Unavailable);
        }
        let token = self.server.issue_token(ttl);
        let mut state = self.state.borrow_mut();
        state.token = Some(token);
        state.refreshes += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;

    use super::*;

    fn note(n: u128) -> NoteId {
        NoteId::from_parts(1, n)
    }

    fn update(op: u128, note_n: u128, key: bool) -> PushUpdate {
        PushUpdate {
            id: OpId::from_parts(2, op),
            note: note(note_n),
            data: Bytes(vec![op as u8; 4]),
            note_key: key.then(|| Bytes(vec![note_n as u8; 8])),
        }
    }

    fn setup() -> (FakeServer, Token) {
        let server = FakeServer::new(ManualTimer::new());
        let token = server.issue_token(Duration::from_secs(900)).token;
        (server, token)
    }

    fn push(
        server: &FakeServer,
        token: &Token,
        updates: Vec<PushUpdate>,
    ) -> Result<PushResponse, TransportError> {
        block_on(server.push(token, &PushRequest { updates }))
    }

    #[test]
    fn push_is_idempotent_and_all_or_nothing() {
        let (server, token) = setup();
        let first = push(
            &server,
            &token,
            vec![update(1, 1, true), update(2, 1, false)],
        )
        .unwrap();
        assert_eq!(first.seqs, [1, 2]);
        // A retry stores nothing new and returns the same sequence numbers.
        let again = push(
            &server,
            &token,
            vec![update(1, 1, true), update(2, 1, false)],
        )
        .unwrap();
        assert_eq!(again, first);
        // One bad update rejects the whole batch.
        let err = push(
            &server,
            &token,
            vec![update(3, 1, false), update(4, 2, false)],
        );
        assert_eq!(err, Err(TransportError::MissingNoteKey));
        assert_eq!(server.head(), 2);
        assert_eq!(server.ops().len(), 2);
        // Same ID, different bytes.
        let mut conflict = update(1, 1, false);
        conflict.data = Bytes(vec![9]);
        assert_eq!(
            push(&server, &token, vec![conflict]),
            Err(TransportError::OpIdConflict)
        );
        let mut other_key = update(5, 1, true);
        other_key.note_key = Some(Bytes(vec![0; 8]));
        assert_eq!(
            push(&server, &token, vec![other_key]),
            Err(TransportError::NoteKeyConflict)
        );
    }

    #[test]
    fn pages_never_split_a_sequence_number() {
        let (server, token) = setup();
        server.set_page_groups(2);
        push(
            &server,
            &token,
            vec![update(1, 1, true), update(2, 2, true), update(3, 1, false)],
        )
        .unwrap();
        let p1 = block_on(server.changes(&token, 0)).unwrap();
        // seq 1 has note 1's key and update; seq 2 note 2's.
        assert_eq!(p1.entries.len(), 2);
        assert_eq!(p1.notes.len(), 2);
        assert!(p1.more);
        assert_eq!(p1.cursor, 2);
        let p2 = block_on(server.changes(&token, p1.cursor)).unwrap();
        assert_eq!(p2.entries.len(), 1);
        assert!(p2.notes.is_empty());
        assert!(!p2.more);
        assert_eq!(p2.cursor, 3);
        let p3 = block_on(server.changes(&token, 3)).unwrap();
        assert!(p3.entries.is_empty() && !p3.more && p3.cursor == 3);
        assert_eq!(
            block_on(server.changes(&token, 4)),
            Err(TransportError::CursorAhead)
        );
    }

    #[test]
    fn snapshot_compacts_and_rejects_stale() {
        let (server, token) = setup();
        push(
            &server,
            &token,
            vec![update(1, 1, true), update(2, 1, false), update(3, 2, true)],
        )
        .unwrap();
        block_on(server.upload_snapshot(&token, note(1), 2, b"snap")).unwrap();
        assert_eq!(server.ops().len(), 1);
        assert_eq!(
            block_on(server.upload_snapshot(&token, note(1), 2, b"snap")),
            Err(TransportError::SnapshotStale)
        );
        // A client that had seen seq 1 gets the snapshot instead of seq 2.
        let page = block_on(server.changes(&token, 1)).unwrap();
        assert_eq!(page.notes.len(), 2);
        let snap = page.notes.iter().find(|n| n.note == note(1)).unwrap();
        assert_eq!(snap.snapshot.as_ref().unwrap().covers, 2);
        assert!(snap.key.is_none());
        assert_eq!(page.entries.len(), 1);
        // Bootstrap still gets note 1's key, alongside its snapshot.
        let page = block_on(server.changes(&token, 0)).unwrap();
        let n1 = page.notes.iter().find(|n| n.note == note(1)).unwrap();
        assert!(n1.key.is_some() && n1.snapshot.is_some());
        assert_eq!(
            block_on(server.upload_snapshot(&token, note(9), 2, b"x")),
            Err(TransportError::UnknownNote)
        );
    }

    #[test]
    fn purge_leaves_a_tombstone_and_rejects_later_pushes() {
        let (server, token) = setup();
        push(&server, &token, vec![update(1, 1, true)]).unwrap();
        block_on(server.purge(&token, note(1))).unwrap();
        block_on(server.purge(&token, note(1))).unwrap();
        assert_eq!(server.head(), 2);
        assert_eq!(
            push(&server, &token, vec![update(2, 1, false)]),
            Err(TransportError::NotePurged)
        );
        let page = block_on(server.changes(&token, 0)).unwrap();
        assert_eq!(
            page.entries,
            [Entry::Purge {
                seq: 2,
                note: note(1)
            }]
        );
        assert!(page.notes.is_empty());
        assert_eq!(
            block_on(server.purge(&token, note(5))),
            Err(TransportError::UnknownNote)
        );
    }

    #[test]
    fn faults_tokens_and_reachability() {
        let (server, token) = setup();
        server.inject(Endpoint::Push, Fault::DropResponse);
        assert!(matches!(
            push(&server, &token, vec![update(1, 1, true)]),
            Err(TransportError::Network(_))
        ));
        // The work happened.
        assert_eq!(server.head(), 1);
        server.inject(Endpoint::Push, Fault::DropRequest);
        assert!(matches!(
            push(&server, &token, vec![update(2, 1, false)]),
            Err(TransportError::Network(_))
        ));
        assert_eq!(server.head(), 1);
        server.inject(
            Endpoint::Changes,
            Fault::RateLimited(Duration::from_secs(30)),
        );
        assert_eq!(
            block_on(server.changes(&token, 0)),
            Err(TransportError::RateLimited {
                retry_after: Some(Duration::from_secs(30))
            })
        );
        assert_eq!(
            block_on(server.changes(&Token("nope".into()), 0)),
            Err(TransportError::InvalidToken)
        );
        server.set_reachable(false);
        assert!(matches!(
            block_on(server.changes(&token, 0)),
            Err(TransportError::Network(_))
        ));
        assert_eq!(server.count(Endpoint::Changes), 3);
    }

    #[test]
    fn live_channel_auth_entries_and_push() {
        let timer = ManualTimer::new();
        let server = FakeServer::new(timer.clone());
        let token = server.issue_token(Duration::from_secs(60));
        let mut live = block_on(server.connect_live()).unwrap();
        block_on(live.send(&ClientFrame::Auth {
            token: token.token.clone(),
        }))
        .unwrap();
        assert_eq!(block_on(live.next()), LiveEvent::Ready { head: 0 });
        block_on(live.send(&ClientFrame::Push {
            request_id: "r1".into(),
            updates: vec![update(1, 1, true)],
        }))
        .unwrap();
        // The pusher gets the echo, then the result.
        assert!(matches!(
            block_on(live.next()),
            LiveEvent::Entry(Entry::Update {
                seq: 1,
                note_key: Some(_),
                ..
            })
        ));
        assert!(matches!(
            block_on(live.next()),
            LiveEvent::PushResult { request_id, result: Ok(_) } if request_id == "r1"
        ));

        // Re-auth extends past the first token's expiry.
        timer.advance(Duration::from_secs(30));
        let fresh = server.issue_token(Duration::from_secs(60));
        block_on(live.send(&ClientFrame::Auth { token: fresh.token })).unwrap();
        timer.advance(Duration::from_secs(45));
        server.check_expiry();
        assert_eq!(server.connections(), 1);
        timer.advance(Duration::from_secs(60));
        server.check_expiry();
        assert_eq!(block_on(live.next()), LiveEvent::Closed(Some(CLOSE_AUTH)));
        assert_eq!(block_on(live.next()), LiveEvent::Closed(Some(CLOSE_AUTH)));
    }

    #[test]
    fn manual_timer_wakes_due_sleeps() {
        let timer = ManualTimer::new();
        let sleep = timer.sleep(Duration::from_millis(500));
        assert_eq!(timer.next_deadline(), Some(500));
        timer.advance(Duration::from_millis(499));
        let mut sleep = Box::pin(sleep);
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(sleep.as_mut().poll(&mut cx).is_pending());
        timer.advance(Duration::from_millis(1));
        assert!(sleep.as_mut().poll(&mut cx).is_ready());
        drop(sleep);
        assert_eq!(timer.next_deadline(), None);
    }
}
