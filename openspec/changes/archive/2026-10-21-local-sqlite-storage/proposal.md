# Proposal

## Why

Native apps (desktop and mobile) must work fully offline and hold a complete local copy of the vault. That needs durable local persistence of encrypted note documents and sync bookkeeping. SQLite is the chosen store. A storage abstraction keeps `notebook-core` platform-independent and gives the browser and tests an in-memory implementation.

## What Changes

- Define a `Storage` contract in `notebook-core`, bound to one vault, covering:
  - Per note: the wrapped note key, an optional compacted base snapshot, and an ordered log of encrypted updates and snapshots received since.
  - An outbox: locally made updates not yet acknowledged by the server, durable across crashes and restarts.
  - The vault's sync cursor (last server sequence number applied). Remote batches and the cursor advance commit together.
  - Device-local settings (e.g. vim mode on/off), non-sensitive only.
- Provide an **in-memory** implementation, used by the thin browser client and by tests.
- Provide a **SQLite** implementation (`rusqlite`, bundled SQLite) for native targets, with versioned schema migrations, in a new `notebook-storage-sqlite` crate so `notebook-core` stays wasm-clean.
- Data at rest stays encrypted: storage only accepts ciphertext blobs from `note-documents` and wrapped keys from `vault-crypto`. Decrypted state and derived indexes (tag tree, search) live only in memory and are rebuilt by consumers from stored documents on startup.
- A shared conformance test suite runs against both implementations.

## Capabilities

### New Capabilities
- `local-storage`: Persistence contract for encrypted note documents, wrapped note keys, the update outbox, the sync cursor, and device-local settings, with SQLite (native) and in-memory implementations.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-core`: new `storage` module with the trait, types, in-memory implementation (wasm-compatible), and the conformance suite. No new dependencies.
- New crate `crates/notebook-storage-sqlite`: depends on `notebook-core` and `rusqlite` (`bundled`). Must resolve to the same `libsqlite3-sys` as the server's `sqlx` (Cargo allows one `links = "sqlite3"` crate per workspace).
- Depends on `note-documents` (`crdt-note-documents`) for the ciphertext it stores and on `vault-crypto` IDs and wrapped note keys.
- Consumed by `sync-client-engine` (outbox, cursor, bootstrap, compaction) and the Tauri app (`desktop-app-shell`, `mobile-app`), which choose the database path.
- Out of scope: when to compact or push (sync engine), where key material and refresh tokens live (OS keychain, app shell), browser persistence (none, by `browser-web-client` design).
