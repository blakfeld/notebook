# Design

## Context

See proposal.md (Why). Requirements are in `specs/local-storage/spec.md` and `specs/vault-crypto/spec.md`. The other five deltas only remove requirements.

`Workspace` (`workspace/mod.rs`) imports `Replica`, `Shared`, and change types from `crate::sync`. `Replica` lives in `sync/replica.rs`, writes local updates through `Storage::{create_note, append_local}` (which also enqueue the outbox), and exposes `pending()` (outbox length). The sync engine owns snapshot policy (client-driven snapshots once a note's log is long) and calls `Storage::compact`, which refuses to drop outbox entries.

## Goals / Non-Goals

**Goals:**
- Delete everything only sync and the server needed, with tests and docs moving in step.
- Keep the core wasm-buildable without any transport.
- Never lose a log entry in the storage upgrade, including entries that were never acknowledged.

**Non-Goals:**
- Removing the CRDT. Yrs documents stay: they are the storage format, they back editor undo and binding, and they keep future sync possible.
- Removing `LocalUnlock`/Argon2id. They are needed for the legacy upgrade path until a later decision ends it.
- Changing the note-editor or note-documents specs. Their "remote edits" requirements describe document behavior that stays true and tested, even though no remote source exists today.

## Decisions

### 1. `replica` module

`git mv sync/replica.rs replica/mod.rs`, plus `Shared` and the change-notification types `Workspace` uses. `sync/` is then deleted whole. `Replica::pending()` goes away along with the outbox. Public paths change from `notebook_core::sync::Replica` to `notebook_core::replica::Replica`, and the app updates its imports.

### 2. Storage trait and schema upgrade

The trait loses `outbox`, `acknowledge`, `pending_keys`, `mark_key_uploaded`, `cursor`, `apply_remote`, and `clear`. `LocalUpdate` loses its `UpdateId`, and `LogEntry` loses its server sequence. `compact(id, through, snapshot)` drops every entry up to `through`. SQLite schema vN+1 runs in one transaction: drop the outbox and cursor tables, rebuild the log table without the `update_id` and `server_seq` columns (`INSERT … SELECT` in log order), and drop the pending-key flag. The conformance suite drops the removed cases and covers the new "Local updates are durable" and compaction scenarios against both stores.

### 3. Local compaction policy

`Replica` takes over the threshold the engine used: after an append, if a note has more than N log entries since its base, take `doc.snapshot()` and `compact` through the last entry. This is synchronous, on the same path as the append, and N is cheap enough at personal scale. Keep the engine's constant, so behavior matches what users had.

### 4. Features and CI

`notebook-core` features: keep `conformance`, and drop `transport-native`, `transport-wasm`, and `test-util`. CI's `wasm` job keeps `cargo build` and `cargo clippy -p notebook-core --target wasm32-unknown-unknown -- -D warnings` with no features. `CLAUDE.md` definition-of-done item 4 and `scripts/check.sh` change to match.

*Alternative:* drop wasm32 entirely. Rejected for now: it costs one CI job, and the pivot is "web later, maybe", so keeping the core portable avoids a cfg-unwinding project later. Revisit if it ever blocks a dependency.

### 5. Order inside the change

1. Move `Replica`. 2. Delete the server crate and the app's server-backed tests. 3. Delete auth, protocol, and sync, and drop the features. 4. Storage trait and schema upgrade, plus local compaction. 5. Crypto removals. 6. Dependencies, docs, CI, agents. Each step keeps `cargo clippy --workspace --all-targets` green, so a reviewer can follow the commits.

## Risks / Trade-offs

- [Schema upgrade bug loses unacknowledged edits] → The upgrade test seeds a store with acknowledged, unacknowledged, and compacted notes from the previous layout and compares the decoded `Note` of every document before and after. `local-only-desktop` users should back up first (`vault-backup`); the README says so.
- [Deleting the server removes the only off-device copy for anyone still relying on it] → The upgrade is one-way already (`local-only-desktop`). The README tells operators to export or back up before shutting a server down.
- [Removing agent definitions the user still wants] → Only `mobile-developer` is deleted. It's recoverable from git if mobile returns.

## Migration Plan

Storage upgrades in place on first open. There is no server migration: operators stop the service and delete its data after confirming every device has upgraded and backed up.
