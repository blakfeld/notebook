# Tasks

## 1. Recovery entry in notebook-core

- [ ] 1.1 Add `crypto/recovery_entry.rs` with `RecoveryEntry::{create, open, vault_id, to_bytes, from_bytes}` (layout `version || vault id || envelope`, kind 33 via `RecoveryKey::wrapping_key`), re-exported from `crypto`; verify unit tests cover every scenario under "Recovery entry" in `specs/vault-crypto/spec.md`, including the fixed test vector, and `cargo build -p notebook-core --target wasm32-unknown-unknown` succeeds

## 2. Storage

- [ ] 2.1 Add a single-row `vault_meta` table to `notebook-storage-sqlite` holding the serialized recovery entry, with a schema version bump, an in-place upgrade, and `recovery_entry()`/`set_recovery_entry()` on `SqliteStorage` (not on the `Storage` trait: only the native app needs it); verify tests for round trip, upgrade from the previous layout keeping all data, and reopening

## 3. Host state and credentials

- [ ] 3.1 Replace the keychain entries in `credentials.rs` with a single `vault-key` entry (vault ID plus key), keeping read-only access to the legacy `session`, `local-unlock`, and `auto-unlock` entries for the upgrade, and update the module docs table; verify the in-memory credential store tests pass
- [ ] 3.2 Rework `state.rs` phases to `Setup | PendingRecoveryKey | NeedsRecoveryKey | NeedsOldPassphrase | Open` with the startup decision order from design Decision 3, and make `vault.rs` open storage, replica, and workspace without auth, transport, or sync engine; verify mock-runtime tests for each startup branch (keychain present, no database, database without a keychain entry, legacy session)
- [ ] 3.3 Implement `create_notebook` and `acknowledge_recovery_key` (nothing persisted before acknowledgment); verify tests that quitting before acknowledgment leaves no database or keychain entry, and that after acknowledgment a relaunch opens with no prompt
- [ ] 3.4 Implement `open_with_recovery_key` for the no-secure-store path, distinguishing mistyped (checksum) from wrong keys; verify tests for correct, wrong, and mistyped keys with the secure store disabled, and that nothing is written to any file
- [ ] 3.5 Implement the upgrade (auto-unlock path, `upgrade_with_passphrase` through `LocalUnlock`, and start-over that renames the old database to `vault.sqlite3.legacy-<timestamp>`); verify `vault_tests.rs` tests that seed a legacy signed-in install with notes and pending outbox entries, run each path, and find every note present, legacy entries deleted only after acknowledgment, and the renamed database intact on start-over
- [ ] 3.6 Remove the sign-in, register, recover, lock, unlock, reauthenticate, change-passphrase, sign-out, pending-count, and sync-status commands and their DTOs from `commands/mod.rs`, `dto.rs`, and the invoke handler; make permanent delete purely local; verify `cargo clippy -p notebook-app --all-targets -- -D warnings` and `cargo test -p notebook-app` pass, with a test that permanent delete works with no server running
- [ ] 3.7 Narrow the CSP in `tauri.conf.json` to `connect-src 'self' ipc: http://ipc.localhost` (dev keeps `ws:`); verify the app starts under `cargo tauri dev` and no console CSP violations appear while editing

## 4. Frontend

- [ ] 4.1 Update `web/src/platform` (`types.ts`, `core.ts`, `tauri.ts`, `fake.ts`, `demo.ts`) to the new phase DTO and commands, removing the account and sync methods; verify `npm run check` and the platform unit tests pass
- [ ] 4.2 Replace `lib/account/` with `lib/setup/` (`Welcome`, `RecoveryKeyShown`, `RecoveryKeyPrompt`, `OldPassphrase`) and route `App.svelte` by phase; verify component tests cover every scenario under "First launch creates a notebook", "Recovery key is shown once and acknowledged", "Recovery key entry without a secure store", and "Upgrade from a server-backed install"
- [ ] 4.3 Delete `lib/state/sync.svelte.ts` and `shell/SyncIndicator.svelte`; remove the account, passphrase, lock, and sign-out sections from `Settings.svelte` and add an About section with the version; verify `shell.svelte.test.ts` covers the "Settings contents" scenario and `npm test` passes

## 5. Docs and integration

- [ ] 5.1 Update `README.md` (description, layout, and "Running" for a local-only desktop app, plus an upgrade note: let sync settle, then install the new version and save the new recovery key); verify the documented `cargo tauri dev` flow works as written
- [ ] 5.2 End-to-end on desktop: fresh install creates a notebook, survives relaunch with no prompt, and makes no outbound connection while editing (check with the OS network monitor or Little Snitch); upgrade a copy of a real signed-in data directory and confirm every note survives; verify by recording the results in the apply summary
- [ ] 5.3 Run the definition-of-done checks from `CLAUDE.md` and `openspec validate local-only-desktop --strict`; verify all exit 0

## Workflow follow-up

- Review with `code-reviewer` and `security-analyst` (keychain handling, recovery entry, CSP), then archive with `/opsx:archive`.
- After archive, edit the `app-shell` Purpose in `openspec/specs/app-shell/spec.md` to describe a local-only desktop app.
- `retire-sync-server` starts once this is archived.
