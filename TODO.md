# TODO

The OpenSpec changes for Notebook, in priority order and grouped into waves.
Each one lives in `openspec/changes/<name>/` (archived ones in
`openspec/changes/archive/`).

**Direction (2026-10-22):** Notebook is a local-only desktop app. There is no
sync server, no accounts or logins, and no browser or mobile client. Notes are
encrypted on disk with a vault key kept in the OS keychain. Off-device safety
comes from encrypted backups written to a folder the user picks, such as
Dropbox, iCloud Drive, OneDrive, or Syncthing, and restored with the recovery
key. Markdown export is the readable escape hatch.

Changes within a wave don't depend on each other and can be worked in
parallel; they are listed highest priority first. A wave starts once the
changes it depends on are archived; the "needs" line on each change says
exactly which. An "archive after" line means the change can be built in
parallel, but its final checks or archive need the named change first.

Milestones mark the points where something usable exists.

Status: `[ ]` not started, `[~]` in progress (see note), `[x]` done and archived.

## Done

- [x] `scaffold-workspace`: Cargo workspace (core, server, app), TS frontend, CI, wasm32 build check
- [x] `note-model-and-tags`: note model, Bear-style nested inline tags, derived tag tree, tag rename
- [x] `vault-encryption`: key hierarchy, per-note keys, XChaCha20-Poly1305, recovery key
- [x] `crdt-note-documents`: per-note CRDT docs (Yrs), encrypted updates and snapshots
- [x] `local-sqlite-storage`: Storage trait, SQLite (native) and in-memory implementations
- [x] `server-foundation`: axum binary, config, SQLite, static hosting, rate limits, security headers, operator CLI *(retired by `retire-sync-server`)*
- [x] `opaque-accounts`: OPAQUE register/login, sessions, server-stored encrypted key bundle, recovery *(retired)*
- [x] `sync-server`: encrypted op log, bootstrap, incremental pull, WebSocket live updates, compaction *(retired)*
- [x] `sync-client-engine`: outbox, push/pull, bootstrap, live mode, snapshotting, sync status *(retired)*
- [x] `note-editor-component`: CodeMirror 6 markdown editor, CRDT binding, optional per-device vim mode
  - Open manual check: `cargo tauri dev` (9.2). The Firefox/Safari server-CSP check (9.1) no longer applies.
- [x] `invite-codes`: invite-gated registration, per-vault storage quotas *(retired)*
- [x] `desktop-app-shell`: Tauri desktop app with sign-in, tag sidebar, note list, editor, search
  - Linux and Windows manual checks not done.
- [x] `local-only-desktop`: no logins; first launch creates a notebook, vault key in the keychain, recovery key shown once, upgrade path for server-backed installs, no network access
- [x] `vault-backup`: encrypted whole-vault backups into a chosen (synced) folder, automatic and on demand, retention, status and sidebar notice, restore with the recovery key from the welcome screen or Settings
  - Open question: after a restore on a second machine both share a vault ID, so if both back up into the same
    folder, each one's retention prunes the other's backups.
  - Deliberate choices: every folder choice (not only the first) backs up right away; restore carries over the
    backup and editor device settings but not `backup.last`; a compromised webview lowering the keep count is an
    accepted risk (see the archived design.md).

## Wave 1

1. [~] `editor-live-preview`: hidden syntax, typography, themes, focus mode, source view
   - In progress, 20 of 26 tasks done. Implementation from `origin/worktree-mellow-bubbling-swing` (`31cd30c`)
     brought onto post-pivot main (uncommitted in `worktree-warm-wiggling-pearl`), touch-tap link opening removed,
     code-reviewed and review findings fixed; fmt, clippy, and web checks pass.
   - All code is written. Left: the manual desktop-app checks in 2.4, 4.4, 6.1, 6.3, 7.1, and 7.3 (CJK IME and
     real-webview typing feel; jsdom timings are in `tasks.md`), then a security-analyst pass on the opener
     capability and `set_preference`, then archive.
   - Needs: nothing outstanding. Updated for the pivot: no touch tap, no remote-edit requirement, no browser checks.
2. [ ] `markdown-export`: export all notes as markdown into a chosen folder
   - Needs: nothing outstanding. Updated for the pivot: zip/browser output removed.
   - Shares `tauri-plugin-dialog` and the `YYYYMMDD-HHMMSS` stamp formatter with `vault-backup`; whichever lands first adds them.
     Both now exist on main (`notebook_core::time::stamp`, the dialog plugin).

> **Milestone: no more logins (reached 2026-10-23).** The app opens straight
> into the notes and never touches the network. Don't shut down
> the old server until devices have upgraded (and, ideally, backed up).

## Wave 2

1. [ ] `retire-sync-server`: delete the server crate, auth, sync engine, transports, storage outbox and cursor, key bundle; local compaction; CI, docs, and agents for desktop-only
   - Needs: nothing outstanding (`local-only-desktop` and `vault-backup` are archived).

> **Milestone: safe local-only app (reached 2026-10-23).** Encrypted backups
> land in Dropbox (or any synced folder) automatically and restore on a new
> computer with the recovery key. This is the core promise after the pivot.

## Parallel work notes

- **Wave 1 overlap.**
  - `local-only-desktop` rewrites `App.svelte` routing, `lib/account/` → `lib/setup/`,
    `Settings.svelte`, `platform/*`, and `notebook-app` state and commands.
  - `editor-live-preview` and `markdown-export` both add sections to `Settings.svelte` and methods to `platform/*`.
  - Expect small merges there; land `local-only-desktop` first if possible.
  - `vault-backup` core is Rust-only (`notebook-core::backup`, the storage trait) and conflicts with none of them.
- **Wave 2 overlap.**
  - `retire-sync-server` reshapes the `Storage` trait, moves `Replica` to `notebook_core::replica`,
    and touches every crate's `Cargo.toml`.
  - `vault-backup` adds `import_note` to the same trait. Rebase whichever is second; the conflict is mechanical.
- **Specs.**
  - `app-shell` is modified by `local-only-desktop`, `vault-backup`, and `markdown-export`,
    each through different requirements, so they archive in any order.
  - `local-storage` is modified by `vault-backup` (ADDED import) and `retire-sync-server`
    (removals and rewrites), which also don't collide.
- **After `retire-sync-server` is archived:**
  - Delete the five empty retired spec directories (`accounts`, `invites`, `server-hosting`, `sync-protocol`, `sync-client`).
  - Fix the Purpose lines of `app-shell`, `local-storage`, and `vault-crypto`.

## Later / out of scope for now

- Browser client and mobile app: shelved. Their proposals are in git history
  (`openspec/changes/browser-web-client`, `openspec/changes/mobile-app`, last at
  commit `ee4be1e`). `notebook-core` keeps building for wasm32 so a browser
  client stays possible.
- App lock with OS authentication (Touch ID / Windows Hello / password)
- Rotate the recovery key from Settings (old backups keep needing the old key)
- Direct Dropbox (or other provider) API upload, for users without a sync client
- Multi-desktop use through the synced folder (merge CRDT updates from backups or a shared op folder)
- Remove the legacy server-install upgrade path (`LocalUnlock`, Argon2id) once no such installs remain
- Attachments and images (encrypted blobs in backups, `attachments/` folder in export)
- Import from Bear / markdown folders (round-trip of the export format)
- Vim-style navigation outside the editor (`j`/`k` in the note list, `/` search)
