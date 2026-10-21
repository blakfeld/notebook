# TODO

The OpenSpec changes for Notebook, in priority order. Each one lives in
`openspec/changes/<name>/`. Work top to bottom. Archive each change before
starting any later change that modifies its capability.

Milestones mark the points where something usable exists.

Status: `[ ]` not started, `[~]` in progress (see note), `[x]` done and archived.

## Foundation

1. [~] `scaffold-workspace`: Cargo workspace (core, server, app), TS frontend, CI, wasm32 build check
   - Implemented: 16/16 tasks done, `scripts/check.sh` passes. Next: review, first commit, `/opsx:archive`.
2. [ ] `note-model-and-tags`: note model, Bear-style nested inline tags, derived tag tree, tag rename
3. [ ] `vault-encryption`: key hierarchy, per-note keys, XChaCha20-Poly1305, recovery key
4. [ ] `crdt-note-documents`: per-note CRDT docs (Yrs), encrypted updates and snapshots
5. [ ] `local-sqlite-storage`: Storage trait, SQLite (native) and in-memory implementations

## Server

6. [ ] `server-foundation`: axum binary, config, SQLite, static hosting, rate limits, security headers, operator CLI
7. [ ] `opaque-accounts`: OPAQUE register/login, sessions, server-stored encrypted key bundle, recovery
8. [ ] `sync-server`: encrypted op log, bootstrap, incremental pull, WebSocket live updates, compaction

## Sync and first client

9. [ ] `sync-client-engine`: outbox, push/pull, bootstrap, live mode, snapshotting, sync status
10. [ ] `note-editor-component`: CodeMirror 6 markdown editor, CRDT binding, optional per-device vim mode
11. [ ] `desktop-app-shell`: Tauri desktop app with sign-in, tag sidebar, note list, editor, search

> **Milestone: usable on desktop.** Single user, account created from the server CLI.

12. [ ] `markdown-export`: export all notes as markdown (folder on native, zip in browser)

> Done early because with E2EE, export is the user's only backup if keys are lost.

## Edit from anywhere

13. [ ] `browser-web-client`: thin WASM browser client, in-memory sessions, nothing persisted

> **Milestone: edit the same note on desktop and in the browser.** This is the core promise.

14. [ ] `invite-codes`: invite-gated registration, per-vault storage quotas

> **Milestone: safe to let other people onto the public server.**

## Polish and reach

15. [ ] `editor-live-preview`: beautiful editor with hidden syntax, typography, themes, focus mode
16. [ ] `mobile-app`: Tauri iOS/Android, stacked navigation, touch toolbar, biometric unlock

## Notes on ordering

- `invite-codes` comes after the browser client because accounts can be created
  from the server CLI until others need to sign up. It modifies `accounts`, so
  `opaque-accounts` must be archived first.
- `markdown-export` modifies `app-shell`, so it follows `desktop-app-shell`.
- `editor-live-preview` modifies `note-editor`. `mobile-app` modifies `app-shell`
  and benefits from live preview being in place.
- Items 2-5 and 6-7 don't depend on each other and can be worked in parallel.

## Later / out of scope for now

- Attachments and images (encrypted blobs, `attachments/` folder in export)
- Import from Bear / markdown folders (round-trip of the export format)
- Vim-style navigation outside the editor (`j`/`k` in the note list, `/` search)
- Synced (non-device) settings such as theme and font
- Encrypted index snapshot to speed up browser login on large vaults
- Sharing notes with other users (public-key crypto)
- In-app invite creation for admins (`is_admin` flag exists from `opaque-accounts`)
