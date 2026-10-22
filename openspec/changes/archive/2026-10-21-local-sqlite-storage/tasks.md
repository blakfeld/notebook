# Tasks

## 1. Setup

- [x] 1.1 Add `rusqlite` (`bundled`) to `[workspace.dependencies]` at the release whose `libsqlite3-sys` matches the version already in `Cargo.lock` (0.37 via `sqlx`), with a comment tying the two together; verify `cargo tree -i libsqlite3-sys` shows a single version
- [x] 1.2 Create `crates/notebook-storage-sqlite` (workspace-inherited package fields and lints, `#![forbid(unsafe_code)]`, deps `notebook-core` + `rusqlite`, dev-deps `tempfile` + `notebook-core` with `conformance` feature); verify `cargo build -p notebook-storage-sqlite` succeeds
- [x] 1.3 Add an empty `storage` module and a `conformance` feature to `notebook-core`; verify `cargo build -p notebook-core --target wasm32-unknown-unknown` still succeeds and `cargo tree -p notebook-core` gains no dependencies

## 2. Storage contract in notebook-core

- [x] 2.1 Define `ServerSeq`, `UpdateId`, `LogPos`, `EntryKind`, `LocalUpdate`, `LogEntry`, `StoredNote`, `RemoteEntry`, `RemoteBatch`, `OutboxEntry`, and `StorageError` per design.md (errors implement `Display` and `std::error::Error`, never echo stored bytes); verify with unit tests for `Display` output and `ServerSeq` ordering
- [x] 2.2 Define the synchronous `Storage` trait with the methods in design.md and rustdoc stating each method's atomicity and error cases; verify `cargo doc -p notebook-core --no-deps` builds without warnings
- [x] 2.3 Export the storage types from `notebook-core` and extend `crates/notebook-core/tests/public_api.rs` to name them; verify that test passes

## 3. Conformance suite

- [x] 3.1 Add `storage::conformance` (compiled under `cfg(any(test, feature = "conformance"))`) with a `run_all` entry point and one test function per portable requirement: create note and duplicate ID, append local and unknown note, load order and absent note, enumerate, outbox order, duplicate update ID, acknowledge and repeat ack, cursor start and regression, remote batch with new note, unknown-note batch rollback, echo handling, remote key not replacing local, pending keys, compaction (all three scenarios), delete note, clear (settings kept), settings round trip and missing; verify it compiles with `cargo check -p notebook-core --features conformance`

## 4. In-memory implementation

- [x] 4.1 Implement `MemoryStorage` with validate-then-mutate writes so every failing call leaves state unchanged; verify `conformance::run_all` passes against it in `cargo test -p notebook-core`
- [x] 4.2 Add a test that a failed `apply_remote` (unknown note mid-batch) and a failed `create_note` leave a `MemoryStorage` byte-for-byte equal to its prior state (compare `load_note`/`outbox`/`cursor`); verify it passes
- [x] 4.3 Verify `MemoryStorage` builds for `wasm32-unknown-unknown` with `cargo build -p notebook-core --target wasm32-unknown-unknown`

## 5. SQLite implementation

- [x] 5.1 Implement `schema.rs`: version-1 schema from design.md as the first entry of an ordered migration list, applied in per-step transactions that bump `user_version`; verify with a test that a fresh database reaches `user_version = 1` and has the expected tables
- [x] 5.2 Implement `SqliteStorage::open(path, vault_id)` and `open_in_memory`: pragmas (WAL, `synchronous=FULL`, foreign keys, busy timeout), `quick_check`, too-new check before any write, migrations, vault binding; verify with tests for reopen-same-vault, vault mismatch (data unchanged), open-after-clear for another vault, too-new `user_version` (file bytes unchanged), and a garbage file (`Corrupt`, file bytes unchanged)
- [x] 5.3 Implement every `Storage` method with one transaction per call and the `apply_remote` ordering from design.md; verify `conformance::run_all` passes against both `open_in_memory` and a tempfile-backed store
- [x] 5.4 Implement `clear(self)` and `delete_database(path)` (removes `-wal`/`-shm` too); verify with tests that settings survive `clear` and that `delete_database` leaves no files behind
- [x] 5.5 Add a schema-upgrade test harness: a test-only migration list with a version-2 step applied to a version-1 file holding notes, outbox entries, a cursor, and settings; verify all data is preserved and `user_version` advances
- [x] 5.6 Add durability tests: append local, drop the handle without acknowledging, reopen, and see the entry in outbox and log; and a batch whose transaction is dropped before commit leaves nothing stored and the cursor unchanged; verify both pass
- [x] 5.7 Add the at-rest privacy test: encrypt a body containing a marker string with `NoteKey::encrypt` (as update and snapshot), store via `create_note`, `append_local`, and `compact`, close, then scan the database file and its `-wal`/`-shm` siblings for the marker; verify it is absent
- [x] 5.8 Add a short crate-level rustdoc to `notebook-storage-sqlite` covering path choice, one vault at a time, settings being unencrypted and non-sensitive only, and `Corrupt` not auto-resetting; verify `cargo doc -p notebook-storage-sqlite --no-deps` builds without warnings

## 6. Integration checks

- [x] 6.1 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo build -p notebook-core --target wasm32-unknown-unknown`; verify all pass
- [x] 6.2 Cross-compile `notebook-storage-sqlite` for one mobile target (`aarch64-apple-ios` or `aarch64-linux-android`) to confirm bundled SQLite builds; verify the build succeeds or record the toolchain gap for `mobile-app`

## Workflow follow-up

- Mark `local-sqlite-storage` `[x]` in TODO.md and archive the change after review.
