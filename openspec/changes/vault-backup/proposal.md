# Proposal

## Why

Once Notebook is local-only (`local-only-desktop`), a lost or broken computer means lost notes unless a copy lives somewhere else. Users already run Dropbox, iCloud Drive, OneDrive, or Syncthing, which sync a folder. If the app writes encrypted backups into such a folder, the user gets off-device safety and history with no account, no API keys, and no server. The provider only ever sees ciphertext.

## What Changes

- The user picks a **backup folder** in Settings, typically inside a synced folder like `~/Dropbox`. The folder picker runs on the Rust side, so the webview never supplies a path.
- **Automatic backups**: while the app runs, it writes a backup when notes have changed and the chosen interval (hourly, daily by default, or weekly) has passed since the last one. A **Back up now** action does it on demand.
- Each backup is one file, `notebook-backup-YYYYMMDD-HHMMSS.nbbackup`, holding:
  - a small cleartext header (format version, vault ID)
  - the recovery entry (the vault key wrapped under the recovery key)
  - one sealed payload with every note's wrapped key and an encrypted snapshot of its current state
  
  Note count, IDs, and sizes are hidden inside the payload. Files are written to a temporary name and renamed, so the sync client never uploads a half-written backup.
- **Retention**: the app keeps the newest N backups of this notebook in the folder (default 14) and deletes older ones. It only ever deletes files it recognizes as its own backups of this vault.
- **Restore**: from the welcome screen on a new computer, or from Settings to replace the current notes. The user picks a backup file and enters the recovery key. A backup of this device's own vault restores without the key. Restore checks the whole backup before changing anything and is all-or-nothing. Restoring from Settings first writes a backup of the current notes.
- **Status**: Settings shows the last successful backup, the next one due, and the last error. A quiet sidebar notice appears only when backups are failing or overdue, for example when the Dropbox folder is unavailable.
- Backup format logic (build, parse, verify) lives in `notebook-core`. File writing, scheduling, and pickers live in `notebook-app`.

## Capabilities

### New Capabilities
- `vault-backup`: encrypted whole-vault backup files, covering format, confidentiality and integrity, naming, atomic writes, retention, scheduling, verification, and all-or-nothing restore with the recovery key.

### Modified Capabilities
- `app-shell`: the welcome screen gains "Restore from backup". Settings gains a Backup section (folder, interval, keep count, back up now, status, restore). The sidebar gains a backup-problem notice.
- `local-storage`: adds importing a note as a wrapped key plus base snapshot, used to fill a fresh store during restore.

## Impact

- `notebook-core`: new `backup` module (format, `build`, `parse`, `open`). It reuses `crypto` envelopes (a new payload kind under the vault key), `RecoveryEntry`, and the document snapshot encoding.
- `notebook-storage-sqlite` and `storage::memory`: an `import_note` operation, plus conformance tests.
- `notebook-app`: backup scheduler task, `backup_now` / `backup_status` / `choose_backup_folder` / `restore_backup` commands, `tauri-plugin-dialog` (shared with `markdown-export`, whichever lands first adds it), and settings keys `backup.folder`, `backup.interval`, `backup.keep`, `backup.last`.
- `web/`: Backup settings section, restore flow on the welcome screen and in Settings, sidebar notice, adapter methods.
- `docs/backup-format.md` documents the file format.
- Depends on `local-only-desktop` (welcome screen, recovery entry, no lock). Core work (format, storage import) can start before that change is archived.
