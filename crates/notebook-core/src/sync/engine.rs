//! The run loop: decides when to pull, push, snapshot, reconnect, and back
//! off. Storage owns durability and echo recognition; this owns policy.

use std::collections::BTreeSet;
use std::future;
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt, pin_mut, select_biased};

use super::backoff::Backoff;
use super::replica::{Applied, Replica};
use super::session::{AccessToken, RefreshError, SessionSource};
use super::timer::Timer;
use super::transport::{LiveChannel, LiveEvent, Transport, TransportError};
use super::{
    Attention, Command, Shared, SyncConfig, SyncError, SyncEvent, SyncHandle, SyncState, SyncStatus,
};
use crate::auth::api::Token;
use crate::note::NoteId;
use crate::protocol::{
    CLOSE_AUTH, CLOSE_RESYNC, ChangesResponse, ClientFrame, Entry, MAX_PUSH_UPDATES, PushRequest,
    PushResponse, PushUpdate,
};
use crate::storage::{ServerSeq, Storage, StorageError};

/// Reopens an empty store for the same vault after a resync cleared it.
pub type Reopen<S> = Box<dyn FnMut() -> Result<S, StorageError>>;

/// Why a step stopped early.
#[derive(Debug)]
enum Failure {
    Transport(TransportError),
    /// The session ended; nothing more until the host signs in again.
    SignIn,
    Storage(StorageError),
}

impl From<TransportError> for Failure {
    fn from(err: TransportError) -> Self {
        Self::Transport(err)
    }
}

impl From<StorageError> for Failure {
    fn from(err: StorageError) -> Self {
        Self::Storage(err)
    }
}

/// A push waiting for its result.
#[derive(Debug, Clone)]
struct Batch {
    updates: Vec<PushUpdate>,
    /// Notes whose wrapped key this batch carries.
    keys: Vec<NoteId>,
}

struct Live<L> {
    channel: L,
    opened_at: u64,
    backoff_reset: bool,
    /// When to send the next `auth` frame.
    reauth_at: u64,
    /// A `push` frame waiting for its `push_result`: request ID, batch, and
    /// when it was sent.
    inflight: Option<(String, Batch, u64)>,
}

enum Wakeup {
    Command(Option<Command>),
    Wake,
    Live(LiveEvent),
    Timer,
}

/// The sync engine for one vault: one long-lived, single-threaded task.
///
/// Create it with [`SyncEngine::new`], spawn [`run`](Self::run) (with
/// `wasm_bindgen_futures::spawn_local` in the browser, or on a
/// current-thread tokio runtime on a dedicated thread natively), drive it
/// through the returned [`SyncHandle`], and consume [`SyncEvent`]s from the
/// returned receiver. Nothing it holds needs to be `Send`.
pub struct SyncEngine<S: Storage, T: Transport, M: Timer, A: SessionSource> {
    replica: Shared<Replica<S>>,
    transport: T,
    timer: M,
    session: A,
    config: SyncConfig,
    reopen: Reopen<S>,
    events: mpsc::UnboundedSender<SyncEvent>,
    commands: mpsc::UnboundedReceiver<Command>,
    wake: mpsc::Receiver<()>,

    live: Option<Live<T::Live>>,
    /// `None` until the first attempt; then whether the last one reached the
    /// server.
    connected: Option<bool>,
    caught_up: bool,
    pull_due: bool,
    resync_due: bool,
    /// Don't resync before this: a server that keeps asking for resyncs
    /// gets them with backoff.
    resync_not_before: u64,
    /// A resync ran and no push has succeeded since.
    resynced_since_push: bool,
    next_connect: u64,
    next_poll: u64,
    next_safety_pull: u64,
    /// No request before this (rate limit).
    not_before: u64,
    backoff: Backoff,

    /// Push once the clock reaches this; `None` pushes right away.
    debounce_until: Option<u64>,
    first_unpushed: Option<u64>,
    /// Halved on each `413` for a multi-update batch; cleared once the
    /// outbox empties.
    batch_limit: Option<usize>,
    /// One note per batch, after a `410` on a multi-note batch.
    isolate_notes: bool,
    /// The last push hit a full quota; cleared only by a successful push.
    quota_full: bool,
    /// Retry pushing at this time while the quota is full.
    quota_until: Option<u64>,
    too_large: Option<NoteId>,
    push_blocked: bool,
    sign_in_required: bool,
    upgrade_required: bool,
    integrity_warnings: u32,
    busy: bool,
    flushes: Vec<oneshot::Sender<Result<(), SyncError>>>,
    last_status: Option<SyncStatus>,
    next_request_id: u64,
    stopping: bool,
}

impl<S, T, M, A> SyncEngine<S, T, M, A>
where
    S: Storage,
    T: Transport,
    M: Timer,
    A: SessionSource,
{
    /// Sets up an engine over `replica`. `reopen` opens an empty store for
    /// the same vault (`MemoryStorage::new`, or the SQLite store at the same
    /// path); it is used only to rebuild storage after a server restore.
    pub fn new(
        replica: Shared<Replica<S>>,
        transport: T,
        timer: M,
        session: A,
        config: SyncConfig,
        reopen: Reopen<S>,
    ) -> (Self, SyncHandle, mpsc::UnboundedReceiver<SyncEvent>) {
        let (events, events_rx) = mpsc::unbounded();
        let (commands_tx, commands) = mpsc::unbounded();
        let (wake_tx, wake) = mpsc::channel(1);
        replica.lock().set_wake(wake_tx.clone());
        let backoff = Backoff::new(config.backoff_base, config.backoff_cap, config.jitter);
        let engine = Self {
            replica,
            transport,
            timer,
            session,
            config,
            reopen,
            events,
            commands,
            wake,
            live: None,
            connected: None,
            caught_up: false,
            pull_due: false,
            resync_due: false,
            resync_not_before: 0,
            resynced_since_push: false,
            next_connect: 0,
            next_poll: 0,
            next_safety_pull: 0,
            not_before: 0,
            backoff,
            debounce_until: None,
            first_unpushed: None,
            batch_limit: None,
            isolate_notes: false,
            quota_full: false,
            quota_until: None,
            too_large: None,
            push_blocked: false,
            sign_in_required: false,
            upgrade_required: false,
            integrity_warnings: 0,
            busy: false,
            flushes: Vec::new(),
            last_status: None,
            next_request_id: 0,
            stopping: false,
        };
        let handle = SyncHandle {
            commands: commands_tx,
            wake: wake_tx,
        };
        (engine, handle, events_rx)
    }

    /// Runs until [`SyncHandle::shutdown`] or every handle is dropped.
    pub async fn run(mut self) {
        let issues = self.replica.lock().take_open_issues();
        self.report(Applied {
            changed: BTreeSet::new(),
            issues,
        });
        self.emit_status();
        while !self.stopping {
            self.step().await;
            self.emit_status();
            if self.stopping {
                break;
            }
            match self.wait().await {
                Wakeup::Command(Some(command)) => self.command(command).await,
                Wakeup::Command(None) => self.stopping = true,
                Wakeup::Wake => self.local_change(),
                Wakeup::Live(event) => {
                    if let Err(failure) = self.live_event(event).await {
                        let err = self.fail(failure);
                        self.settle_flushes(Some(err));
                    }
                    self.settle_flushes(None);
                }
                Wakeup::Timer => {}
            }
        }
        self.live = None;
        for flush in self.flushes.drain(..) {
            let _ = flush.send(Err(SyncError::Stopped));
        }
    }

    fn now(&self) -> u64 {
        self.timer.now()
    }

    /// Waits for the next command, local edit, live event, or deadline.
    async fn wait(&mut self) -> Wakeup {
        let deadline = self.next_deadline();
        let now = self.now();
        let Self {
            timer,
            live,
            commands,
            wake,
            ..
        } = self;
        let sleep = async {
            match deadline {
                Some(at) => {
                    timer
                        .sleep(Duration::from_millis(at.saturating_sub(now)))
                        .await
                }
                None => future::pending().await,
            }
        }
        .fuse();
        let live = async {
            match live {
                Some(live) => live.channel.next().await,
                None => future::pending().await,
            }
        }
        .fuse();
        pin_mut!(sleep, live);
        select_biased! {
            command = commands.next() => Wakeup::Command(command),
            _ = wake.next() => Wakeup::Wake,
            event = live => Wakeup::Live(event),
            () = sleep => Wakeup::Timer,
        }
    }

    /// When the loop must next run on its own, or `None` to wait for a
    /// command or event.
    fn next_deadline(&self) -> Option<u64> {
        if self.sign_in_required {
            return None;
        }
        let now = self.now();
        if self.resync_due {
            // Nothing else runs until the resync does.
            return Some(self.resync_not_before.max(now).max(self.not_before));
        }
        let mut deadlines = Vec::new();
        match &self.live {
            None => {
                deadlines.push(self.next_connect);
                if self.connected == Some(true) {
                    deadlines.push(self.next_poll);
                }
            }
            Some(live) => {
                deadlines.push(self.next_safety_pull);
                deadlines.push(live.reauth_at);
                if !live.backoff_reset {
                    deadlines.push(live.opened_at + ms(self.config.backoff_reset_after));
                }
                if let Some((_, _, sent)) = &live.inflight {
                    deadlines.push(sent + ms(self.config.push_result_timeout));
                }
            }
        }
        if self.pull_wanted(now) {
            deadlines.push(now);
        }
        if self.push_possible() {
            deadlines.push(self.debounce_until.unwrap_or(now));
        }
        if let Some(until) = self.quota_until.filter(|until| *until > now) {
            deadlines.push(until);
        }
        deadlines.into_iter().min().map(|d| d.max(self.not_before))
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Flush(reply) => {
                self.flushes.push(reply);
                self.debounce_until = None;
                self.first_unpushed = None;
                if self.live.is_none() {
                    // Try now rather than waiting out the backoff.
                    self.next_connect = self.next_connect.min(self.now());
                }
                self.step().await;
            }
            Command::Purge(note, reply) => {
                let result = self.purge(note).await;
                let _ = reply.send(result);
            }
            Command::SessionChanged => {
                self.sign_in_required = false;
                self.connected = None;
                self.next_connect = self.now();
            }
            Command::Shutdown => self.stopping = true,
        }
    }

    /// A local edit (or a host wake): push soon, coalescing a burst.
    fn local_change(&mut self) {
        let now = self.now();
        let first = *self.first_unpushed.get_or_insert(now);
        let until = (now + ms(self.config.debounce)).min(first + ms(self.config.max_debounce));
        self.debounce_until = Some(until);
        // Retry a push paused by a full quota (the status keeps saying so
        // until one succeeds).
        self.quota_until = None;
    }

    /// Does whatever is due, then settles flushes.
    async fn step(&mut self) {
        if self.stopping {
            return;
        }
        if self.sign_in_required {
            self.settle_flushes(Some(SyncError::SignInRequired));
            return;
        }
        let failure = match self.step_inner().await {
            Ok(()) => None,
            Err(failure) => Some(self.fail(failure)),
        };
        self.settle_flushes(failure);
    }

    async fn step_inner(&mut self) -> Result<(), Failure> {
        let now = self.now();
        if now < self.not_before {
            return Ok(());
        }
        if self.resync_due {
            if now < self.resync_not_before {
                return Ok(());
            }
            self.resync()?;
        }
        let mut pulled = false;
        if self.live.is_none() && now >= self.next_connect {
            self.connect().await?;
            pulled = true;
        }
        self.live_upkeep().await?;
        if !pulled && self.pull_wanted(self.now()) {
            self.pull().await?;
        }
        let mut rounds = 0;
        while self.push_wanted() && !self.resync_due && rounds < 64 {
            rounds += 1;
            self.push().await?;
        }
        if self.caught_up && self.connected == Some(true) && !self.resync_due {
            self.snapshot().await?;
        }
        Ok(())
    }

    fn pull_wanted(&self, now: u64) -> bool {
        if self.live.is_some() {
            // Not caught up means a catch-up pull failed and must be retried
            // (after any rate limit).
            return self.pull_due || !self.caught_up || now >= self.next_safety_pull;
        }
        // Polling mode: pull on the interval, and before every push.
        self.connected == Some(true)
            && (self.pull_due
                || now >= self.next_poll
                || (self.push_possible() && self.debounced()))
    }

    /// Whether a push could go out once the debounce allows.
    fn push_possible(&self) -> bool {
        let inflight = self.live.as_ref().is_some_and(|l| l.inflight.is_some());
        self.connected == Some(true)
            && !inflight
            && !self.push_blocked
            && self.too_large.is_none()
            && self.quota_until.is_none_or(|until| self.now() >= until)
            && self.pending() > 0
    }

    fn debounced(&self) -> bool {
        self.debounce_until.is_none_or(|until| self.now() >= until)
    }

    fn push_wanted(&self) -> bool {
        self.caught_up && self.push_possible() && self.debounced()
    }

    fn pending(&self) -> usize {
        self.replica.lock().pending().unwrap_or(0)
    }

    // ---- Requests ----

    /// The current token, refreshed first if it has expired.
    async fn token(&mut self) -> Result<AccessToken, Failure> {
        match self.session.access_token() {
            Some(token) if token.expires_at > self.now() => Ok(token),
            Some(_) => {
                self.refresh().await?;
                self.session.access_token().ok_or(Failure::SignIn)
            }
            None => Err(Failure::SignIn),
        }
    }

    async fn refresh(&mut self) -> Result<(), Failure> {
        match self.session.refresh().await {
            Ok(()) => Ok(()),
            Err(RefreshError::SessionEnded) => Err(Failure::SignIn),
            Err(RefreshError::Unavailable) => Err(Failure::Transport(TransportError::Network(
                "session refresh unavailable".into(),
            ))),
        }
    }

    /// Runs one authenticated request; on `401` refreshes once and retries.
    async fn call<R>(
        &mut self,
        op: impl AsyncFn(&T, &Token) -> Result<R, TransportError>,
    ) -> Result<R, Failure> {
        self.busy = true;
        self.emit_status();
        let result = self.call_inner(op).await;
        self.busy = false;
        result
    }

    async fn call_inner<R>(
        &mut self,
        op: impl AsyncFn(&T, &Token) -> Result<R, TransportError>,
    ) -> Result<R, Failure> {
        let token = self.token().await?.token;
        match op(&self.transport, &token).await {
            Err(TransportError::InvalidToken) => {
                self.refresh().await?;
                let token = self.token().await?.token;
                match op(&self.transport, &token).await {
                    Err(TransportError::InvalidToken) => Err(Failure::SignIn),
                    result => Ok(result?),
                }
            }
            result => Ok(result?),
        }
    }

    /// Opens the live channel and catches up. If the channel can't be
    /// opened, falls back to polling over HTTP.
    async fn connect(&mut self) -> Result<(), Failure> {
        match self.open_live().await {
            Ok(live) => {
                self.live = Some(live);
                self.connected = Some(true);
                self.pull().await
            }
            Err(failure @ Failure::SignIn) => Err(failure),
            Err(Failure::Transport(err @ TransportError::RateLimited { .. })) => Err(err.into()),
            Err(_) => {
                self.next_connect = self.now() + ms(self.backoff.next_delay());
                self.pull().await
            }
        }
    }

    async fn open_live(&mut self) -> Result<Live<T::Live>, Failure> {
        for attempt in 0..2 {
            let token = self.token().await?;
            let mut channel = self.transport.connect_live().await?;
            channel
                .send(&ClientFrame::Auth {
                    token: token.token.clone(),
                })
                .await?;
            let deadline = self.now() + ms(self.config.ready_timeout);
            loop {
                let now = self.now();
                if now >= deadline {
                    return Err(TransportError::Network("no ready frame".into()).into());
                }
                let event = {
                    let sleep = self
                        .timer
                        .sleep(Duration::from_millis(deadline - now))
                        .fuse();
                    let next = channel.next().fuse();
                    pin_mut!(sleep, next);
                    select_biased! {
                        event = next => Some(event),
                        () = sleep => None,
                    }
                };
                match event {
                    Some(LiveEvent::Ready { .. }) => {
                        let now = self.now();
                        return Ok(Live {
                            channel,
                            opened_at: now,
                            backoff_reset: false,
                            reauth_at: self.reauth_at(&token),
                            inflight: None,
                        });
                    }
                    Some(LiveEvent::Closed(Some(CLOSE_AUTH))) if attempt == 0 => {
                        self.refresh().await?;
                        break;
                    }
                    Some(LiveEvent::Closed(Some(CLOSE_AUTH))) => return Err(Failure::SignIn),
                    Some(LiveEvent::Closed(code)) => {
                        return Err(TransportError::Network(format!(
                            "live channel closed ({code:?})"
                        ))
                        .into());
                    }
                    Some(LiveEvent::Error(err)) => return Err(err.into()),
                    // Nothing else comes before `ready`.
                    Some(_) => {}
                    None => return Err(TransportError::Network("no ready frame".into()).into()),
                }
            }
        }
        Err(Failure::SignIn)
    }

    fn reauth_at(&self, token: &AccessToken) -> u64 {
        token
            .expires_at
            .saturating_sub(ms(self.config.reauth_margin))
            .max(self.now() + 1000)
    }

    /// Housekeeping for an open live channel: backoff reset, renewing its
    /// authentication, and abandoning a push whose result never came.
    async fn live_upkeep(&mut self) -> Result<(), Failure> {
        let now = self.now();
        let reset_after = ms(self.config.backoff_reset_after);
        let timeout = ms(self.config.push_result_timeout);
        let Some(live) = &mut self.live else {
            return Ok(());
        };
        if !live.backoff_reset && now >= live.opened_at + reset_after {
            live.backoff_reset = true;
            self.backoff.reset();
        }
        if live
            .inflight
            .as_ref()
            .is_some_and(|(_, _, sent)| now >= sent + timeout)
        {
            // Treat the socket as broken; the batch goes again over HTTP.
            self.drop_live();
            self.next_connect = now;
            return Ok(());
        }
        if now >= live.reauth_at {
            let token = match self.session.access_token() {
                Some(t) if t.expires_at > now + ms(self.config.reauth_margin) => t,
                _ => match self.session.refresh().await {
                    Ok(()) => self.session.access_token().ok_or(Failure::SignIn)?,
                    Err(RefreshError::SessionEnded) => return Err(Failure::SignIn),
                    Err(RefreshError::Unavailable) => {
                        if let Some(live) = &mut self.live {
                            live.reauth_at = now + 10_000;
                        }
                        return Ok(());
                    }
                },
            };
            let reauth_at = self.reauth_at(&token);
            let Some(live) = &mut self.live else {
                return Ok(());
            };
            let sent = live
                .channel
                .send(&ClientFrame::Auth { token: token.token })
                .await;
            match sent {
                Ok(()) => live.reauth_at = reauth_at,
                Err(_) => {
                    self.drop_live();
                    self.next_connect = now;
                }
            }
        }
        Ok(())
    }

    fn drop_live(&mut self) {
        self.live = None;
    }

    /// Pulls pages from the stored cursor until caught up.
    async fn pull(&mut self) -> Result<(), Failure> {
        self.caught_up = false;
        loop {
            let after = self.replica.lock().storage()?.cursor()?.0;
            let page = self
                .call(async |t, token| t.changes(token, after).await)
                .await?;
            self.connected = Some(true);
            if page.more && page.cursor <= after {
                // A page that doesn't advance would be fetched forever.
                return Err(TransportError::Server(200).into());
            }
            let applied = self.replica.lock().apply_page(&page)?;
            self.report(applied);
            if !page.more {
                break;
            }
        }
        let now = self.now();
        self.caught_up = true;
        self.pull_due = false;
        self.next_poll = now + ms(self.config.poll_interval);
        self.next_safety_pull = now + ms(self.config.safety_pull_interval);
        Ok(())
    }

    /// The next batch from the outbox, in order, within the count and size
    /// caps. The first update of each note with a pending key carries it.
    fn next_batch(&self) -> Result<Batch, StorageError> {
        let replica = self.replica.lock();
        let store = replica.storage()?;
        let pending_keys = store.pending_keys()?;
        let limit = self
            .config
            .max_batch_updates
            .min(MAX_PUSH_UPDATES)
            .min(self.batch_limit.unwrap_or(usize::MAX))
            .max(1);
        let mut batch = Batch {
            updates: Vec::new(),
            keys: Vec::new(),
        };
        let mut bytes = 0;
        for entry in store.outbox()? {
            if batch.updates.len() == limit {
                break;
            }
            if !batch.updates.is_empty() {
                let other_note = batch.updates[0].note != entry.note;
                if (self.isolate_notes && other_note)
                    || bytes + entry.bytes.len() > self.config.max_batch_bytes
                {
                    break;
                }
            }
            bytes += entry.bytes.len();
            let key = pending_keys
                .iter()
                .find(|(note, _)| *note == entry.note && !batch.keys.contains(note))
                .map(|(note, key)| {
                    batch.keys.push(*note);
                    key.clone().into()
                });
            batch.updates.push(PushUpdate {
                id: entry.id.into(),
                note: entry.note,
                data: entry.bytes.into(),
                note_key: key,
            });
        }
        Ok(batch)
    }

    /// Sends one batch: as a `push` frame while live (the result arrives as
    /// a live event), otherwise over HTTP.
    async fn push(&mut self) -> Result<(), Failure> {
        let batch = self.next_batch()?;
        if batch.updates.is_empty() {
            return Ok(());
        }
        self.debounce_until = None;
        self.first_unpushed = None;
        if let Some(live) = &mut self.live {
            self.next_request_id += 1;
            let request_id = self.next_request_id.to_string();
            let frame = ClientFrame::Push {
                request_id: request_id.clone(),
                updates: batch.updates.clone(),
            };
            match live.channel.send(&frame).await {
                Ok(()) => {
                    let now = self.timer.now();
                    if let Some(live) = &mut self.live {
                        live.inflight = Some((request_id, batch, now));
                    }
                    return Ok(());
                }
                Err(_) => {
                    // Fall through to HTTP; the socket gets reopened.
                    self.drop_live();
                    self.next_connect = self.now();
                }
            }
        }
        let request = PushRequest {
            updates: batch.updates.clone(),
        };
        let result = match self
            .call(async |t, token| t.push(token, &request).await)
            .await
        {
            Ok(response) => Ok(response),
            Err(Failure::Transport(err)) => Err(err),
            Err(failure) => return Err(failure),
        };
        self.push_result(batch, result)
    }

    /// Handles a push's outcome, whichever path it took.
    fn push_result(
        &mut self,
        batch: Batch,
        result: Result<PushResponse, TransportError>,
    ) -> Result<(), Failure> {
        match result {
            Ok(response) => {
                if response.seqs.len() != batch.updates.len() {
                    return Err(TransportError::Server(200).into());
                }
                self.connected = Some(true);
                self.quota_full = false;
                self.quota_until = None;
                self.resynced_since_push = false;
                let mut replica = self.replica.lock();
                let store = replica.storage_mut()?;
                for (update, seq) in batch.updates.iter().zip(&response.seqs) {
                    store.acknowledge(update.id.into(), ServerSeq(*seq))?;
                }
                for note in &batch.keys {
                    store.mark_key_uploaded(*note)?;
                }
                if store.outbox()?.is_empty() {
                    self.batch_limit = None;
                    self.isolate_notes = false;
                }
                Ok(())
            }
            Err(TransportError::QuotaExceeded) => {
                self.quota_full = true;
                self.quota_until = Some(self.now() + ms(self.config.quota_retry));
                Ok(())
            }
            Err(TransportError::UpdateTooLarge) if batch.updates.len() > 1 => {
                self.batch_limit = Some(batch.updates.len() / 2);
                Ok(())
            }
            Err(TransportError::UpdateTooLarge) => {
                self.too_large = Some(batch.updates[0].note);
                Ok(())
            }
            Err(TransportError::NotePurged) => {
                let notes: BTreeSet<NoteId> = batch.updates.iter().map(|u| u.note).collect();
                if notes.len() == 1 {
                    self.purged_elsewhere(batch.updates[0].note)?;
                } else {
                    self.isolate_notes = true;
                }
                Ok(())
            }
            Err(TransportError::UnknownNote | TransportError::MissingNoteKey) => {
                self.request_resync();
                Ok(())
            }
            Err(TransportError::OpIdConflict | TransportError::NoteKeyConflict) => {
                self.push_blocked = true;
                self.warn(batch.updates[0].note);
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Removes a note the server says was purged, and reports it.
    fn purged_elsewhere(&mut self, note: NoteId) -> Result<(), StorageError> {
        self.replica.lock().delete_note(note)?;
        let _ = self
            .events
            .unbounded_send(SyncEvent::NotesChanged(vec![note]));
        Ok(())
    }

    /// Uploads at most one due snapshot, then compacts the note locally.
    async fn snapshot(&mut self) -> Result<(), Failure> {
        let threshold = self.config.snapshot_threshold;
        let (note, prepared, covers) = {
            let replica = self.replica.lock();
            let store = replica.storage()?;
            let in_outbox: BTreeSet<NoteId> = store.outbox()?.iter().map(|e| e.note).collect();
            let Some(note) = replica.snapshot_candidate(threshold, &in_outbox) else {
                return Ok(());
            };
            let covers = store.cursor()?.0;
            let prepared = match replica.prepare_snapshot(note) {
                Ok(Some(prepared)) => prepared,
                Ok(None) | Err(_) => {
                    drop(replica);
                    self.replica.lock().defer_snapshot(note, threshold);
                    return Ok(());
                }
            };
            (note, prepared, covers)
        };
        let data = prepared.snapshot.as_bytes().to_vec();
        let result = self
            .call(async |t, token| t.upload_snapshot(token, note, covers, &data).await)
            .await;
        match result {
            Ok(()) => {
                self.replica
                    .lock()
                    .compact_note(note, &prepared.snapshot, prepared.through)?;
                Ok(())
            }
            Err(Failure::Transport(
                TransportError::SnapshotStale
                | TransportError::UpdateTooLarge
                | TransportError::BadRequest
                | TransportError::QuotaExceeded,
            )) => {
                self.replica.lock().defer_snapshot(note, threshold);
                Ok(())
            }
            Err(Failure::Transport(TransportError::NotePurged)) => {
                self.purged_elsewhere(note)?;
                Ok(())
            }
            Err(Failure::Transport(TransportError::UnknownNote)) => {
                self.request_resync();
                Ok(())
            }
            Err(failure) => Err(failure),
        }
    }

    /// Purges a trashed note on the server, then locally.
    async fn purge(&mut self, note: NoteId) -> Result<(), SyncError> {
        match self.replica.lock().note(note) {
            None => return Err(SyncError::NoteNotFound(note)),
            Some(n) if !n.is_trashed() => return Err(SyncError::NotTrashed(note)),
            Some(_) => {}
        }
        if self.sign_in_required {
            return Err(SyncError::SignInRequired);
        }
        if self.now() < self.not_before {
            return Err(SyncError::RateLimited);
        }
        match self.call(async |t, token| t.purge(token, note).await).await {
            // Unknown: it was never pushed, so there is nothing to purge
            // remotely; its queued updates go with it.
            Ok(()) | Err(Failure::Transport(TransportError::UnknownNote)) => {
                self.connected = Some(true);
                self.replica
                    .lock()
                    .delete_note(note)
                    .map_err(SyncError::Storage)?;
                if self.too_large == Some(note) {
                    self.too_large = None;
                }
                self.emit_status();
                Ok(())
            }
            Err(failure) => Err(self.fail(failure)),
        }
    }

    /// Rebuilds storage from memory after the server lost history, then
    /// starts over from cursor zero.
    fn resync(&mut self) -> Result<(), Failure> {
        self.resync_due = false;
        self.drop_live();
        self.caught_up = false;
        self.batch_limit = None;
        self.isolate_notes = false;
        let rebuilt = {
            let mut replica = self.replica.lock();
            replica.rebuild_storage(&mut *self.reopen)
        };
        let now = self.now();
        if let Err(err) = rebuilt {
            // Retry from memory later; the documents still hold everything.
            self.resync_due = true;
            self.resync_not_before = now + ms(self.backoff.next_delay());
            return Err(match err {
                super::ReplicaError::Storage(err) => Failure::Storage(err),
                other => Failure::Storage(StorageError::Backend(other.to_string())),
            });
        }
        self.resynced_since_push = true;
        self.next_connect = now;
        Ok(())
    }

    /// Schedules a resync: right away the first time, with backoff if the
    /// last one hasn't been followed by a successful push.
    fn request_resync(&mut self) {
        self.resync_due = true;
        let now = self.now();
        self.resync_not_before = if self.resynced_since_push {
            now + ms(self.backoff.next_delay())
        } else {
            now
        };
    }

    // ---- Live events ----

    async fn live_event(&mut self, event: LiveEvent) -> Result<(), Failure> {
        match event {
            LiveEvent::Ready { .. } => Ok(()),
            LiveEvent::Entry(entry) => self.live_entry(entry),
            LiveEvent::PushResult { request_id, result } => {
                let inflight = self.live.as_mut().and_then(|l| {
                    if l.inflight
                        .as_ref()
                        .is_some_and(|(id, _, _)| *id == request_id)
                    {
                        l.inflight.take()
                    } else {
                        None
                    }
                });
                match inflight {
                    Some((_, batch, _)) => self.push_result(batch, result),
                    None => Ok(()),
                }
            }
            LiveEvent::Closed(code) => {
                let now = self.now();
                // A channel closed soon after opening reconnects with
                // backoff, so a server closing every connection can't spin us.
                let short_lived = self
                    .live
                    .as_ref()
                    .is_some_and(|l| now < l.opened_at + ms(self.config.backoff_reset_after));
                self.drop_live();
                let soon = if short_lived {
                    now + ms(self.backoff.next_delay())
                } else {
                    now
                };
                match code {
                    Some(CLOSE_AUTH) => {
                        self.next_connect = soon;
                        self.refresh().await
                    }
                    Some(CLOSE_RESYNC) => {
                        // The step pulls, respecting any rate limit.
                        self.pull_due = true;
                        self.next_connect = soon;
                        Ok(())
                    }
                    // Too many connections, shutting down, or no code:
                    // back off, and check over HTTP meanwhile.
                    _ => {
                        self.next_connect = now + ms(self.backoff.next_delay());
                        self.pull_due = true;
                        Ok(())
                    }
                }
            }
            LiveEvent::Error(_) => {
                self.drop_live();
                self.next_connect = self.now() + ms(self.backoff.next_delay());
                self.pull_due = true;
                Ok(())
            }
        }
    }

    /// Applies a live entry above the cursor, as a one-entry page.
    fn live_entry(&mut self, entry: Entry) -> Result<(), Failure> {
        if !self.caught_up {
            // It will come with the next pull.
            self.pull_due = true;
            return Ok(());
        }
        let cursor = self.replica.lock().storage()?.cursor()?.0;
        let seq = entry.seq();
        if seq <= cursor {
            return Ok(());
        }
        let page = ChangesResponse {
            notes: Vec::new(),
            entries: vec![entry],
            cursor: seq,
            more: false,
        };
        let applied = self.replica.lock().apply_page(&page)?;
        self.report(applied);
        Ok(())
    }

    // ---- Failures, events, and status ----

    /// Records a failed step and schedules the retry. Returns what a flush
    /// waiting on it should report.
    fn fail(&mut self, failure: Failure) -> SyncError {
        let now = self.now();
        match failure {
            Failure::SignIn => {
                self.sign_in_required = true;
                self.connected = Some(false);
                self.drop_live();
                SyncError::SignInRequired
            }
            Failure::Transport(TransportError::RateLimited { retry_after }) => {
                let wait = retry_after.unwrap_or_else(|| self.backoff.next_delay());
                self.not_before = now + ms(wait);
                self.next_connect = self.next_connect.max(self.not_before);
                SyncError::RateLimited
            }
            Failure::Transport(TransportError::CursorAhead) => {
                self.request_resync();
                SyncError::Offline
            }
            Failure::Transport(err @ TransportError::Network(_))
            | Failure::Transport(err @ TransportError::Server(_)) => {
                self.went_offline(now);
                if matches!(err, TransportError::Network(_)) {
                    SyncError::Offline
                } else {
                    SyncError::Rejected(err)
                }
            }
            Failure::Transport(err) => {
                // An unexpected rejection: back off as if offline.
                self.went_offline(now);
                SyncError::Rejected(err)
            }
            Failure::Storage(err) => {
                // Hold off every request, so a store that keeps failing
                // doesn't turn into back-to-back pulls.
                self.not_before = now + ms(self.backoff.next_delay());
                self.next_connect = self.next_connect.max(self.not_before);
                SyncError::Storage(err)
            }
        }
    }

    fn went_offline(&mut self, now: u64) {
        self.connected = Some(false);
        self.caught_up = false;
        self.drop_live();
        // Unless this attempt already scheduled the next one.
        if self.next_connect <= now {
            self.next_connect = now + ms(self.backoff.next_delay());
        }
    }

    /// Resolves waiting flushes: with `failure`, with success once the
    /// outbox is empty, or with the reason pushing is paused.
    fn settle_flushes(&mut self, failure: Option<SyncError>) {
        if self.flushes.is_empty() {
            return;
        }
        let result = if let Some(err) = failure {
            Err(err)
        } else if self.pending() == 0 {
            Ok(())
        } else if self.sign_in_required {
            Err(SyncError::SignInRequired)
        } else if self.push_blocked {
            Err(SyncError::PushBlocked)
        } else if let Some(note) = self.too_large {
            Err(SyncError::UpdateTooLarge(note))
        } else if self.quota_until.is_some_and(|until| self.now() < until) {
            Err(SyncError::QuotaFull)
        } else if self.now() < self.not_before {
            Err(SyncError::RateLimited)
        } else if self.connected != Some(true) {
            Err(SyncError::Offline)
        } else {
            // A push is in flight or about to go; keep waiting.
            return;
        };
        for flush in self.flushes.drain(..) {
            let _ = flush.send(result.clone());
        }
    }

    fn report(&mut self, applied: Applied) {
        if !applied.changed.is_empty() {
            let notes = applied.changed.into_iter().collect();
            let _ = self.events.unbounded_send(SyncEvent::NotesChanged(notes));
        }
        for note in applied.issues.integrity {
            self.warn(note);
        }
        self.upgrade_required |= applied.issues.upgrade_required;
    }

    fn warn(&mut self, note: NoteId) {
        self.integrity_warnings = self.integrity_warnings.saturating_add(1);
        let _ = self
            .events
            .unbounded_send(SyncEvent::IntegrityWarning(note));
    }

    fn status(&self) -> SyncStatus {
        let pending = self.pending();
        let state = match self.connected {
            Some(false) => SyncState::Offline,
            None => SyncState::Syncing,
            Some(true) if self.busy || pending > 0 || !self.caught_up => SyncState::Syncing,
            Some(true) => SyncState::Synced,
        };
        let attention = if self.sign_in_required {
            Some(Attention::SignInRequired)
        } else if let Some(note) = self.too_large {
            Some(Attention::UpdateTooLarge(note))
        } else if self.quota_full {
            Some(Attention::QuotaFull)
        } else if self.upgrade_required {
            Some(Attention::UpgradeRequired)
        } else {
            None
        };
        SyncStatus {
            state,
            pending,
            attention,
            integrity_warnings: self.integrity_warnings,
        }
    }

    fn emit_status(&mut self) {
        let status = self.status();
        if self.last_status != Some(status) {
            self.last_status = Some(status);
            let _ = self.events.unbounded_send(SyncEvent::Status(status));
        }
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
