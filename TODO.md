# TODO

The OpenSpec changes for Notebook, grouped into waves. Each one lives in
`openspec/changes/<name>/` (archived ones in `openspec/changes/archive/`).

Changes within a wave don't depend on each other and can be worked in
parallel. A wave starts once the changes it depends on are archived; the
"needs" line on each change says exactly which, so a change can start before
the rest of its previous wave is done. An "archive after" line means the change
can be built in parallel, but its final checks need the named change first.
Archive each change before starting any later change that modifies its
capability.

Milestones mark the points where something usable exists.

Status: `[ ]` not started, `[~]` in progress (see note), `[x]` done and archived.

## Done

- [x] `scaffold-workspace`: Cargo workspace (core, server, app), TS frontend, CI, wasm32 build check
- [x] `note-model-and-tags`: note model, Bear-style nested inline tags, derived tag tree, tag rename
- [x] `vault-encryption`: key hierarchy, per-note keys, XChaCha20-Poly1305, recovery key
- [x] `crdt-note-documents`: per-note CRDT docs (Yrs), encrypted updates and snapshots
- [x] `local-sqlite-storage`: Storage trait, SQLite (native) and in-memory implementations
- [x] `server-foundation`: axum binary, config, SQLite, static hosting, rate limits, security headers, operator CLI
- [x] `opaque-accounts`: OPAQUE register/login, sessions, server-stored encrypted key bundle, recovery
- [x] `sync-server`: encrypted op log, bootstrap, incremental pull, WebSocket live updates, compaction
- [x] `sync-client-engine`: outbox, push/pull, bootstrap, live mode, snapshotting, sync status

## Wave 1 (can start now)

- [x] `note-editor-component`: CodeMirror 6 markdown editor, CRDT binding, optional per-device vim mode
  - Open manual checks: Firefox and Safari under the server CSP (tasks 9.1), `cargo tauri dev` (9.2).
- [x] `invite-codes`: invite-gated registration, per-vault storage quotas
  - Needs: nothing outstanding. Server and `notebook-core::auth`; no UI.

> `invite-codes` adds `invite_code` to `Registration::start`, which the
> `desktop-app-shell` register screen calls. Landing it first avoids reworking
> that screen; if it lands second, it updates the call site.

## Wave 2

- [ ] `desktop-app-shell`: Tauri desktop app with sign-in, tag sidebar, note list, editor, search
  - Needs: `note-editor-component`. Should follow `invite-codes` (see above).

> **Milestone: usable on desktop.** Accounts created from the server CLI.
>
> **Milestone: safe to let other people onto the public server** once
> `invite-codes` is also archived.

## Wave 3

- [ ] `markdown-export`: export all notes as markdown (folder on native, zip in browser)
  - Needs: `desktop-app-shell` (modifies `app-shell`).
  - Do this first if only one change is in flight: with E2EE, export is the user's only backup if keys are lost.
- [ ] `editor-live-preview`: beautiful editor with hidden syntax, typography, themes, focus mode
  - Needs: `note-editor-component` (modifies `note-editor`), `desktop-app-shell` (Settings view).
  - Archive after `browser-web-client`: several of its checks run in the browser, and the
    browser's preference allowlist (`prefs.ts`) must gain `editor.sourceView` and `editor.focusMode`.

## Wave 4

- [ ] `browser-web-client`: thin WASM browser client, in-memory sessions, nothing persisted
  - Needs: `desktop-app-shell`, `markdown-export`. Does not need `editor-live-preview`.
- [ ] `mobile-app`: Tauri iOS/Android, stacked navigation, touch toolbar, biometric unlock
  - Needs: `desktop-app-shell` and `markdown-export` (modifies both capabilities), `editor-live-preview`.
  - Archive after `browser-web-client`: its 390 px browser check and desktop/browser/phone
    convergence check need the browser client.

> **Milestone: edit the same note on desktop and in the browser.** This is the core promise.

## Parallel work notes

- Wave 1 changes don't overlap: `note-editor-component` is frontend plus core
  `NoteDocument`/`Replica`; `invite-codes` is server plus core auth.
- Wave 3 changes modify different capabilities but both add to the Settings
  view; expect a small merge in `web/`.
- Wave 4 changes don't modify each other's capabilities. `mobile-app` modifies
  `app-shell`, which `browser-web-client` reuses as-is, so rebase the browser
  client onto any `app-shell` change from mobile (the stacked layout applies to
  narrow browser windows too).

## Later / out of scope for now

- Attachments and images (encrypted blobs, `attachments/` folder in export)
- Import from Bear / markdown folders (round-trip of the export format)
- Vim-style navigation outside the editor (`j`/`k` in the note list, `/` search)
- Synced (non-device) settings such as theme and font
- Encrypted index snapshot to speed up browser login on large vaults
- Sharing notes with other users (public-key crypto)
- In-app invite creation for admins (`is_admin` flag exists from `opaque-accounts`)
