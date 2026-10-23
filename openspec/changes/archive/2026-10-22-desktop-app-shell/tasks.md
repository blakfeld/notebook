# Tasks

## 1. Prerequisites and dependencies

- [x] 1.1 Confirm that `crdt-note-documents`, `local-sqlite-storage`, `opaque-accounts`, `sync-server`, `sync-client-engine`, and `note-editor-component` are archived, and check their published APIs against design.md's assumptions (`Replica` edit and read methods, `SyncHandle`, the `SyncEvent` receiver and `SyncStatus` variants, device-local settings, authenticated key-bundle read, editor host interface); record any mismatch in design.md and stop to report it before continuing; verify by listing each assumed API with its actual signature in the design's Context table
- [x] 1.2 Add `keyring` (macOS native, Windows native, Linux `sync-secret-service` + `crypto-rust`) to the workspace and `notebook-app`, plus whatever core/sync/storage features the app needs; create empty `commands/`, `state.rs`, `credentials.rs`, `error.rs`, `dto.rs`, and `events.rs` modules in `notebook-app`, and empty `workspace` and `search` modules in `notebook-core`; verify `scripts/check.sh` passes, including the wasm32 core build

## 2. Core: server address and search

- [x] 2.1 Implement `ServerUrl::parse` (https anywhere, http only for loopback or `localhost`, trailing slash normalized) with errors naming the problem; verify unit tests for the three Server address scenarios plus `http://[::1]`, `http://localhost:8080`, `ftp://…`, and a URL with no host
- [x] 2.2 Implement `search::SearchIndex` (per-note lowercased body, whitespace-split terms, all-terms substring match, insert/update/remove by note id) and `filter(ordered_ids, query)` preserving order; verify unit tests for all-terms, case-insensitive, empty query, non-ASCII (`café`, `日本語`) terms, and removal
- [x] 2.3 Add a public `Replica::set_setting(key, value)` writing through to storage (M3); verify a unit test that the value reads back through `Replica::storage()` and survives reopening the store
- [x] 2.4 Add zeroizing `VaultKey::to_secret_bytes`/`from_secret_bytes` and `crypto::LocalUnlock` (salt, KDF version, vault key sealed under the Argon2id-derived key with the vault id as AAD; `seal`, `open`, `to_bytes`, `from_bytes`) (M4); verify unit tests for round trip, wrong passphrase, a blob bound to another vault id, and a corrupted blob
- [x] 2.5 Add `auth::http`: `AuthClient` over an `AuthHttp` trait for register, login, recover, refresh, key-bundle, logout, and passphrase change, with errors distinguishing network, rate limit, and server codes; native (reqwest, `transport-native`) and wasm (gloo-net, `transport-wasm`) implementations; and `AuthSession`, a `SessionSource` that refreshes through it and reports rotated refresh tokens to a host callback (M2); verify tests against an in-memory `AuthHttp` for each call's request path and error mapping, that `SessionEnded` maps to `RefreshError::SessionEnded`, and that the wasm32 core build passes

## 3. Core: workspace

- [x] 3.1 Implement `Workspace` wrapping the shared `Replica` and `SyncHandle` (no document ownership of its own): build tree and index from `Replica::notes()` on open, record ids of its own local edits, drain `SyncEvent::NotesChanged`, keep the tag tree and search index current from local and remote changes, and emit change notifications with changed ids and a tree-changed flag, and expose `all_notes()` passing through `Replica::notes()` (archived and trashed included); verify tests with in-memory storage that opening rebuilds tree and index, that `all_notes()` includes a trashed note, and that a remote update produces a notification
- [x] 3.2 Implement `list(selection, query)` returning entries (id, title, preview from the text after the title line, updated, flags) for views and tags in tag-tree order, filtered through search; verify tests covering each special view, a parent tag including descendant-tagged notes, ordering by updated time, "Untitled"-ready empty titles, and scoped search
- [x] 3.3 Implement `create(selection, now)` (body `#<tag>\n` with multi-word tags closed by `#`, else empty), `set_pinned`, `set_archived`, `set_trashed`, rejection of body changes to trashed notes, and `purge` (trashed notes only, awaiting `SyncHandle::purge`, `offline` when unreachable); verify tests for create under `work/acme` and under a multi-word tag, view membership after each flag change, restore returning an archived note to Archive, trashed-note edit rejected, purge of a non-trashed note refused, and purge while offline leaving the note in Trash
- [x] 3.4 Implement `rename_tag(old, new, now)` using `tags::rename` over every note including archived and trashed, applying `set_body` per affected document; verify tests for rename, move under a new parent, merge into an existing tag, archived/trashed notes rewritten, invalid name changing nothing, and one update per affected note in the outbox

## 4. App: credentials and state machine

- [x] 4.1 Implement the `CredentialStore` trait with `keyring` and in-memory implementations and the `session`, `local-unlock`, and `auto-unlock` entries, detecting "no secure store" on first write; verify unit tests on the in-memory store and a manual check on macOS that entries appear under service `com.blakfeld.notebook` in Keychain Access
- [x] 4.2 Implement `AppError` with the design's stable codes, the DTOs, `AppState` with `SignedOut`/`Locked`/`Unlocked` phases, launch-time phase selection from stored entries, and the `app_state` command and `app-phase-changed` event; verify mock-runtime tests for each launch case (no entry, auto-unlock present, auto-unlock absent)
- [x] 4.3 Implement `sign_in`, `generate_recovery_key`, and `register` (server URL validation, passphrase length and match checks, pending recovery key consumed by register, local-unlock and auto-unlock written, SQLite opened in the app data dir); verify mock-runtime tests that invalid input sends no request, `bad_credentials` is returned for both unknown user and wrong passphrase, `register` fails without a pending recovery key, and the no-secure-store fallback leaves no entries and reports the notice
- [x] 4.4 Implement `lock` and `unlock` (zeroize workspace, stop sync, delete auto-unlock, locked hint; online-first OPAQUE unlock with offline Argon2id fallback in `spawn_blocking`), and key-bundle fingerprint invalidation of `local-unlock`; verify tests for lock hiding state, relaunch-while-locked, online unlock, offline unlock via a failing transport, wrong passphrase staying locked, and a changed fingerprint forcing online unlock
- [x] 4.5 Implement `recover`, `change_passphrase` (online only, rewrites local-unlock), `sign_out` (pending count confirmation flag, best-effort revoke, delete entries and database files), `pending_count`, and session-expiry handling that keeps the workspace open with `SignInRequired`; verify tests for recovery key typo vs wrong key codes, offline passphrase change refused, sign-out removing the database and entries, sign-out requiring confirmation when pending > 0, and edits still accepted after a refresh failure

## 5. App: vault commands and events

- [x] 5.1 Implement `tag_tree`, `list_notes`, `create_note`, `set_pinned`, `set_archived`, `set_trashed`, `delete_permanently`, and `rename_tag` commands over `Workspace`, all returning `locked` when not unlocked; verify mock-runtime tests for each command's happy path and the locked error
- [x] 5.2 Forward workspace notifications as `vault-changed` and engine `SyncStatus` as `sync-status` (`synced`/`syncing`/`offline`, pending count, and attention flags `sign_in_required`/`upgrade_required`/`quota_full`/`update_too_large`/`integrity_warning`), running the engine on its own thread with a current-thread runtime; verify tests that a local edit and a simulated remote update each emit `vault-changed`, and that offline-with-pending, upgrade required, and quota full each map to the expected payload
- [x] 5.3 Implement the editor host commands and remote-update event defined by `note-editor-component`, refusing writes to trashed notes; verify tests that a local update from the editor reaches the outbox and a remote update reaches the open-note event
- [x] 5.4 Implement `get_settings`, `set_vim_mode`, and `set_tag_expanded` over device-local settings (vim default off); verify tests for defaults and persistence across an app-state reload
- [x] 5.5 Write a test that creates a note containing a unique marker word, syncs, locks, and scans every file in the temp app data dir for the marker and for the refresh token; verify the test passes, and review `tracing` call sites added in this change for secrets
- [x] 5.6 Supply the `reopen` storage callback `sync-client-engine` needs for its server-restore resync, reopening `SqliteStorage` at the same app-data path, and refresh the `Workspace` tree and index from the rebuilt `Replica` afterwards; verify a mock-runtime test where the engine reports `cursor_ahead`, the database is reopened at the same path, and a note edited before the resync is still listed and reaches the outbox

## 6. Frontend: platform adapter and state

- [x] 6.1 Define the `CoreApi` interface and DTO types in `web/src/platform/`, implement `tauri.ts` with `invoke`/`listen`, select it in `index.ts`, and add a `FakeCoreApi` for tests; verify `npm run check` passes and ESLint confirms no `@tauri-apps/api` import outside `platform/` (add a `no-restricted-imports` rule)
- [x] 6.2 Implement rune state modules (`phase`, `selection`, `notes`, `sync`, `settings`) that query the adapter and re-query on `vault-changed`, with debounced search; verify Vitest tests against `FakeCoreApi` that a `vault-changed` event refreshes the list and tree and that the search query is debounced

## 7. Frontend: account screens

- [x] 7.1 Build sign-in, register (with invite code, recovery key display, and required acknowledgment checkbox before submit), recover, and unlock screens, mapping error codes to messages; verify Vitest tests for mismatched passphrases blocking submit, `invalid_invite` and `username_taken` messages, acknowledgment gating registration, identical text for `bad_credentials`, and distinct unreachable/typo messages
- [x] 7.2 Show the no-secure-store notice and the sign-in-required prompt that re-enters the passphrase without leaving the shell; verify Vitest tests that each appears for the corresponding adapter state

## 8. Frontend: shell

- [x] 8.1 Build the three-pane `Shell` with `Sidebar` (special views, expandable `TagTree` persisting expansion), `NoteList` (title or "Untitled", preview, relative updated time, search field), and `EditorPane` (note editor, empty state, read-only trashed notes with Restore); verify Vitest tests for initial selection Notes, Untitled rendering, and trashed read-only mode
- [x] 8.2 Add note actions (new note, pin/unpin, archive/unarchive, trash/restore, delete permanently with confirmation from Trash only) in the list context menu and editor toolbar, plus `Mod+N`, `Mod+Shift+F`, and `Mod+L` shortcuts; verify Vitest tests that each action calls the adapter and that delete is offered only in Trash and requires confirmation
- [x] 8.3 Add tag rename/move from the sidebar (inline edit with full path, invalid-name message); verify Vitest tests for a successful rename and the `invalid_tag_name` message
- [x] 8.4 Add the `SyncIndicator` and the sectioned `Settings` modal (Account and Editor sections, leaving room for Appearance and Export; server, username, status, vim toggle applied live, change passphrase, lock, sign out with pending warning); verify Vitest tests for each sync state and for the sign-in required, upgrade required, and storage full messages, and that toggling vim calls `set_vim_mode` and updates the open editor
- [x] 8.5 Have `ui-designer` review the shell for layout, spacing, and empty states and apply its fixes; verify by launching the app with `cargo tauri dev` and checking each screen in light and dark OS appearance

## 9. Integration checks

- [x] 9.1 Add an app-crate integration test that starts `notebook-server` in-process with registration allowed and drives register → create and edit notes → a second workspace signs in and sees them → rename a tag → trash and purge a note → the second workspace reflects each step; verify it passes in `cargo test -p notebook-app`
- [x] 9.2 Manually verify on macOS (and Linux and Windows where available) with a local server: register, sign in, quit and relaunch without a prompt, go offline and edit, relaunch offline, come back online and see edits sync, lock and unlock offline, sign out; record results in the change's review notes
- [x] 9.3 Run `scripts/check.sh` and `openspec validate desktop-app-shell --strict`; verify both pass

## Workflow follow-up

- Review with the `code-reviewer` and `security-analyst` agents (credentials, lock, CSP, logging), then archive with `/opsx:archive` so `app-shell` lands in `openspec/specs/`.
- Update `TODO.md`: mark `desktop-app-shell` `[x]` once archived.
- `markdown-export` and `mobile-app` modify `app-shell` (`Settings` and `Three-pane layout`); they should start after this archive.
