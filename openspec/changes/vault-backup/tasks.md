# Tasks

Groups 1–3 are core-only and can start before `local-only-desktop` is archived (they need only its `RecoveryEntry`, task 1.1 there). Groups 4–6 need `local-only-desktop` archived.

## 1. Storage import

- [ ] 1.1 Add `Storage::import_note(id, wrapped_key, snapshot)` to the trait, the memory store, and `SqliteStorage` (one transaction, no outbox entry, not pending), and add conformance cases; verify the "Import a note from a snapshot" scenarios pass against both stores via `cargo test -p notebook-core --features conformance` and `cargo test -p notebook-storage-sqlite`

## 2. Backup format in notebook-core

- [ ] 2.1 Add the `backup` module with the header layout, `BackupError` (`NotABackup`, `TooNew`, `Damaged`, `WrongKey`), and `read_header`; verify unit tests for "Not a backup", "Newer format", and truncated headers
- [ ] 2.2 Implement `build` and `open` (payload kind 48 under the vault key, bound to the vault ID) and `OpenedBackup::verify`; verify tests for every scenario under "Backup contents", "Backups are encrypted" (marker string and note-ID bytes absent from the file), and "Backups are tamper-evident" (every single-byte flip, every truncation length, header vault swapped)
- [ ] 2.3 Add the shared `YYYYMMDD-HHMMSS` UTC stamp formatter (reuse `markdown-export`'s if it has landed, otherwise add it to a core `time` module that export then reuses) and the backup file-name and retention name pattern helpers; verify unit tests for the "Name from time" scenario and pattern matching of suffixed, conflicted-copy, and unrelated names
- [ ] 2.4 Write `docs/backup-format.md` (layout, encryption, what is visible in the clear, restore steps) and module rustdoc; verify `cargo doc -p notebook-core --no-deps` is warning-free and the documented offsets match a test that parses a built file by hand

## 3. File writing and retention (native, notebook-app or a core native module)

- [ ] 3.1 Implement the temp-then-rename writer with free-name resolution, partial-file cleanup, and the folder-must-exist check; verify `tempfile` tests for "Folder chosen", "Folder unavailable" (no folder created), "Failure mid-write", and an existing name getting a ` 2` suffix
- [ ] 3.2 Implement retention by pattern and header vault; verify tests for "Old backups pruned" and "Other files untouched" (another vault's backups, a renamed backup, a Dropbox conflicted copy, an unreadable file)

## 4. App wiring (needs local-only-desktop)

- [ ] 4.1 Add `tauri-plugin-dialog` if `markdown-export` hasn't, with Rust-side folder and file pickers and no JS dialog permission; implement `choose_backup_folder`, `set_backup_options`, `backup_now`, and `backup_status` commands and the `backup-status-changed` event over the device-local `backup.*` settings; verify mock-runtime tests for each command with the picker stubbed
- [ ] 4.2 Implement the scheduler task (10-minute tick, dirty flag from replica changes, interval check, immediate first backup); verify tests with a manual clock for "Due backup runs", "Nothing changed", "First backup", and failing and overdue status
- [ ] 4.3 Implement restore (header → same-vault or recovery key → open and verify → optional safety backup → build `.restoring` → swap with `before-restore` → keychain → reopen, with rollback); verify `vault_tests.rs` tests for "Full round trip" on an empty device, "Restore over existing notes", "Wrong recovery key", "Damaged backup", "Same-vault restore needs no key", a keychain failure rolling back, and a log-capture check that no log line holds the entered key

## 5. Frontend (needs local-only-desktop)

- [ ] 5.1 Add the backup adapter methods to `web/src/platform` (types, Tauri, fake, demo); verify adapter unit tests and `npm run check`
- [ ] 5.2 Add "Restore from backup" to the welcome screen (picker, recovery key input with mistyped-key feedback, progress, errors); verify component tests for "New computer", "Cancelled picker", and a mistyped key
- [ ] 5.3 Add the Backup section to Settings (folder, auto toggle, interval, keep count, status, Back up now, encryption note, "Restore from backup…" with replace confirmation and safety backup); verify component tests for "No folder yet", "Status shown", "Safety backup first", and "Cancelled confirmation"
- [ ] 5.4 Add `BackupNotice.svelte` in the sidebar footer, shown only when failing or overdue and opening Backup settings; verify component tests for "Dropbox folder gone" and "Healthy"

## 6. Integration

- [ ] 6.1 End-to-end on desktop with a real Dropbox (or iCloud Drive) folder: choose the folder, edit notes, confirm automatic and manual backups appear and upload, unmount or rename the folder and see the sidebar notice, restore the backup on a second machine or a fresh data directory with the recovery key; verify by recording results in the apply summary
- [ ] 6.2 Run the definition-of-done checks from `CLAUDE.md` and `openspec validate vault-backup --strict`; verify all exit 0

## Workflow follow-up

- Review with `code-reviewer` and `security-analyst` (format, key use, file handling, restore swap), then archive with `/opsx:archive` after `local-only-desktop` is archived.
