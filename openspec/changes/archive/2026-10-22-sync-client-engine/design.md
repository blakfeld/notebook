# Design

## Context

See proposal.md (Why). Requirements are in `specs/sync-client/spec.md`.

What this builds on:

- `notebook-core` has `note` (`NoteId`, `Timestamp`, `Note`), `tags`, and `crypto` (`VaultKey`, `NoteKey`, `wrap_note_key`/`unwrap_note_key`, the envelope). It forbids `unsafe` and CI builds it for `wasm32-unknown-unknown`.
- `crdt-note-documents`: `NoteDocument` owns its `NoteKey`, emits only `EncryptedUpdate` / `EncryptedSnapshot`, and leaves its state unchanged when `apply_update` / `load_snapshot` fail (`DocumentError::{Crypto, UnsupportedFormat, Malformed}`). Its snapshot includes pending (held) structs, so compaction can't drop an update that is waiting on a dependency. It leaves "when to snapshot" to this change.
- `local-sqlite-storage`: a synchronous `Storage` trait with `&mut self` writes, each multi-row write atomic. A note is `base snapshot + log entries` ordered by a storage-assigned `LogPos`. Remote snapshots are appended to the log, not swapped in as the base. `apply_remote(RemoteBatch { keys, entries, cursor })` stores a batch and the cursor in one transaction, and it recognises echoes of this device's own updates by `UpdateId` (16 bytes): it marks them acknowledged instead of storing them twice. `compact(note, snapshot, through: LogPos)` never drops outbox entries. Wrapped keys of locally created notes stay pending until `mark_key_uploaded`. `delete_note` removes a note's key, log, and outbox entries.
- `sync-server` (`sync-protocol`), under `/sync/v1`, JSON with base64 payloads, types in `notebook_core::protocol`:
    - `POST push`: `PushUpdate { id: OpId, note, data, note_key: Option }` → `PushResponse { seqs, head }`. All-or-nothing per batch.
    - `GET changes?after=C`: `ChangesResponse { notes: [NoteEntry { key, snapshot }], entries: [Update { seq, id, note, data, note_key } | Purge { seq, note }], cursor, more }`. Bootstrap is `after=0`. Pages never split a sequence number.
    - `PUT notes/{id}/snapshot?covers=S` (raw body) and `DELETE notes/{id}` (purge).
    - `GET live`: WebSocket. The client sends `{"type":"auth","token"}` within 10 s and receives `ready{head}`, then `entry` frames (same `Entry` shapes as a pull). The client may send `push` frames (`request_id` + `PushUpdate`s) and gets a `push_result` with `seqs`/`head` or an `error` code; the socket stays open on a rejected push. A later `auth` frame with a refreshed token extends the connection past the 15-minute access-token expiry. Close codes: `4001` re-auth, `4002` resync, `4003` too many connections, `4004` shutting down.
    - Errors `{"error": code}`: `401 invalid_token`, `404 unknown_note`, `409 op_id_conflict | note_key_conflict | snapshot_stale | cursor_ahead`, `410 note_purged`, `413 update_too_large` (per update, 256 KiB default), `422 missing_note_key`, `507 quota_exceeded` (quota values are owned by `invite-codes`). Plus `server-hosting`'s `413` body limit (1 MiB) and `429` with `Retry-After`.
- `opaque-accounts`: short-lived access tokens plus a refresh token per device.

## Goals / Non-Goals

**Goals:**
- One sync implementation in `notebook-core`, compiled for native and wasm, with platform differences confined to a transport and a timer.
- Never lose a local edit: durable before acknowledged locally, removed from the outbox only on acknowledgment or echo.
- Deterministic tests: the whole engine runs against an in-memory fake server with a manual clock and fault injection.
- No duplicated bookkeeping. Storage owns durability, ordering, echo recognition, and key-upload state. The engine owns policy: when to push, pull, snapshot, and back off.

**Non-Goals:**
- The editor's live-document bridge (plaintext path between CodeMirror/Yjs and core). `note-editor-component` adds it on top of the replica's edit entry point.
- Tag index and search. Hosts rebuild them from change notifications.
- Credential storage (keychain), login UI, and how the refresh token is persisted. Hosts provide a session source.
- Several engines sharing one storage. One engine per storage.
- Preserving purges made after a server backup was taken, if the server is restored from it (see "Resync after a server restore").
- Offline purge (see "Purge").

## Decisions

### Module layout

```
notebook-core/src/sync/
  mod.rs          SyncEngine, SyncHandle, SyncEvent, SyncStatus, SyncConfig
  replica.rs      Replica: open documents, local edits, apply remote pages
  engine.rs       the run loop (push, pull, live, snapshot, backoff)
  transport.rs    Transport + LiveChannel traits, TransportError
  session.rs      SessionSource trait
  timer.rs        Timer trait
  backoff.rs      exponential backoff with full jitter
  fake.rs         in-memory fake server + manual timer (cfg(test) / feature "test-util")
  native.rs       reqwest + tokio-tungstenite transport, tokio timer (feature "transport-native")
  wasm.rs         fetch + web-sys WebSocket transport, gloo timer (feature "transport-wasm")
```

Native and wasm transports are behind features so `notebook-core`'s default build and its wasm32 CI check stay dependency-light. `notebook-app` enables `transport-native`; the browser bindings enable `transport-wasm`. The transports speak only the `notebook_core::protocol` types from `sync-server`.

**Alternative considered:** putting transports in the host crates. Rejected: the browser and mobile clients would each duplicate URL building, error mapping, and auth handling, which is where clients drift apart.

### Replica owns documents; engine owns I/O

`Replica<S: Storage>` holds the unlocked `VaultKey`, the storage, and a map `NoteId → NoteDocument` built at open time. For each note it unwraps the key, loads the base snapshot, then replays log entries in `LogPos` order, calling `load_snapshot` or `apply_update` by entry kind. It is synchronous and has no network code.

- `create_note(body, now)`: `NoteKey::generate`, wrap it, `NoteDocument::create`, then `Storage::create_note(id, wrapped, LocalUpdate { id: fresh UpdateId, bytes })`. The key is stored pending.
- `set_body` / `set_pinned` / `set_archived` / `set_trashed`: mutate the document, then `append_local`. If storage fails, the note's document is rebuilt from storage, so memory matches disk. `NoteDocument` has no undo, and the rebuild is what satisfies the spec's rollback guarantee.
- `notes()`, `note(id)`: materialized `Note` values for the UI, tag tree, search, and export.
- `apply_page(ChangesResponse) -> Changed`: used by the engine (see "Pull").

`UpdateId` and the protocol's `OpId` are both 16-byte ULIDs. A `From` conversion bridges them if they remain separate types.

The host shares the replica with the engine as `Rc<RefCell<_>>` on wasm and `Arc<Mutex<_>>` on native, behind a small `Shared<T>` alias. Locks are held only inside synchronous replica calls, never across an `.await`. After a local edit, the replica pokes the engine through a wake channel.

**Alternative considered:** the engine task owns documents, and every UI read or edit is a message round trip. Rejected: it makes simple reads async across the Tauri/WASM boundary and serializes editing behind network work.

### Engine is a single non-`Send` task

`SyncEngine::run(self) -> impl Future<Output = ()>` is one long-lived task. On wasm the host spawns it with `wasm_bindgen_futures::spawn_local`. On native, `notebook-app` runs it on a dedicated thread with a current-thread tokio runtime. Nothing in the engine needs `Send`, which sidesteps the `Send`-bound problem of async trait methods across native and wasm.

The host talks to it through `SyncHandle` (cloneable, `Send` on native): `wake()`, `flush() -> Result`, `purge(note) -> Result`, `set_session(...)`, `shutdown()`. These are `futures::channel` messages with oneshot replies. Events come out on one `futures::channel::mpsc` receiver of `SyncEvent::{Status(SyncStatus), NotesChanged(Vec<NoteId>), IntegrityWarning(NoteId)}`, and the host fans them out (Tauri events, Svelte stores).

Traits use native `async fn` in traits (edition 2024), with no `Send` bounds:

```rust
trait Transport { async fn push(&self, PushRequest) -> Result<PushResponse>;
                  async fn changes(&self, after: u64) -> Result<ChangesResponse>;
                  async fn upload_snapshot(&self, NoteId, covers: u64, &[u8]) -> Result<()>;
                  async fn purge(&self, NoteId) -> Result<()>;
                  async fn connect_live(&self, token) -> Result<impl LiveChannel>; }
trait LiveChannel { async fn next(&mut self) -> LiveEvent; } // Ready{head} | Op | Purged | Closed(code) | Error
trait Timer { fn now(&self) -> Millis; async fn sleep(&self, d: Duration); }
trait SessionSource { fn access_token(&self) -> Option<Token>; async fn refresh(&self) -> Result<(), RefreshError>; }
```

`futures::select!` drives the loop over: the wake channel, host commands, live events, and the next timer deadline.

### Pull: one vault-wide cursor, one transaction per page

The cursor is storage's `ServerSeq` (0 = nothing seen). `changes?after=cursor` is fetched page by page while `more` is true. For each page, the replica:

1. Deletes every note named by a `Purge` entry (`delete_note`, a no-op for unknown notes) and drops its document. Each is its own transaction, done before the batch. A crash before step 3 re-delivers the tombstones, because the cursor hasn't moved.
2. Builds a `RemoteBatch`: wrapped keys from `notes[].key`, snapshot entries from `notes[].snapshot`, and update entries from `Update` entries with their `OpId` as `UpdateId` (an entry's `note_key`, when present, is added to the batch's keys). Then calls `Storage::apply_remote`, which commits entries and `cursor` atomically.
3. Only after the commit, applies the same entries to in-memory documents (opening documents for new notes), and computes changed notes by comparing each touched note's materialized `Note` before and after.

If step 3 fails (for example, a key that won't unwrap), memory is behind disk for that note only. It's handled as in "Bad remote input" and the note is rebuilt from storage on the next open.

Live `entry` frames go through the same path as a one-entry page whose cursor is the frame's seq. Frames at or below the cursor are ignored. On every (re)connect the engine waits for `ready{head}`, then pulls from its cursor until `more` is false, then applies live frames. That is the server's documented no-gap handshake.

### Echoes: storage owns it, the engine just observes

Storage already recognises a returning `UpdateId`, marks it acknowledged, and doesn't store it twice. The engine adds no second record of in-flight IDs. "Own updates are not reported as remote changes" falls out of step 3: applying an update the document already contains doesn't change its materialized `Note`, so it isn't in `NotesChanged`. Comparing materialized state rather than tracking IDs also keeps no-op remote updates (e.g. a duplicate after server compaction) out of notifications.

### Push

Batched in outbox order (`Storage::outbox()`). While the live socket is up, a batch goes as a `push` frame and its `push_result` is matched by `request_id`. Otherwise, or if no result arrives within 30 s (then the socket is treated as broken and reconnected), it goes as `POST push`. Both paths run the same server store function and return the same error codes, so the engine maps a `push_result.error` and an HTTP error body to one `PushError` and handles them identically. A retry after a lost result reuses the same op IDs, so switching paths mid-retry is safe. A batch is capped by count (default 100) and by encoded size (default 512 KiB of ciphertext, about 700 KiB as base64 JSON, under the 1 MiB body limit). Edits made in quick succession are coalesced with a debounce (default 300 ms, at most 2 s after the first unpushed edit).

Wrapped keys: for every note in `Storage::pending_keys()`, the first `PushUpdate` for that note in the batch carries `note_key`. After a `200`, the engine calls `acknowledge(id, seq)` for each entry in response order, then `mark_key_uploaded` for each key that was sent. A pending key whose note has no outbox entries can't occur, because `create_note` always queues the initial update.

Each cycle pulls before it pushes, so tombstones and echoes usually arrive before a push could hit them. Rejections (the server rolls back the whole batch):

- `507 quota_exceeded` → attention `QuotaFull`. Pushes pause and are retried every 10 minutes, or sooner on `wake()` (e.g. after the user purges notes). Pull and snapshot upload continue, because the server always accepts a snapshot whose net size change is ≤ 0, so compaction can free space.
- `413 update_too_large`, or a body-limit `413`, on a multi-entry batch → split the batch in half and retry. On a single entry → attention `UpdateTooLarge(note)` and push pauses.
- `410 note_purged` → the note was purged elsewhere. If the batch has one note, delete it locally (as for a tombstone) and continue. Otherwise retry the batch one note per request to find it. The same code on a snapshot upload also deletes the note locally.
- `404 unknown_note` or `422 missing_note_key` → the server has no registration for a note this client marked uploaded. That only happens after a server restore, so the engine runs a resync (below). `404` on a snapshot upload is handled the same way.
- `409 op_id_conflict` / `note_key_conflict` → can only come from a client bug or ID collision. Raise an integrity warning for the note and pause push. This is not expected in practice.

**Why both paths:** the socket saves a request per typing burst while live. HTTP is still needed for polling mode and for clients whose socket is refused (`4003`).

### Bootstrap and compaction catch-up

Bootstrap is just a pull from cursor 0. Because every page commits with its cursor, an interrupted bootstrap resumes from the last stored page rather than starting over. Local outbox entries survive and are pushed after the pull.

When a pull's `NoteEntry` carries a snapshot for a note the client already has, storage appends it to the log and the replica calls `load_snapshot` on the **existing** document, which merges. Unpushed local edits stay in the document and in the outbox.

### Snapshot policy

Per note, the replica counts log entries since the base, from `load_note` at open time plus entries added since. When the count exceeds `snapshot_threshold` (default 200), the note has no outbox entries, and the engine is caught up (last pull had `more == false`), the engine:

1. Takes `snapshot()` and records the note's current max `LogPos` and the cursor `S`.
2. Uploads it with `covers=S`. Correctness: the document holds every server update for the note up to `S` and nothing unpushed. Any held (pending) structs are included in the snapshot (see `crdt-note-documents`).
3. On `204`, calls `Storage::compact(note, snapshot, through = recorded LogPos)`. Entries stored after step 1, and any outbox entries, are kept by storage.

Two devices snapshotting the same note concurrently: the server accepts the first and rejects the second with `409 snapshot_stale`. On `snapshot_stale` or `413`, the note's trigger moves up by another threshold, so it retries only after more entries arrive. `410 note_purged` deletes the note locally, and `404 unknown_note` triggers a resync (see "Pull" and "Resync after a server restore"). One snapshot is uploaded per sync cycle, so it never delays pulls much. The engine may also compact locally without uploading when the note has only remote entries: the merged remote snapshot plus the entries after it can be folded into the base. That is the same `compact` call and needs no server round trip.

### Purge

Purge is online-only. `SyncHandle::purge(note)` checks that the note is trashed, then calls `DELETE notes/{id}`. On `204`, or `404` for a note that was never pushed, the replica calls `delete_note`, which also drops its outbox entries, and drops the document. A network failure returns an error and changes nothing.

**Alternative considered:** queue the purge durably in the outbox. Rejected: storage's outbox holds CRDT updates only, a pending purge would need its own durable record, and "delete permanently" is a rare, deliberate action where "try again when online" is acceptable.

### Live mode, polling, and backoff

States: `Connecting → CatchingUp → Live`, with `Offline` on failure. While `Offline`, or when the socket can't open, the engine pulls every `poll_interval` (default 30 s) and retries the socket with exponential backoff: base 1 s, factor 2, cap 60 s, full jitter (random in [0, delay]) via `getrandom`. Backoff resets after the socket has been open for 10 s. A `429` sets the next attempt to at least `Retry-After`.

Close codes: `4001` → refresh the session and reconnect immediately. `4002` → pull, then reconnect. `4003` → stay in polling mode with backoff. `4004` → normal backoff. While live, the engine also pulls every 5 minutes as a safety net. The server's 30 s pings keep the native socket honest. The browser socket relies on close events plus that pull.

### Session handling

The HTTP transport attaches the access token from `SessionSource`. The socket sends it in the first `auth` frame. Access tokens live 15 minutes, so when a token is within 60 s of expiry the engine refreshes it and sends another `auth` frame on the open socket instead of reconnecting. A `401`, close `4001`, or failed re-auth triggers one `refresh()` (single-flight, since only the engine task calls it) and one retry. Failure → attention `SignInRequired`, and the engine idles with the outbox intact. `SyncHandle::set_session` (after the host signs in again) resumes.

### Bad remote input

Page step 3 maps document errors:

- `Crypto` / `Malformed`, or a wrapped key that won't unwrap → the in-memory note is left as it was. Emit `IntegrityWarning(note)` and bump a sticky `integrity_warnings` count. The cursor has already advanced with the commit, so sync continues.
- `UnsupportedFormat` → the entry is stored (it was committed) but not applied, and attention becomes `UpgradeRequired`. A newer client replays it from storage on open.

The bad entry stays in storage. At replica open, the same rules apply when replaying, so a tampered entry is skipped consistently. It is removed at the note's next compaction, whose snapshot only reflects what applied.

### Resync after a server restore

`409 cursor_ahead` on pull (or `404`/`422` for a note this client registered) means the server lost history this client has seen: it was restored from an older backup. The protocol says the client must bootstrap again. Storage refuses to move its cursor backwards, and plain re-bootstrapping would lose whatever the server no longer has. The in-memory replica, though, holds every note's full merged state, including its unpushed edits. So resync rebuilds storage from memory:

1. Status `Syncing`, attention none. For each note in the replica, the engine collects its wrapped key (from `load_note`) and a full-state encrypted update from its document.
2. `Storage::clear()` and reopen an empty store for the vault through a host-supplied `reopen` callback (`MemoryStorage::new` or `SqliteStorage::open` with the same path).
3. For each note, `create_note(id, wrapped_key, LocalUpdate { fresh id, full_update })`. Every key is pending again and every note has one outbox entry.
4. Normal sync resumes from cursor 0. The pull bootstraps the server's restored state and merges it into the existing documents, then the push re-registers notes the server lost (their keys are pending) and re-sends state it's missing. Full-state updates the server already covers are CRDT no-ops.

This needs a full-state update from `NoteDocument`. Its snapshot is already a full-state update in format 1, only encrypted as `CrdtSnapshot`, so task 3.5 adds `NoteDocument::full_update() -> EncryptedUpdate`, the same bytes encrypted as `CrdtUpdate`. That's a new method, not a change to `note-documents` requirements.

Trade-off: a note purged after the backup was taken comes back from the restored server, and is re-uploaded by any device that still has it. The user can purge it again. A resync that fails partway leaves an empty or partial store and is simply rerun on the next start, from the in-memory replica if it is still open, or, after a crash, by bootstrapping from the server alone (local edits made since the last push to the restored server are then lost). That narrow window is accepted for an operator-emergency event.

**Alternative considered:** stop and make the user clear local data. Rejected: `sync-protocol` defines `cursor_ahead` as "bootstrap again", and asking the user to clear data would discard exactly what the server lost.

### Status model

```rust
struct SyncStatus { state: Synced | Syncing | Offline, pending: usize,
                    attention: Option<SignInRequired | UpgradeRequired | QuotaFull
                                     | UpdateTooLarge(NoteId)>,
                    integrity_warnings: u32 }
```

`pending` is the outbox length. `Synced` = outbox empty, caught up, and connected (live, or last poll succeeded). `Syncing` = a request in flight, or outbox non-empty while connected. `Offline` = last attempt failed. Events are emitted only when the value changes.

## Risks / Trade-offs

- [Wire details may shift while `sync-server` is implemented] → The engine uses `notebook_core::protocol` types directly, and the fake server is built on them, so drift fails to compile.
- [Single update over 256 KiB (e.g. a huge paste) blocks pushing] → Surfaced as `UpdateTooLarge`. Chunking edits is a later change.
- [Whole vault decrypted in memory] → Already accepted by `browser-web-client` at personal scale. Same on native.
- [Clients snapshot independently, so a losing upload is rejected as `snapshot_stale`] → Bounded by the threshold. It costs one wasted upload, not correctness.
- [Skipping tampered updates hides lost data] → Integrity warnings are surfaced. Tampered data couldn't be recovered anyway.
- [Purge needs a connection] → Clear error, and the note stays trashed. Rare action.
- [Resync after a server restore can resurrect notes purged after the backup] → Documented. The user can purge them again.
- [Purge tombstones and `410` responses aren't authenticated, so a malicious server can delete any note on every device, unpushed edits included] → Accepted. An honest purge can reach a device that never saw the note trashed, so the client can't require that. Flushing before purge and a local quarantine of purged notes are possible later protocol changes.
- [A server that keeps answering `404`/`422` would make the client rebuild storage repeatedly] → Resyncs after the first are backed off until a push succeeds.
- [Native mutex shared by UI and engine threads] → Held only for synchronous replica calls. Decrypt/apply runs per page, and server pages are capped at ~2 MiB.
- [Wasm transport has no automated runtime test in CI] → Engine logic is shared and tested through the fake transport. The wasm transport is thin and covered by the wasm32 build check, plus the browser client's end-to-end testing later.

## Migration Plan

None. No sync data exists yet. Storage schema is owned by `local-sqlite-storage`, server schema by `sync-server`.

## Open Questions

- Default tuning (snapshot threshold, poll interval, batch size, debounce) may change after real use. These are `SyncConfig` fields, so changing them doesn't affect specs or structure.
