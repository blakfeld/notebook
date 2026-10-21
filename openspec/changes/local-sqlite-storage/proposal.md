# Proposal

## Why

Native apps (desktop and mobile) must work fully offline and hold a complete local copy of the vault. That needs durable local persistence of encrypted note documents and sync bookkeeping. SQLite is the chosen store. A storage abstraction keeps `notebook-core` platform-independent and gives the browser and tests an in-memory implementation.

## What Changes

- Define a `Storage` trait in `notebook-core` covering:
  - Encrypted snapshots and pending or applied encrypted updates per note.
  - Sync cursors (last server sequence seen per note or vault).
  - An outbox of local updates not yet acknowledged by the server.
  - Device-local settings (e.g. vim mode on/off).
- Provide an **in-memory** implementation, used by the thin browser client and by tests.
- Provide a **SQLite** implementation (`rusqlite`) for native targets, with schema migrations.
- Data at rest in SQLite stays encrypted (ciphertext blobs). Decrypted state lives only in memory.
- Rebuild the derived tag index and search index in memory from stored documents on startup.

## Capabilities

### New Capabilities
- `local-storage`: Persistence contract for encrypted note documents, update outbox, sync cursors, and device-local settings, with SQLite (native) and in-memory implementations.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-core`: `storage` module with the trait and in-memory implementation (wasm-compatible). The SQLite implementation sits behind a native-only feature flag or in a small `notebook-storage-sqlite` crate.
- Dependency: `rusqlite` with the `bundled` feature (also works on iOS/Android).
- Consumed by the sync client engine and the Tauri app.
