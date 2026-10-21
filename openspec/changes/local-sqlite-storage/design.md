# Design

## Context

See proposal.md (Why). `notebook-core` has `note` (`NoteId`, `Timestamp`), `crypto` (`VaultId`, `VaultKey::wrap_note_key`, `NoteKey`), and `tags`. It forbids `unsafe`, and CI builds it for `wasm32-unknown-unknown` with default features. `crdt-note-documents` (in progress) emits `EncryptedUpdate` / `EncryptedSnapshot` byte strings. Snapshots are full-state Yjs v1 updates, so loading a snapshot on top of more state is idempotent, and so is applying any update twice. That property lets this design keep the storage rules simple.

The server (`notebook-server`) already uses `sqlx` 0.9 with SQLite, which pulls `libsqlite3-sys` 0.37 into the workspace lockfile. Cargo allows only one crate with `links = "sqlite3"` per resolve, so any client-side SQLite binding has to share that exact `libsqlite3-sys` version.

Downstream: `sync-client-engine` drives every write (outbox, acks, remote batches, compaction). The Tauri app picks the database path and opens the store after unlock. The browser uses the in-memory store and persists nothing. Requirements are in `specs/local-storage/spec.md`.

## Goals / Non-Goals

**Goals:**
- One small, synchronous `Storage` trait whose every multi-row write is atomic. The sync engine never has to reason about half-applied batches.
- Identical behavior from the SQLite and in-memory implementations, enforced by one shared test suite.
- Nothing plaintext about note content ever touches disk.

**Non-Goals:**
- Policy: when to push, pull, snapshot, or purge. That's the sync engine's job.
- Opening `NoteDocument`s, unwrapping keys, or building the tag/search index. Storage hands back bytes, and consumers do the rest.
- Keychain storage of refresh tokens and key bundles (app shell).
- Encrypting the database file itself (SQLCipher). Rows are already ciphertext; see Risks.
- Multi-vault-at-once on one device. A device database holds one vault at a time.

## Decisions

### Placement: trait in core, SQLite in its own crate

```
crates/notebook-core/src/storage/
  mod.rs        Storage trait, types, StorageError
  memory.rs     MemoryStorage
  conformance.rs  generic test suite (behind `cfg(any(test, feature = "conformance"))`)
crates/notebook-storage-sqlite/
  src/lib.rs    SqliteStorage
  src/schema.rs migrations
  tests/        runs the conformance suite + on-disk-only tests
```

`notebook-core` gets no new dependencies and stays wasm-clean. The SQLite crate depends on `notebook-core` (with `features = ["conformance"]` as a dev-dependency) and `rusqlite`.

**Alternative considered:** a `sqlite` cargo feature on `notebook-core`. Rejected. It's easy to turn on by accident in the wasm build, and it makes every `cargo test -p notebook-core` compile SQLite.

### `rusqlite`, not `sqlx`

`rusqlite` with `bundled`, at the release whose `libsqlite3-sys` is 0.37 (rusqlite 0.39 at time of writing; task 1.1 confirms the version against the lockfile). It is synchronous, has no runtime dependency, and cross-compiles cleanly to iOS and Android with the bundled amalgamation. The trait is synchronous (next decision), so an async driver would only add a runtime and a worker thread.

**Alternative considered:** reuse the workspace `sqlx`. That avoids the shared-`links` coupling. But sqlx is async-only and tokio-bound, which would force an async trait into a core that also compiles for wasm. Its compile-time-checked queries also need a database at build time or an offline cache, which is overkill for six tables.

If the two ever need different `libsqlite3-sys` versions, both crates must be bumped together. That's a workspace-wide lockstep, noted in Risks.

### Synchronous trait

```rust
pub trait Storage {
    fn vault_id(&self) -> VaultId;
    fn list_notes(&self) -> Result<Vec<NoteId>>;
    fn load_note(&self, id: NoteId) -> Result<Option<StoredNote>>;
    fn create_note(&mut self, id: NoteId, wrapped_key: &[u8], initial: LocalUpdate) -> Result<()>;
    fn append_local(&mut self, id: NoteId, update: LocalUpdate) -> Result<()>;
    fn outbox(&self) -> Result<Vec<OutboxEntry>>;
    fn acknowledge(&mut self, update: UpdateId, seq: ServerSeq) -> Result<()>;
    fn pending_keys(&self) -> Result<Vec<(NoteId, Vec<u8>)>>;
    fn mark_key_uploaded(&mut self, id: NoteId) -> Result<()>;
    fn cursor(&self) -> Result<ServerSeq>;
    fn apply_remote(&mut self, batch: RemoteBatch) -> Result<()>;
    fn compact(&mut self, id: NoteId, snapshot: &[u8], through: LogPos) -> Result<()>;
    fn delete_note(&mut self, id: NoteId) -> Result<()>;
    fn clear(self) -> Result<()> where Self: Sized;
    fn setting(&self, key: &str) -> Result<Option<String>>;
    fn set_setting(&mut self, key: &str, value: &str) -> Result<()>;
}
```

Local SQLite operations take microseconds to low milliseconds at personal-vault scale. A sync API is trivially implementable in memory, needs no async runtime abstraction in core (which `sync-client-engine` still has to settle for transports), and on native the engine or Tauri command can call it from a blocking task. No `Send` bound is on the trait (wasm values aren't `Send`). `SqliteStorage` and `MemoryStorage` are both `Send`, and native callers share one behind a `Mutex`.

**Alternative considered:** an async trait. Rejected. It adds runtime coupling for no I/O concurrency gain on a single-writer local database.

### Types

- `ServerSeq(u64)`: server-assigned sequence, 0 = nothing seen. The cursor is one `ServerSeq` per vault, because the `sync-server` op log is per vault, not per note.
- `UpdateId([u8; 16])`: caller-supplied (the sync engine generates it randomly when it records a local update, and remote updates carry theirs from the server). Storage only enforces uniqueness. If `sync-protocol` defines its own update-ID type, this becomes a re-export or `From` conversion; the 16-byte width is what matters here.
- `LogPos(u64)`: storage-assigned, strictly increasing across the whole store. It orders the outbox and log entries, and it's the compaction cutoff. The engine reads it from `StoredNote` entries (`max_pos`) when it takes a snapshot.
- `LocalUpdate { id: UpdateId, bytes: Vec<u8> }`.
- `StoredNote { wrapped_key, base: Option<Vec<u8>>, entries: Vec<LogEntry> }`, `LogEntry { pos, id: Option<UpdateId>, kind: EntryKind::{Update, Snapshot}, seq: Option<ServerSeq>, bytes }`. `kind` matters because update vs snapshot are distinct `PayloadKind`s in the AAD, so the caller must call `apply_update` vs `load_snapshot`.
- `RemoteBatch { keys: Vec<(NoteId, Vec<u8>)>, entries: Vec<RemoteEntry>, cursor: ServerSeq }`, `RemoteEntry { note, kind, id: Option<UpdateId>, seq, bytes }`. Remote snapshots have no `UpdateId`.
- `StorageError { VaultMismatch { stored, requested }, SchemaTooNew(u32), Corrupt, NoteNotFound(NoteId), DuplicateNote(NoteId), DuplicateUpdate, CursorRegressed { current, requested }, Backend(String) }`. `Backend` carries the SQLite message for logs. Bytes stored in rows are never echoed into errors.

### Log model: base snapshot plus an ordered log of updates *and* snapshots

A note's state is `base snapshot (optional) + log entries in LogPos order`. Remote snapshots (uploaded by another device's compaction, delivered on bootstrap or pull) are appended to the log as `Snapshot` entries rather than replacing the base. Replacing could drop state: a locally compacted base may include acknowledged updates newer than the remote snapshot's coverage. Since applying a snapshot over more state is idempotent, appending is always safe.

Only `compact` replaces the base. It deletes entries with `pos <= through` that are **not** in the outbox. Pending entries stay, so a crash between compaction and push can't lose an edit. They're applied redundantly on load, which is harmless. The engine's compaction recipe is to open the document from storage, apply anything new, call `snapshot()`, then call `compact(id, snapshot, max_pos_seen)`. That also covers folding in remote snapshots.

**Alternative considered:** separate tables for "applied remote updates" and "outbox", as the original proposal sketched. Rejected because a local update would then live in two places, and loading a note would have to merge them. One log with an `is_pending` flag makes "logged and queued" a single row insert.

### Remote batch semantics

`apply_remote` runs in one transaction, in this order:

1. Reject if `batch.cursor < current` (`CursorRegressed`). Equal is allowed, for idempotent retries.
2. Insert wrapped keys for unknown notes (`key_pending = false`). Keep existing keys.
3. For each entry, error if its note is unknown. That aborts the batch.
   - If its `UpdateId` is already stored: when that row is pending, clear `is_pending` and set `seq`; otherwise skip it.
   - If it's a new entry, append it.
4. Set the cursor.

Echo handling (step 3) means a device's own pushes coming back over pull or WebSocket are neither duplicated nor left in the outbox if the push response was lost.

### Wrapped note keys and their upload state

`create_note` stores the key with `key_pending = true`. The sync engine uploads it (shape is up to `sync-protocol`), then calls `mark_key_uploaded`. Keys from remote batches are never pending. Storage tracks this because a key that never reached the server makes that note's updates undecryptable on every other device. It needs the same durability as the outbox.

### SQLite schema (version 1)

```sql
CREATE TABLE meta     (key TEXT PRIMARY KEY, value BLOB NOT NULL) STRICT;
  -- 'vault_id' (16 bytes, absent when unbound), 'cursor' (u64 as INTEGER)
CREATE TABLE notes    (note_id BLOB PRIMARY KEY, wrapped_key BLOB NOT NULL,
                       key_pending INTEGER NOT NULL, base BLOB) STRICT;
CREATE TABLE log      (pos INTEGER PRIMARY KEY AUTOINCREMENT,
                       note_id BLOB NOT NULL REFERENCES notes ON DELETE CASCADE,
                       update_id BLOB UNIQUE, kind INTEGER NOT NULL,
                       seq INTEGER, is_pending INTEGER NOT NULL, bytes BLOB NOT NULL) STRICT;
CREATE INDEX log_note ON log(note_id, pos);
CREATE INDEX log_pending ON log(pos) WHERE is_pending = 1;
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
```

`AUTOINCREMENT` guarantees `pos` never reuses a value after deletes, which the compaction cutoff relies on. IDs are stored as raw 16-byte blobs.

**Connection setup:** `journal_mode=WAL`, `synchronous=FULL` (so an append that returned survives power loss, as the outbox-durability requirement demands; NORMAL can lose the last commits), `foreign_keys=ON`, `busy_timeout=5000`. Writes are a handful per second at most (the editor debounces), so FULL's extra fsync is fine.

**Opening:** `SqliteStorage::open(path, vault_id)`.
1. `PRAGMA quick_check` on a non-empty file. Failure, or SQLite's "not a database", becomes `Corrupt`, and the file is never touched.
2. Read `user_version`. If it's greater than the latest known version, return `SchemaTooNew` before any write.
3. Run pending migrations, each in its own transaction that also bumps `user_version`.
4. Check or record the `vault_id` binding.

Migrations are an ordered `&[&str]` of SQL in `schema.rs`, hand-rolled (about 30 lines) rather than pulling in a migration crate. `open_in_memory` (backed by SQLite `:memory:`) exists only for tests.

`clear(self)` deletes all rows except `settings`, removes `meta.vault_id`, and resets the cursor in one transaction, then closes. It consumes the handle so no further writes can re-populate a store that has lost its binding. `delete_database(path)` (an inherent fn) removes the file plus its `-wal`/`-shm` siblings, for an app that wants to wipe even settings.

**Path:** the app passes one fixed per-device path (e.g. `<app data dir>/notebook.sqlite3`). Settings live there too, so they're truly per device and survive sign-out. Per-vault files were considered. They make "settings per device" awkward and buy nothing while only one vault is signed in at a time.

### In-memory implementation

`MemoryStorage::new(vault_id)` holds `BTreeMap`s mirroring the tables plus a `next_pos` counter. Each multi-step write validates everything first, then mutates, which gives atomicity without rollback logic. `clear(self)` just drops it. Settings in the browser are ephemeral by design; `browser-web-client` keeps UI preferences in localStorage itself.

### Conformance suite

`notebook_core::storage::conformance` exposes `pub fn run_all<S: Storage>(make: impl FnMut() -> S)` plus one fn per spec requirement. Core's tests call it with `MemoryStorage`, and the SQLite crate's tests call it with `open_in_memory` and with a tempfile-backed store. Behavior that only exists on disk lives only in the SQLite crate's tests: reopen, crash atomicity, schema upgrade, corruption, vault mismatch, and the marker-not-on-disk check. Crash atomicity is tested by aborting a transaction mid-batch (drop without commit) and by a child process killed after write. The latter can be `#[ignore]`d on CI if it's flaky.

## Risks / Trade-offs

- [Shared `libsqlite3-sys` with server `sqlx`: a version bump of either crate can break the other's resolve] → Pin `rusqlite` in `[workspace.dependencies]` with a comment linking the two. CI's `cargo test --workspace` catches a mismatch immediately.
- [Metadata at rest: note IDs (which embed creation times), counts, sizes, sequence numbers, and settings are readable from the file] → Same metadata the server already sees (vault-crypto threat model). A stolen device with an unlocked OS account is out of scope. SQLCipher could be added later behind the same trait.
- [`synchronous=FULL` costs an extra fsync per commit] → Negligible at edit-debounce rates. Required for the outbox guarantee.
- [Pending entries survive compaction, so they're applied twice on load] → CRDT idempotence makes it harmless, and they go away at the next compaction after acknowledgment.
- [The log grows unbounded if the engine never compacts] → The engine owns the snapshot threshold. Storage exposes entry counts via `load_note`, so the engine can decide.
- [Corrupt database blocks the app] → Surfaced as `Corrupt`, not auto-deleted, because the outbox may hold unsynced edits. The app shell can offer "reset local data and re-download" after warning about pending edits.
- [Synchronous calls on the UI thread could stutter] → Native callers run storage on a blocking worker, and the Tauri commands are async wrappers.

## Migration Plan

None. No client data is persisted yet. This change defines schema version 1, and later layouts are added as new migration steps.

## Open Questions

- The exact wire shape for uploading and distributing wrapped note keys belongs to `sync-protocol`. Storage only needs "pending until marked uploaded", which works whatever the shape.
- Whether `UpdateId` is a storage-local type or re-exported from the `sync-protocol` types. The width (16 bytes) is the only thing storage depends on.
