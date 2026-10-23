# Tasks

## 1. Move the replica

- [x] 1.1 Move `Replica`, `EditorSessionId`, `ReplicaError`, `Shared`, and the change types `Workspace` uses from `sync` to a new `notebook_core::replica` module, and update `workspace` and `notebook-app` imports; verify `cargo test --workspace` passes unchanged

## 2. Delete the server

- [x] 2.1 Delete `crates/notebook-server`, remove it from the workspace, and delete the app's server-backed tests (`integration_tests.rs`, the server parts of `state/tests.rs` and `vault_tests.rs`) along with the `notebook-server`/`sqlx` dev-dependencies; verify `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [x] 2.2 Delete `docs/deployment.md` and the server sections of `README.md`, and add an operator note on shutting a server down after devices have upgraded and backed up; verify no doc links point to removed files (`grep -rn deployment.md`)

## 3. Delete auth, protocol, and sync

- [x] 3.1 Delete `notebook-core::auth`, `protocol`, `workspace::server_url`, and the rest of `sync`, and drop the `transport-native`, `transport-wasm`, and `test-util` features and their optional dependencies; remove `transport-native` from `notebook-app`; verify `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo clippy -p notebook-core --target wasm32-unknown-unknown -- -D warnings` pass

## 4. Storage

- [x] 4.1 Slim the `Storage` trait, `LocalUpdate`, and `LogEntry` per design Decision 2 in core, the memory store, and SQLite, and update the conformance suite to the modified and removed `local-storage` requirements; verify `cargo test -p notebook-core --features conformance` and `cargo test -p notebook-storage-sqlite` pass
- [x] 4.2 Add the SQLite schema upgrade that drops outbox, cursor, identifiers, sequence numbers, and pending flags; verify a test that seeds the previous layout (acknowledged, unacknowledged, and compacted notes plus settings) and finds every note's decoded `Note` and every log entry preserved, and that a newer layout still fails with layout-too-new
- [x] 4.3 Move the snapshot threshold policy into `Replica` (compact after append once a note's log passes the engine's old constant); verify tests that a note edited past the threshold loads with a base snapshot and a short log, and reloads to the same body

## 5. Crypto

- [x] 5.1 Delete `crypto/bundle.rs`, the recovery verifier, and auth-secret plumbing not used by `LocalUnlock`, keeping the envelope kind constants reserved in a comment so they're never reused; verify `cargo test -p notebook-core` passes and the "Recovery entry" fixed test vector is unchanged

## 6. Dependencies, CI, docs, agents

- [x] 6.1 Remove unused workspace dependencies (`axum`, `clap`, `gloo-*`, `js-sys`, `opaque-ke`, `reqwest`, `sha2_010`, `sqlx`, `tokio-tungstenite`, `tower`, `tower-http`, and `rand_core`/`tracing-subscriber` if now unused) and refresh `Cargo.lock`; verify `cargo tree -d` shows no new duplicates and `cargo build --workspace` succeeds
- [x] 6.2 Update `.github/workflows/ci.yml` and `scripts/check.sh` (wasm job without features, no server steps) and `CLAUDE.md` definition-of-done item 4; verify `scripts/check.sh` exits 0
- [x] 6.3 Delete `.claude/agents/mobile-developer.md` and remove server, OPAQUE, browser-client, and mobile references from the `code-reviewer`, `security-analyst`, `tauri-developer`, and `web-developer` agent descriptions; verify `grep -rniE "opaque|mobile|browser client|sync server" .claude/agents` returns only intended mentions

## 7. Integration

- [ ] 7.1 Upgrade a copy of a real data directory from the `local-only-desktop` build and confirm every note opens and edits, then edit one note heavily and confirm its log compacts; verify by recording results in the apply summary
- [x] 7.2 Run the updated definition-of-done checks and `openspec validate retire-sync-server --strict`; verify all exit 0

## Workflow follow-up

- Review with `code-reviewer`, then archive with `/opsx:archive`.
- After archive, delete `openspec/specs/{accounts,invites,server-hosting,sync-protocol,sync-client}/` (now empty), and update the `local-storage` and `vault-crypto` Purpose lines to drop the outbox, sync cursor, and server-stored key bundle.
