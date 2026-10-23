# Design

## Context

See proposal.md (Why). Today `crates/notebook-app` is the scaffold: one `core_version` command, a `default` capability granting `core:default`, a strict CSP whose `connect-src` allows only IPC, and tests that drive commands on Tauri's mock runtime. `web/` is a Svelte 5 + Vite SPA with no router; `web/src/platform/index.ts` is the only file that imports `@tauri-apps/api`.

`notebook-core` already has `note` (`Note`, title derivation), `tags` (parser, `TagTree`, `View`, pure `rename`), and `crypto` (`KeyBundle`, `RecoveryKey`, `AuthSecret`, passphrase KDF v1, envelope). This change sits on top of six in-flight changes and consumes their interfaces rather than redefining them:

| capability | change | what the shell uses |
|---|---|---|
| `note-documents` | `crdt-note-documents` | `NoteDocument` (`set_body`, `set_pinned/archived/trashed`, `note()`) |
| `local-storage` | `local-sqlite-storage` | `Storage` trait, SQLite impl, device-local settings |
| `sync-client` | `sync-client-engine` | engine handle, status stream, change notifications, purge |
| `accounts` | `opaque-accounts` | client OPAQUE register/login/recover/change, refresh, logout |
| `sync-protocol` | `sync-server` | indirectly, through the engine |
| `note-editor` | `note-editor-component` | the editor component and its integration interface |

### Verified upstream APIs (task 1.1)

All six prerequisite changes are archived (`openspec/changes/archive/2026-10-2{1,2}-*`). Checked against the code on 2026-10-22:

| assumed API | actual | status |
|---|---|---|
| `Replica::notes()` / `note(id)` | `fn notes(&self) -> Vec<Note>`; `fn note(&self, id: NoteId) -> Option<Note>` | matches |
| `Replica::create_note` | `fn create_note(&mut self, body: &str, now: Timestamp) -> Result<NoteId, ReplicaError>` | matches |
| `Replica::set_body` / flag setters | `fn set_body(&mut self, id, body: &str, now) -> Result<(), ReplicaError>`; `set_pinned/set_archived/set_trashed(&mut self, id, bool) -> Result<(), ReplicaError>` | matches |
| `Replica::purge` (queue purge, remove locally) | **does not exist.** Purge is `SyncHandle::purge(&self, NoteId) -> impl Future<Output = Result<(), SyncError>>`: "Needs a connection; on failure nothing changes." Returns `SyncError::NotTrashed` / `NoteNotFound` / `Offline` | **mismatch M1** |
| `Replica::pending` | `fn pending(&self) -> Result<usize, StorageError>` | matches |
| `SyncHandle` | `wake()`, `async flush() -> Result<(), SyncError>`, `async purge(NoteId)`, `set_session()`, `shutdown()`; `Clone` | matches (plus `purge`) |
| `SyncEngine::new` | `new(Shared<Replica<S>>, T: Transport, M: Timer, A: SessionSource, SyncConfig, Reopen<S>) -> (SyncEngine, SyncHandle, mpsc::UnboundedReceiver<SyncEvent>)`; `Reopen<S> = Box<dyn FnMut() -> Result<S, StorageError>>`; `async run(self)`, not `Send` | matches |
| `SyncEvent` | `Status(SyncStatus)`, `NotesChanged(Vec<NoteId>)`, `IntegrityWarning(NoteId)` | matches |
| `SyncStatus` | `{ state: SyncState { Synced, Syncing, Offline }, pending: usize, attention: Option<Attention>, integrity_warnings: u32 }`; `Attention { SignInRequired, UpgradeRequired, QuotaFull, UpdateTooLarge(NoteId), PushBlocked }` | matches, but `attention` is a single `Option` (not a list) and there is an extra `PushBlocked` the design's `attention` list lacks (minor, M5) |
| `SessionSource` | host-implemented: `access_token(&self) -> Option<AccessToken>`, `refresh(&self) -> impl Future<Output = Result<(), RefreshError>>` | matches; the host also has to do the refresh HTTP call itself (see M2) |
| device-local settings | `Storage::setting(&self, &str) -> Result<Option<String>>`, `Storage::set_setting(&mut self, &str, &str) -> Result<()>`. `Replica` exposes only `storage(&self) -> Result<&S, _>`; `storage_mut` is `pub(crate)` | **mismatch M3**: no way to write a setting while the replica owns the store |
| `accounts` client | sans-I/O: `Registration::start(username, passphrase, invite_code)` / `finish(&RegisterStartResponse, Device, now_ms) -> (RegisterFinishRequest, NewAccount)`; `Login::start/finish -> (LoginFinishRequest, LoginKeys)`; `LoginKeys::unlock(&[u8]) -> Result<VaultKey, AuthError>`; `PassphraseChange::start(old, new)` / `finish(&resp, current_bundle)`; `Recovery::start(username, RecoveryKey, new)` / `finish(&resp, Device)`; `RefreshOutcome::classify` | APIs match, but **no HTTP client for `/auth/*` exists anywhere** (M2) |
| authenticated key-bundle read | server `GET /auth/key-bundle` → `KeyBundleResponse { key_bundle }` | exists on the server; no client call (part of M2) |
| storage open | `SqliteStorage::open(path, VaultId)`, `SqliteStorage::delete_database(path)` | matches; the vault id must be kept alongside the session entry |
| local-unlock / auto-unlock key material | `derive_auth_secret(passphrase, &Salt, KdfVersion) -> AuthSecret`. `VaultKey` has no byte export/import, `AuthSecret` has none either, and the envelope (`seal`/`open`) is `pub(crate)` | **mismatch M4**: the app can neither store the vault key (auto-unlock) nor wrap it under a passphrase-derived key (local-unlock) |
| editor host interface | `Replica::editor_attach(id) -> (EditorSessionId, EditorUpdate)`, `apply_editor_update(session, &EditorUpdate, now) -> Result<bool, _>`, `editor_pending(session) -> Result<Option<EditorUpdate>, _>`, `editor_detach(session)`; TS `NoteDocBridge { open(noteId) -> OpenNoteDoc { initialState, push, onRemote, close } }`; vim mode via TS `DeviceSettings` key `editor.vimMode` | matches |

Mismatches and their resolutions (decided 2026-10-22):

- **M1 – purge needs a connection.** The design's offline-queued `Replica::purge` doesn't exist; `SyncHandle::purge` requires the server and changes nothing when offline. That conflicts with the spec's "Every note operation SHALL work offline" for Delete permanently. **Resolved:** permanent delete is online-only; the spec says so (Delete permanently, Offline editing is durable) and the command returns `offline`.
- **M2 – no auth HTTP client.** `accounts` is sans-I/O and the sync transport covers only `/sync/v1`. Someone must send `/auth/{register,login,recover}/{start,finish}`, `/auth/refresh`, `/auth/key-bundle`, `/auth/logout`, and `/auth/passphrase/*`, and implement `SessionSource`. To keep the browser client able to reuse it, this belongs in `notebook-core` next to the sync transports (native reqwest / wasm fetch), not in the app crate. **Resolved:** new `notebook_core::auth::http` (task 2.5): an `AuthClient` over a small `AuthHttp` trait with native (reqwest), wasm (gloo-net), and test implementations, plus `AuthSession`, a `SessionSource` that refreshes through it and hands rotated refresh tokens to a host callback for storage. The server revokes a session when a refresh token is reused, so both sides share one `SessionTokens` that serializes refreshes (an async lock plus a generation counter: a waiter reuses another's result). After each refresh it also fetches the key bundle and passes it to the callback, which is where the fingerprint check runs. `Registration::finish_with_recovery_key` lets `register` use the key `generate_recovery_key` showed earlier.
- **M3 – no settings write through the replica.** Add a public `Replica::set_setting` (or make settings a separate small store), so `set_vim_mode`, `set_tag_expanded`, and the locked hint can be written while unlocked. **Resolved:** `Replica::set_setting` (task 2.3). While locked or signed out the app opens the store directly to read and write settings.
- **M4 – no way to persist key material.** Auto-unlock and local-unlock need new `crypto` API: an exportable/importable `VaultKey` (zeroized), and wrapping a `VaultKey` under an `AuthSecret` with the envelope (a "local unlock blob" type with salt, KDF version, and AAD bound to the vault id). **Resolved:** task 2.4 adds `VaultKey::to_secret_bytes`/`from_secret_bytes` (zeroizing) and `crypto::LocalUnlock` (`seal(vault, &VaultKey, passphrase)` / `open(passphrase)`, Argon2id via `derive_auth_secret`).
- **M5 – attention is one value.** `SyncStatus::attention` is `Option<Attention>` plus a separate `integrity_warnings` counter, and includes `PushBlocked`. The `sync-status` event can still carry a list, built from `attention` and `integrity_warnings > 0`; add `push_blocked` to it. No upstream change needed. **Resolved:** done that way in task 5.2.

Requirements are in `specs/app-shell/spec.md`. The browser client (`browser-web-client`) reuses the frontend and the same core logic over WASM, so anything that isn't Tauri-specific must live in `notebook-core` or behind the TS platform adapter.

## Goals / Non-Goals

**Goals:**
- The webview is a view. All decrypted state, keys, storage, sync, and networking live in Rust; the frontend holds only what it is currently rendering.
- One TS `CoreApi` interface that the Tauri adapter implements now and the WASM adapter implements later, with no frontend code aware of which.
- Vault behavior (queries, note and tag operations, search) in a wasm-compatible core module, so desktop and browser behave identically.

**Non-Goals:**
- How the editor's live document crosses the boundary and whether JS or Rust owns it while a note is open. That is `note-editor-component`'s decision; this change implements whatever host side it defines.
- Visual polish, themes, typography (`editor-live-preview`), responsive/stacked layout (`mobile-app`), export (`markdown-export`).
- Auto-lock on inactivity, biometric unlock (`mobile-app`), multiple accounts per install, keyboard-driven navigation outside the editor, release signing and auto-update.

## Decisions

### Layering

```
web/ (Svelte)            components ─ stores ─ platform/CoreApi ─┐
                                                                 │ invoke / events
crates/notebook-app      commands ─ AppState ─ CredentialStore   │
                                    │                            │
crates/notebook-core     workspace::Workspace ── search ── tags ─┘ (shared with wasm)
                           │ wraps Shared<Replica>   │ holds SyncHandle, drains SyncEvent
                         sync::Replica (owns NoteDocuments, Storage, VaultKey) ◄── sync::SyncEngine task
                         crypto ─ accounts client
```

- **`notebook_core::workspace`** (new, wasm-compatible): `Workspace` is the unlocked vault. It does **not** own documents: `sync-client-engine`'s `Replica` owns the `VaultKey`, `Storage`, and the `NoteId → NoteDocument` map, and performs every edit (`create_note`, `set_body`, `set_pinned/archived/trashed`, `purge`) with its own storage transaction and outbox entry. `Workspace` wraps the shared replica (`Shared<Replica>`: `Arc<Mutex<_>>` native, `Rc<RefCell<_>>` wasm, locks never held across `.await`) and the `SyncHandle`, and adds what the replica doesn't have: the tag tree, the search index, list derivation, and app-level rules. It exposes: `tree()`, `list(selection, query)`, `note(id)`, `create(selection_tag, now)`, `set_pinned/archived/trashed`, `purge`, `rename_tag(old, new, now)`, and a change-notification stream. Selection is `View(View) | Tag(TagPath)`. It derives list entries (id, title or empty, preview, updated, flags) so both clients render the same text. The replica loads every note, trashed and archived included, at startup; `Workspace::all_notes()` passes through `Replica::notes()`, which is the load-all path `markdown-export` relies on.
- **Change notifications:** the engine emits one `SyncEvent` channel (`Status`, `NotesChanged(ids)` for remote changes, `IntegrityWarning(id)`). Local edits produce no engine event, so `Workspace` records the ids it changed itself. Both sources update the tree and search index and come out of `Workspace` as one notification stream with a tree-changed flag.
- **`notebook_core::search`** (new): see "Search".
- **`notebook-app`** stays thin: validate input, call core, map errors, emit events. No vault logic in the app crate.
- **Alternative considered:** keep the decrypted model in the frontend (JS) and use Rust only for crypto/storage. Rejected: duplicates tag/view logic in TS, puts more plaintext in the webview, and the browser client would need it in TS anyway instead of reusing core.

### App state machine

`AppState` in `tauri::State` holds `RwLock<Phase>`:

- `SignedOut` – no account on this install.
- `Locked { account }` – account metadata known, no keys in memory, sync stopped.
- `Unlocked { account, workspace }` – the workspace holds the `SyncHandle`. On native the engine runs on a dedicated thread with a current-thread tokio runtime, as `sync-client-engine` specifies; a host task drains its event receiver. The sync status carries `SignInRequired` when refresh fails; the phase stays `Unlocked` so local edits continue (spec: Session expiry).

`app_state()` returns the phase (without secrets) so the frontend chooses which screen to render. Transitions emit `app-phase-changed`. On launch: no credential entry → `SignedOut`; entry with an auto-unlock key → unlock directly; entry without one → `Locked`.

### IPC surface

Commands (all `async`, all return `Result<Dto, AppError>`):

| group | commands |
|---|---|
| app | `app_state` |
| account | `sign_in`, `generate_recovery_key`, `register`, `recover`, `unlock`, `lock`, `change_passphrase`, `sign_out`, `pending_count` |
| vault | `tag_tree`, `list_notes(selection, query)`, `create_note(selection)`, `set_pinned`, `set_archived`, `set_trashed`, `delete_permanently`, `rename_tag` |
| editor | the host side defined by `note-editor-component` (open document, apply local update, close) |
| settings | `get_settings`, `set_vim_mode`, `set_tag_expanded` |

Events: `app-phase-changed`, `sync-status` (`{state, pending, attention}` where `state` is `synced | syncing | offline` and `attention` lists any of `sign_in_required`, `upgrade_required`, `quota_full`, `update_too_large`, `integrity_warning`, mapped from the engine's `SyncStatus`), `vault-changed` (`{changed: NoteId[], tree_changed: bool}`), and the editor's remote-update event.

- `AppError { code, message }` with stable `code` strings (`invalid_server_url`, `invalid_username`, `passphrase_too_short`, `passphrase_mismatch`, `bad_credentials`, `unreachable`, `rate_limited`, `invalid_invite`, `username_taken`, `recovery_key_required`, `recovery_key_malformed`, `recovery_key_wrong`, `wrong_passphrase`, `sign_in_required`, `offline`, `pending_changes`, `invalid_tag_name`, `trashed`, `not_trashed`, `locked`, `not_found`, `internal`; the list in `notebook-app/src/error.rs` is authoritative). `invalid_invite` matches the code `invite-codes` uses. A quota refusal during sync is not a command error: it appears in the sync status as the engine's `quota_full` attention flag. The frontend maps codes to messages; `bad_credentials` is the only code for unknown-user and wrong-passphrase (the OPAQUE layer already can't distinguish them).
- DTOs live in the app crate (`serde`), not in core, per the existing convention. Note ids cross as ULID strings.
- `vault-changed` carries ids, not content: the frontend re-queries the current list and tree. That keeps one source of truth and makes "live updates" (spec) a single code path for local and remote changes.
- **Alternative considered:** push full list deltas in events. Rejected as premature; re-querying a personal-scale list is cheap in-process.

The recovery key is generated by `generate_recovery_key`, which returns its display form and keeps the key in a short-lived pending-registration slot in `AppState`; `register` consumes it. This lets the UI show the key and collect acknowledgment *before* the account exists (spec), without the frontend ever sending the key back.

### TS platform adapter

`web/src/platform/` exports a `CoreApi` interface (typed methods mirroring the commands plus `on(event, handler)`), `tauri.ts` implementing it with `invoke`/`listen`, and `index.ts` selecting the implementation (`isTauri()`). Components only import `CoreApi`. Tests use a `FakeCoreApi`. `browser-web-client` adds `wasm.ts` later.

### Credentials and lock

One keychain service `com.blakfeld.notebook`, accessed with the `keyring` crate, version 4 (its default `v1` feature and `keyring::Entry`, which picks the platform store itself: macOS Keychain, Windows Credential Manager, and Linux Secret Service over the pure-Rust zbus client with `crypto-rust`), behind a `CredentialStore` trait with an in-memory implementation for tests and for the no-secure-store fallback. Entries (account key = install-scoped id):

| entry | contents | present when |
|---|---|---|
| `session` | server URL, username, device id, refresh token | signed in |
| `local-unlock` | Argon2id v1 salt + vault key wrapped under the derived key (vault-crypto envelope), bundle fingerprint | signed in, until invalidated |
| `auto-unlock` | the vault key | signed in and not locked |

- **Auto-unlock stores the vault key itself in the keychain.** Wrapping it under another key that also lives in the keychain adds nothing; the OS store is the boundary on desktop (spec: Credentials in the OS secure store).
- **Lock** zeroizes the `Workspace` (keys drop and zeroize), stops sync, deletes `auto-unlock`, and records `locked` in device settings (a UX hint only; the missing entry is what enforces it).
- **Unlock** with a passphrase: if the server is reachable, run OPAQUE login (fresh export key, fresh bundle), then rewrite `local-unlock` and `auto-unlock`. If unreachable, derive with Argon2id from `local-unlock` and unwrap. Argon2 runs in `spawn_blocking`. Online-first means a passphrase changed elsewhere is caught whenever possible.
- **Passphrase changed elsewhere:** `local-unlock` stores a fingerprint (SHA-256) of the key bundle it was derived alongside. Whenever the app obtains the server's current bundle (login, and the bundle check after each session refresh), a mismatch deletes `local-unlock`; the next unlock then requires the server (spec). In practice the server ends every other session on a passphrase change or recovery, so the device usually can't fetch the new bundle at all. The app therefore also **fails closed**: when sync reports `sign_in_required`, `local-unlock` is deleted, and the next unlock goes through the server, which checks the passphrase. A plain revocation costs one online unlock. `change_passphrase` likewise drops `local-unlock` before calling the server and rewraps it afterwards.
- **Fallback mid-write:** if the OS store fails partway (for example a keychain that locks during sign-in), the entries already written there are deleted and every write is replayed into memory, so lock and sign-out never miss one.
- **Change passphrase** on this device: OPAQUE change via `accounts`, then rewrite `local-unlock` with the new passphrase.
- **No secure store** (keyring returns a platform/no-backend error on first write): use the in-memory store for the session, emit a notice, never fall back to files.
- **Alternative considered:** unlock after lock requires the server. Rejected: an offline-first app that can't be opened offline after locking breaks the core promise, and mobile's auto-lock will make this path common.
- **Alternative considered:** Tauri Stronghold plugin. Rejected: it stores into a file protected by a password we'd have to get from somewhere, which is the problem we're solving; the OS keychain is the native answer.

### Local data

One SQLite database per install at `app_data_dir()/vault.sqlite3` via the `local-storage` SQLite implementation. Device-local settings (vim mode, tag expansion, locked hint) use the storage settings API. Collapsed tag paths are vault content, so they are stored sealed under the vault key (`Replica::set_sealed_setting`, envelope kind 17) and returned only while unlocked. Sign-out: confirm if `pending_count > 0`, best-effort session revoke (short timeout), delete keychain entries, close and delete the database file (and its WAL/SHM), return to `SignedOut`.

### Search

`notebook_core::search::SearchIndex` keeps, per note, the lowercased body (Unicode `to_lowercase`) in memory, updated from `vault-changed`. A query is split on Unicode whitespace, each term lowercased; a note matches when every term is a substring. `Workspace::list(selection, query)` filters the selection's ordered ids through the index, so results keep list order and are scoped to the selection (spec). The frontend debounces input (~150 ms).

- Linear scan over a few thousand notes of a few KB is well under a frame budget; an inverted index can come later behind the same API.
- **Alternative considered:** `tantivy`. Rejected: it persists a plaintext index to disk (violates "decrypted data stays in memory") and doesn't target wasm. Substring matching also handles CJK without a tokenizer.
- Diacritic folding is not done; it would add a normalization dependency and isn't in the spec.

### Note and tag operations

- **Create:** `Workspace::create` calls `Replica::create_note` with body `"#<tag path>\n"` when a tag is selected (multi-word segments written `#a b/c d#` per the tag syntax), else empty. Display spelling of the selected tag is used.
- **Trash read-only:** enforced in the frontend by opening the editor read-only *and* in core: `Workspace` (and the editor host path) rejects body changes to a trashed note before calling the replica, so a stale editor can't write.
- **Delete permanently:** only for trashed notes; `Workspace` checks the trashed flag, then awaits `SyncHandle::purge`, which deletes it on the server and then locally, and needs a connection (M1): offline it returns `offline` and nothing changes. The engine's `NotesChanged` for the purged id refreshes the tree and index. After a purge while `quota_full`, `SyncHandle::wake()` lets the engine retry the push sooner.
- **Rename tag:** `Workspace::rename_tag` runs `tags::rename` over all notes (including archived/trashed), then `Replica::set_body` on each affected note, producing one encrypted update per note through the outbox. Merging into an existing tag falls out of tag identity. Invalid names surface as `invalid_tag_name` before anything changes.

### Server address validation

`notebook_core::workspace::ServerUrl::parse` (shared with the browser) enforces the spec's scheme rule and normalizes away a trailing slash. Validation happens before any request.

### Networking and CSP

All server traffic goes through the sync engine's native transport and the `accounts` client in Rust, so the webview CSP is unchanged (`connect-src 'self' ipc: http://ipc.localhost`) and no Tauri HTTP plugin or extra capability is needed. The capability file gains only `core:event:default` if `listen` is not already covered by `core:default`.

### Frontend structure

- `App.svelte` switches on the phase: `SignIn`/`Register`/`Recover` screens, `Unlock`, or `Shell`.
- `Shell` = `Sidebar` (special views + `TagTree`), `NoteList` (search field, entries), `EditorPane` (note editor or empty state, note actions toolbar), `SyncIndicator`, `Settings` (modal with sections: Account, Editor, and room for `appearance` from `editor-live-preview` and Export from `markdown-export`).
- State in Svelte 5 rune modules under `web/src/lib/state/` (`phase`, `selection`, `notes`, `sync`, `settings`), each subscribing to adapter events.
- Visual design is minimal and system-font; `ui-designer` reviews layout before polish lands in `editor-live-preview`.

### Testing

- Core: unit tests for `Workspace` (views, create-under-tag, rename across notes, trash read-only, purge) using in-memory storage and a loopback/fake transport; `search` tests mirroring the spec scenarios.
- App crate: command tests on the mock runtime with the in-memory `CredentialStore`, covering phase transitions, lock/unlock (online and offline via a failing transport), passphrase-changed invalidation, sign-out cleanup, and that the data directory never contains a planted marker word.
- Integration: an app-crate test starts `notebook-server` in-process on an ephemeral port and drives register → create → second workspace syncs → rename → purge.
- Frontend: Vitest component/state tests against `FakeCoreApi`.
- Manual QA on macOS, Windows, and Linux for keychain behavior (Tauri WebDriver doesn't run on macOS, so no automated E2E yet).

## Risks / Trade-offs

- [Upstream interfaces may still shift before archive: `Workspace` relies on `sync-client-engine`'s `Replica` (`notes`, `note`, `create_note`, `set_body`, flag setters, `purge`), `SyncHandle` (`wake`, `flush`, `set_session`, `shutdown`), and the single `SyncEvent` receiver] → Task 1.1 re-checks them against the archived changes; mismatches are reported, not papered over in the app crate.
- [The editor binding writes to the same documents the replica owns] → The editor host path goes through `Replica` calls so every edit gets its storage transaction and outbox entry; how live edits reach it is `note-editor-component`'s design.
- [Detecting a passphrase change elsewhere needs an authenticated "get my key bundle" from `accounts`] → If `opaque-accounts` doesn't provide it, add it there (small, read-only) before task group 4; until then only login refreshes the fingerprint.
- [The vault key sits in the OS keychain while unlocked] → Same trust level as any password manager auto-unlock; lock removes it. Users who want more can lock.
- [Offline unlock trusts a local Argon2id wrap that can lag a passphrase change until the device next talks to the server] → Online-first unlock and fingerprint invalidation bound the window to the device's offline period.
- [Linux without Secret Service (headless, some tiling WMs)] → In-memory fallback with a notice; never a plaintext file.
- [Registration can't work against a production server until `invite-codes` is archived] → Tested against a test server configured to allow registration; the UI already sends the invite code.
- [Whole-list re-query on every change] → Fine at personal scale; event payload already carries ids for an incremental path later.
- [Linear search scan] → Same API can switch to an inverted index if profiles show it.

## Migration Plan

None. No user data exists yet; the first install creates the database and keychain entries. Rollback is uninstalling and removing the app data directory and keychain items.

## Open Questions

- Exact default window size and whether to persist window geometry (cosmetic, can be decided during implementation).
- The full set of global keyboard shortcuts beyond new note (`Mod+N`), focus search (`Mod+Shift+F`), and lock (`Mod+L`).
