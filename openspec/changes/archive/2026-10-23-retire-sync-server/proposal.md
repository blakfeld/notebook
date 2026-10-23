# Proposal

## Why

After `local-only-desktop`, nothing calls the sync server, OPAQUE auth, or the sync engine. They are still roughly half the codebase, most of the dependency tree (axum, sqlx, reqwest, tungstenite, opaque-ke, gloo), and most of what security review has to cover. Dead code that still compiles still has to be kept compiling, patched, and reviewed. The storage outbox also blocks compaction now that nothing ever acknowledges an update.

## What Changes

- **BREAKING**: Delete the `notebook-server` crate, `docs/deployment.md`, and the server's CI and test wiring.
- Delete `notebook-core::auth`, `protocol`, `workspace::server_url`, and the sync engine, transports, sessions, backoff, timers, and fakes. Move `Replica` (and `EditorSessionId`, `ReplicaError`, the `Shared` wrapper) out of `sync` into its own `replica` module, since the app and `Workspace` still use it.
- Remove the `transport-native`, `transport-wasm`, and `test-util` features and their dependencies. `notebook-core` keeps building for `wasm32-unknown-unknown` (crypto, documents, tags, storage, backup) so a browser client stays possible later, and CI and the definition of done check that plain build.
- Delete `KeyBundle`, the recovery verifier, and auth-secret plumbing. Keep `LocalUnlock` and Argon2id only for the legacy-install upgrade.
- **Storage**: remove the outbox, update identifiers, acknowledgments, sync cursor, remote batches, and pending keys from the `Storage` trait, the memory store, and SQLite. A schema upgrade drops that data and keeps every note and log entry. Compaction no longer waits for acknowledgment.
- **Local compaction**: the app snapshots a note once its log passes a threshold (the policy the sync engine used to run), so logs stop growing.
- Remove the `mobile-developer` agent, and update the `code-reviewer`, `security-analyst`, `tauri-developer`, and `web-developer` agent descriptions, `README.md`, `CLAUDE.md`, `scripts/check.sh`, and `.github/workflows/ci.yml` for a desktop-only app.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `accounts`: all requirements removed.
- `invites`: all requirements removed.
- `server-hosting`: all requirements removed.
- `sync-protocol`: all requirements removed.
- `sync-client`: all requirements removed.
- `local-storage`: removes outbox, cursor, remote batch, echo, identifier, and pending-key requirements. Adds plain local durability, and rewrites create, append, load, enumerate, compaction, delete, clear, schema versioning, and memory parity without them.
- `vault-crypto`: removes key bundle, auth secret input, passphrase rewrapping, and recovery verifier. Rewords vault identity and context binding without a server.

## Impact

- Deleted: `crates/notebook-server/`, `crates/notebook-core/src/{auth,protocol.rs,sync/{engine,transport,session,backoff,timer,native,wasm,fake,tests}.rs,workspace/server_url.rs}`, `crypto/bundle.rs`, the verifier in `crypto/recovery.rs`, `docs/deployment.md`, `.claude/agents/mobile-developer.md`, and the app's server-backed integration tests.
- Workspace dependencies removed: `axum`, `clap`, `gloo-net`, `gloo-timers`, `js-sys`, `opaque-ke`, `rand_core` (if only opaque used it), `reqwest`, `sha2_010`, `sqlx`, `tokio-tungstenite`, `tower`, `tower-http`, `tracing-subscriber` (if only the server used it).
- `notebook-storage-sqlite`: schema upgrade and slimmer API. `notebook-app`: drops `transport-native` and the `notebook-server`/`sqlx` dev-dependencies, and runs local compaction.
- The five retired capability directories are deleted from `openspec/specs/` after archive.
- Needs `local-only-desktop` archived. It does not touch `vault-backup` except `import_note`, which lands in either order.
