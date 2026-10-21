# Tasks

## 1. Prerequisites and dependencies

- [ ] 1.1 Confirm `opaque-accounts` is archived and identify its `AuthSession` extractor, the token-lookup function behind it, `sessions::is_active`, its last migration number, and its error-body shape; if the token lookup is not callable with a plain token string, factor it out in `notebook-server` without changing `accounts` behavior; verify the existing `accounts` tests still pass
- [ ] 1.2 Add `serde` (derive) and `base64` to `notebook-core`, and `dashmap` plus axum's `ws` feature to `notebook-server` (and `tokio-tungstenite` as a dev-dependency); verify `cargo build -p notebook-server` and `cargo build -p notebook-core --target wasm32-unknown-unknown` succeed

## 2. Shared protocol types (`notebook_core::protocol`)

- [ ] 2.1 Implement `OpId`, the base64 `Bytes` newtype, `PushRequest`/`PushUpdate`/`PushResponse`, `ChangesResponse`/`NoteEntry`/`Snapshot`, the tagged `Entry` (`update` / `purge`), the live frames (`auth`, `push`, `ready`, `entry`, `push_result` with seqs or error), the `ErrorCode` enum with the design's codes, and the close-code constants; verify serde round-trip tests and golden JSON tests that pin every field name, tag, and error-code string, plus a test that malformed base64 fails to deserialize
- [ ] 2.2 Document the module (endpoint table, cursor and pagination rules, close codes) in its rustdoc; verify `cargo doc -p notebook-core --no-deps` builds without warnings

## 3. Schema and store

- [ ] 3.1 Add migration `0002_sync.sql` (numbered after the accounts migration) with `vaults.sync_head`, `sync_notes`, `ops`, `snapshots` (blob column `payload` in both), and their indexes, per the design; verify a migration test on a fresh database asserts the tables, columns, and the unique `(vault_id, op_id)` index, and the existing idempotent re-migration test still passes
- [ ] 3.2 Implement the `sync::store` push function: `BEGIN IMMEDIATE`, per-update size check against `sync_max_update_bytes`, op-ID dedup and conflict, in-batch duplicate rejection, purged-note rejection, note-key registration and comparison, sequence assignment from `sync_head`, net payload-byte delta with a marked quota hook point, all-or-nothing rollback; add `sync_max_update_bytes` (default 262144) to `Config`; verify unit tests for increasing seqs, retry returning the same seq, op-ID conflict, missing and conflicting note keys, purged note, oversize and exactly-at-limit updates, empty batch, a mid-batch failure storing nothing and leaving `sync_head` unchanged, and the config default and env override
- [ ] 3.3 Implement the snapshot store function: registered/purged/`covers ≤ head`/stale checks, replace snapshot, delete the note's ops `≤ covers`, net delta with the hook point; verify unit tests for the 3/5/9 compaction example, stale and equal `covers`, `covers` above head, unknown and purged notes, and other notes untouched
- [ ] 3.4 Implement the purge store function: delete ops and snapshot, null the wrapped key, assign `purged_seq` from `sync_head`, idempotent repeat, `404` for unknown notes; verify unit tests for data removal, tombstone seq, repeat purge changing nothing, and a later push for the note failing with `note_purged`
- [ ] 3.5 Implement the paginated changes query in one read transaction, merging ops, snapshots, registrations, and tombstones by key with the 2 MiB budget, same-key extension, oversize-item-alone, `cursor`/`more` semantics, and `cursor_ahead`; verify unit tests for incremental pull, nothing-new, new-note keys, client behind and past a compaction, bootstrap excluding purged notes, a multi-page bootstrap yielding every item exactly once, an oversize single item, and a property-style test that random interleavings of pushes, snapshots, and purges followed by paged pulls from random cursors reproduce, per note, the full set of live updates or a covering snapshot

## 4. HTTP endpoints

- [ ] 4.1 Implement `SyncError` → status + `{"error": code}` mapping (no payload echo, generic `500`), and wire `POST /sync/v1/push`, `GET /sync/v1/changes`, `PUT /sync/v1/notes/{id}/snapshot` (raw body, `covers` query), and `DELETE /sync/v1/notes/{id}` behind `AuthSession` into `app::router()`; verify router tests with `oneshot` for every status and code in the design's error table, `401` without or with an expired token, and that a snapshot larger than `max_body_bytes` gets `413`
- [ ] 4.2 Add an isolation test with two accounts pushing the same note ID, and a byte-identity test pushing arbitrary non-ciphertext bytes and pulling them back; verify both pass
- [ ] 4.3 Add a logging test that pushes, uploads a snapshot, and pulls with marker bytes in payloads and asserts the captured log contains neither the markers nor their base64 forms; verify it passes

## 5. Live channel

- [ ] 5.1 Implement the per-vault `VaultHub` (write mutex held across commit and broadcast, `broadcast` channel of capacity 256, connection counter capped at 16) on `AppState`, and make the push and purge store calls broadcast their entries after commit while holding the mutex; verify a unit test that concurrent pushes are broadcast in strictly increasing seq order and a failed push broadcasts nothing
- [ ] 5.2 Implement `GET /sync/v1/live`: `Origin` check against `public_url` (`403`), upgrade, 10 s auth deadline, first-frame token auth via the token lookup (never logged), subscribe-then-read-head-then-`ready`, `entry` forwarding, `4002` on lag, `4003` over the cap, 30 s pings with close after two misses; verify integration tests on an ephemeral port with `tokio-tungstenite` for ready-with-head, delivery of another client's push and purge, no cross-vault delivery, the no-gap pull-after-ready sequence, silent-connection close, bad token close, foreign-origin `403`, the connection cap, and lag closing with `4002` (using a test-only small channel capacity)
- [ ] 5.3 Implement socket `push` frames through the same store function, with a per-frame `KeyedLimiter` check, `push_result` with seqs or error code, socket kept open on rejected pushes, and close on push-before-auth or unparseable frames; verify tests that a socket push is acknowledged and relayed to other connections, an oversize socket push returns an error `push_result` without storing or relaying and the socket stays open, and a push before auth closes with `4001`
- [ ] 5.4 Implement re-auth frames (same vault only, deadline moved to the new expiry), close with `4001` at token expiry, `is_active` re-check per ping tick, and `4004` to open sockets on graceful shutdown; verify tests using paused tokio time for expiry close, refresh extending the connection, revoked session closing within one ping interval, and the existing shutdown test still finishing within the grace period with a socket open

## 6. Documentation

- [ ] 6.1 Add a sync section to `docs/deployment.md`: `sync_max_update_bytes`, WebSocket proxying (Caddy passes upgrades by default; note `Origin` must match `public_url`), the single-process requirement, metadata the server sees, and that users must re-bootstrap after a database restore; verify the sample `notebook.toml` with `sync_max_update_bytes` set still runs `notebook-server --config <file> migrate`

## 7. Integration checks

- [ ] 7.1 Run an end-to-end test that two clients built on `notebook_core::protocol` and real `NoteDocument`s (if `crdt-note-documents` is archived; otherwise opaque test bytes) create, edit, snapshot, and purge notes over HTTP and the live channel, and a third client bootstraps to identical state; verify it passes
- [ ] 7.2 Run `scripts/check.sh` and verify it exits 0 (fmt, clippy `-D warnings`, workspace tests, wasm32 core build)
- [ ] 7.3 Run `openspec validate sync-server --strict` and verify it passes

## Workflow follow-up

- Run the `security-analyst` agent over the sync endpoints and live channel before archiving.
- Archive the change before `invite-codes` starts, since `invite-codes` hooks its quota check into the `sync::store` write functions.
