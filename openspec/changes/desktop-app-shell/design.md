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

- `AppError { code, message }` with stable `code` strings (`invalid_server_url`, `bad_credentials`, `unreachable`, `rate_limited`, `invalid_invite`, `username_taken`, `quota_exceeded`, `recovery_key_malformed`, `recovery_key_wrong`, `wrong_passphrase`, `offline`, `invalid_tag_name`, `locked`, `not_found`, `internal`). `invalid_invite` and `quota_exceeded` match the codes `invite-codes` uses; A quota refusal during sync is not a command error: it appears in the sync status as the engine's `quota_full` attention flag. The frontend maps codes to messages; `bad_credentials` is the only code for unknown-user and wrong-passphrase (the OPAQUE layer already can't distinguish them).
- DTOs live in the app crate (`serde`), not in core, per the existing convention. Note ids cross as ULID strings.
- `vault-changed` carries ids, not content: the frontend re-queries the current list and tree. That keeps one source of truth and makes "live updates" (spec) a single code path for local and remote changes.
- **Alternative considered:** push full list deltas in events. Rejected as premature; re-querying a personal-scale list is cheap in-process.

The recovery key is generated by `generate_recovery_key`, which returns its display form and keeps the key in a short-lived pending-registration slot in `AppState`; `register` consumes it. This lets the UI show the key and collect acknowledgment *before* the account exists (spec), without the frontend ever sending the key back.

### TS platform adapter

`web/src/platform/` exports a `CoreApi` interface (typed methods mirroring the commands plus `on(event, handler)`), `tauri.ts` implementing it with `invoke`/`listen`, and `index.ts` selecting the implementation (`isTauri()`). Components only import `CoreApi`. Tests use a `FakeCoreApi`. `browser-web-client` adds `wasm.ts` later.

### Credentials and lock

One keychain service `com.blakfeld.notebook`, accessed with the `keyring` crate (macOS Keychain, Windows Credential Manager, Linux Secret Service via `sync-secret-service` + `crypto-rust`), behind a `CredentialStore` trait with an in-memory implementation for tests and for the no-secure-store fallback. Entries (account key = install-scoped id):

| entry | contents | present when |
|---|---|---|
| `session` | server URL, username, device id, refresh token | signed in |
| `local-unlock` | Argon2id v1 salt + vault key wrapped under the derived key (vault-crypto envelope), bundle fingerprint | signed in, until invalidated |
| `auto-unlock` | the vault key | signed in and not locked |

- **Auto-unlock stores the vault key itself in the keychain.** Wrapping it under another key that also lives in the keychain adds nothing; the OS store is the boundary on desktop (spec: Credentials in the OS secure store).
- **Lock** zeroizes the `Workspace` (keys drop and zeroize), stops sync, deletes `auto-unlock`, and records `locked` in device settings (a UX hint only; the missing entry is what enforces it).
- **Unlock** with a passphrase: if the server is reachable, run OPAQUE login (fresh export key, fresh bundle), then rewrite `local-unlock` and `auto-unlock`. If unreachable, derive with Argon2id from `local-unlock` and unwrap. Argon2 runs in `spawn_blocking`. Online-first means a passphrase changed elsewhere is caught whenever possible.
- **Passphrase changed elsewhere:** `local-unlock` stores a fingerprint (SHA-256) of the key bundle it was derived alongside. Whenever the app obtains the server's current bundle (login, and the bundle check after each session refresh), a mismatch deletes `local-unlock`; the next unlock then requires the server (spec). This needs `accounts` to let an authenticated client read its current key bundle; see Risks.
- **Change passphrase** on this device: OPAQUE change via `accounts`, then rewrite `local-unlock` with the new passphrase.
- **No secure store** (keyring returns a platform/no-backend error on first write): use the in-memory store for the session, emit a notice, never fall back to files.
- **Alternative considered:** unlock after lock requires the server. Rejected: an offline-first app that can't be opened offline after locking breaks the core promise, and mobile's auto-lock will make this path common.
- **Alternative considered:** Tauri Stronghold plugin. Rejected: it stores into a file protected by a password we'd have to get from somewhere, which is the problem we're solving; the OS keychain is the native answer.

### Local data

One SQLite database per install at `app_data_dir()/vault.sqlite3` via the `local-storage` SQLite implementation. Device-local settings (vim mode, tag expansion, locked hint) use the storage settings API. Sign-out: confirm if `pending_count > 0`, best-effort session revoke (short timeout), delete keychain entries, close and delete the database file (and its WAL/SHM), return to `SignedOut`.

### Search

`notebook_core::search::SearchIndex` keeps, per note, the lowercased body (Unicode `to_lowercase`) in memory, updated from `vault-changed`. A query is split on Unicode whitespace, each term lowercased; a note matches when every term is a substring. `Workspace::list(selection, query)` filters the selection's ordered ids through the index, so results keep list order and are scoped to the selection (spec). The frontend debounces input (~150 ms).

- Linear scan over a few thousand notes of a few KB is well under a frame budget; an inverted index can come later behind the same API.
- **Alternative considered:** `tantivy`. Rejected: it persists a plaintext index to disk (violates "decrypted data stays in memory") and doesn't target wasm. Substring matching also handles CJK without a tokenizer.
- Diacritic folding is not done; it would add a normalization dependency and isn't in the spec.

### Note and tag operations

- **Create:** `Workspace::create` calls `Replica::create_note` with body `"#<tag path>\n"` when a tag is selected (multi-word segments written `#a b/c d#` per the tag syntax), else empty. Display spelling of the selected tag is used.
- **Trash read-only:** enforced in the frontend by opening the editor read-only *and* in core: `Workspace` (and the editor host path) rejects body changes to a trashed note before calling the replica, so a stale editor can't write.
- **Delete permanently:** only for trashed notes; `Workspace` checks the trashed flag, then calls `Replica::purge` (which queues the purge and removes the note locally) and `SyncHandle::wake()`. After a purge while `quota_full`, the wake also lets the engine retry the push sooner.
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
