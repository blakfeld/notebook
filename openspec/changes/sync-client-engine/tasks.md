# Tasks

Prerequisites: `crdt-note-documents`, `local-sqlite-storage`, `opaque-accounts`, and `sync-server` are implemented (`NoteDocument`, `Storage` + `MemoryStorage`, sessions, and `notebook_core::protocol` exist).

## 1. Module skeleton and dependencies

- [ ] 1.1 Add `notebook-core/src/sync/` with the module layout from design.md (empty `mod.rs`, `replica.rs`, `engine.rs`, `transport.rs`, `session.rs`, `timer.rs`, `backoff.rs`) and declare `pub mod sync` in `lib.rs`. Verify `cargo build -p notebook-core` and `cargo build -p notebook-core --target wasm32-unknown-unknown` succeed
- [ ] 1.2 Add `futures` as a core dependency and declare the features `test-util`, `transport-native` (`reqwest` with rustls, `tokio-tungstenite`, `tokio` time), and `transport-wasm` (`gloo-net`, `gloo-timers`, `wasm-bindgen-futures`, `web-sys` WebSocket) in the workspace and core manifests. Verify default, `--features transport-native`, and `--target wasm32-unknown-unknown --features transport-wasm` builds succeed

## 2. Contracts and test fixtures

- [ ] 2.1 Define the `Transport`, `LiveChannel` (`Ready{head}`, `Entry`, `PushResult{request_id, seqs|error}`, `Closed(code)`, `Error`; send `Auth` and `Push` frames), `Timer`, and `SessionSource` traits and `TransportError` (network, `invalid_token`, rate-limited with retry-after, and each `sync-protocol` error code: `unknown_note`, `op_id_conflict`, `note_key_conflict`, `snapshot_stale`, `cursor_ahead`, `note_purged`, `update_too_large`/body-limit 413, `missing_note_key`, `quota_exceeded`) over the `notebook_core::protocol` types, and the `OpId` ↔ `UpdateId` conversion. Verify it compiles natively and for wasm32, and a unit test round-trips an ID through the conversion
- [ ] 2.2 Implement the in-memory fake server and `ManualTimer` in `sync/fake.rs` (behind `cfg(test)` or `test-util`) following the `sync-server` design: per-vault seq, all-or-nothing idempotent push with note-key registration, paged `changes` with snapshots/keys/tombstones, snapshot upload with `snapshot_stale` and compaction, purge with `410` on later pushes, `cursor_ahead`, and the live channel (`auth` within 10 s, `ready{head}`, `entry` frames including echoes to the pusher, `push` / `push_result`, re-`auth` extending expiry, close codes `4001`–`4004`). Add fault injection: drop request, drop response, disconnect, 401, 429 with retry-after, 413, 507, forced compaction, server restore. Verify unit tests for the fake's idempotency, pagination, and compaction behavior pass

## 3. Replica: local edits and remote application

- [ ] 3.1 Implement `Replica::open`: for each stored note, unwrap the key, load the base, and replay log entries by kind, skipping entries that fail (integrity) or have an unknown format (upgrade required). Verify a test that writes notes, reopens a new replica on the same `MemoryStorage`, and gets identical `notes()`, plus a test with one tampered stored entry that still opens
- [ ] 3.2 Implement `create_note` (fresh note key, wrapped, `Storage::create_note` with the initial update, key pending) and `set_body` / `set_pinned` / `set_archived` / `set_trashed` via `append_local`, rebuilding the note from storage on a storage error. Verify tests for the "Local edits are durable" scenarios, using a storage wrapper that injects a failure, and that a new note appears in `pending_keys()`
- [ ] 3.3 Implement `apply_page`: delete purged notes, build and commit the `RemoteBatch` via `apply_remote`, then apply entries to documents (snapshot vs update by kind) and report notes whose materialized `Note` changed. Map document errors to integrity warnings or upgrade-required. Verify tests for "Echo of own push" (no change reported), "Tampered update from the server", "Newer client wrote a note", compaction catch-up merging into a document with an unpushed edit, and purge tombstones
- [ ] 3.4 Implement the per-note entry counter and local compaction (`snapshot()` + `Storage::compact` through the recorded max `LogPos`). Verify a test that the note state is unchanged after compaction and that an outbox entry and an entry stored after the snapshot was taken both survive
- [ ] 3.5 Add `NoteDocument::full_update() -> EncryptedUpdate` in the `document` module (the snapshot's format-1 full-state bytes encrypted as `CrdtUpdate`) and `Replica::export_for_resync()` returning each note's wrapped key and full update. Verify a test that applying a note's full update to an empty document for that note reproduces its state, and that it decrypts only as an update

## 4. Engine loop

- [ ] 4.1 Implement `SyncEngine`, `SyncHandle`, `SyncEvent`, `SyncStatus`, and `SyncConfig` with defaults from design.md, and the run loop skeleton selecting over wake, commands, live events, and timer deadlines. Verify a test that the engine starts, emits an initial status, and shuts down on `shutdown()`
- [ ] 4.2 Implement paged pull from the stored cursor (bootstrap = cursor 0). Verify against the fake server: "New device sees the whole vault", "Interrupted bootstrap" (resumes from the last stored page), "Remote edit arrives", and "Crash during pull" (drop the engine mid-page, restart on the same storage)
- [ ] 4.3 Implement push over both paths (`push` frame with `request_id` while live, `POST push` otherwise, 30 s result timeout → reconnect and retry over HTTP), mapping `push_result.error` and HTTP error bodies to one `PushError`: pull-before-push cycle, outbox order, count/size caps, debounce, `note_key` on the first entry per pending-key note, `acknowledge` + `mark_key_uploaded` after `200`, `413` splitting, `507` pause/retry, `410 note_purged` handling (per-note retry for multi-note batches), `404 unknown_note`/`422 missing_note_key` → resync, `409` conflicts → integrity warning. Verify tests for "Push fails and is retried", "Acknowledgment lost", "Echo arrives before the acknowledgment", "Key upload interrupted", "Typing burst", "Quota full", and "Offline edit to a note purged elsewhere"
- [ ] 4.4 Implement live mode: auth frame, wait for `ready`, catch-up pull over HTTP, apply `entry` frames above the cursor, close-code handling (`4001`–`4004`), periodic safety pull. Verify "Edit appears without polling" with two engines on one fake server, and "Server asks to resynchronize"
- [ ] 4.5 Implement backoff with full jitter, fallback polling, `429` retry-after, and backoff reset. Verify with the manual timer: "Server unreachable" (growing, capped delays), "Rate limited", and "Connection restored"
- [ ] 4.6 Implement session handling: re-`auth` frame on the open socket 60 s before token expiry, one refresh and retry on 401 / close `4001` / failed re-auth, `SignInRequired` on refresh failure, resume on `set_session`. Verify "Access token expired", "Live connection outlives a token", and "Refresh token revoked"
- [ ] 4.7 Implement the snapshot policy (threshold, caught up, no outbox entries for the note, one per cycle, `covers` = cursor, compaction on `204`, trigger raised on `409`/`413`/`410`). Verify "Threshold reached", "Snapshot rejected" (via `snapshot_stale`), and "Snapshot does not change content"
- [ ] 4.8 Implement `purge` (trashed only, online `DELETE`, `204`/`404` → `delete_note`). Verify "Purge propagates", "Purging a live note", "Purge while offline", and "Never-synced note"
- [ ] 4.9 Implement resync on `cursor_ahead` (and `404`/`422` on push or snapshot): collect wrapped keys and full-state updates from the replica, `clear()` storage, reopen via the host `reopen` callback, re-create every note with its full update, and resume from cursor 0. Verify "Server restored from backup" and "Edits the restored server lost" with the fake's restore fault
- [ ] 4.10 Implement status computation and change-only emission, `NotesChanged` batching, and `flush()`. Verify "Offline with pending edits", "Back in sync", "Batch of remote edits", "Local edit", "Flush before backgrounding", and "Flush while offline"
- [ ] 4.11 Add a multi-client convergence test: three engines on one fake server with random edits, random disconnects, dropped responses, duplicate deliveries, and forced compactions, run with a fixed seed. Verify all replicas end with identical `notes()` and empty outboxes

## 5. Platform transports

- [ ] 5.1 Implement the native transport and tokio timer in `sync/native.rs` (`/sync/v1` endpoints, bearer auth, `{"error": code}` and `Retry-After` mapping, raw-body snapshot upload, WebSocket with auth frame and pong replies). Verify unit tests for status-code-to-`TransportError` mapping
- [ ] 5.2 Add an integration test in `notebook-server` (dev-dependency on `notebook-core` with `transport-native`) that starts the real server on an ephemeral port with a temp database, creates a user, and runs two native engines (each with `MemoryStorage`) editing the same note concurrently. Verify both converge, a pushed update is stored once after a forced retry, and a purge on one engine removes the note from the other
- [ ] 5.3 Implement the wasm transport and gloo timer in `sync/wasm.rs` (fetch, browser WebSocket with auth frame, same error mapping). Verify `cargo build -p notebook-core --target wasm32-unknown-unknown --features transport-wasm` and clippy with that feature pass, and add that build to the CI wasm job

## 6. Documentation and checks

- [ ] 6.1 Write rustdoc for the `sync` module's public API (host responsibilities: spawning the run future, sharing the replica, providing a `SessionSource`, consuming events). Verify `cargo doc -p notebook-core --no-deps` builds without warnings
- [ ] 6.2 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` (native features), and `cargo test --workspace`. Verify all pass

## Workflow follow-up

- Run the security-analyst review (touches keys, sync, and session handling) and the code-reviewer before archiving.
- Archive the change, then update TODO.md item 9.
