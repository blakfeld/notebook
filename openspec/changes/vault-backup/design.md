# Design

## Context

See proposal.md (Why). Requirements are in `specs/vault-backup/spec.md`, `specs/app-shell/spec.md`, and `specs/local-storage/spec.md`.

After `local-only-desktop`, the app's `Open` phase holds a `Replica` (every `NoteDocument` loaded and decrypted in memory) over `SqliteStorage`, the vault key, and a `RecoveryEntry` stored in the database's `vault_meta`. The welcome screen has a slot for restore, and the sidebar footer has a slot where the sync indicator was. `NoteDocument::snapshot()` already produces an `EncryptedSnapshot` under the note's key, and `Storage::load_note` returns each note's wrapped key. `crypto::envelope` seals with context (vault ID, note ID, kind), and kinds 16, 17, and 32 to 34 are taken.

`markdown-export` plans a `YYYYMMDD-HHMMSS` stamp formatter and adds `tauri-plugin-dialog` with a Rust-side picker. Whichever of the two changes lands first introduces these, and the other reuses them.

## Goals / Non-Goals

**Goals:**
- A backup is a single self-contained file that a sync client can upload and that restores with only the recovery key.
- The provider learns nothing beyond file size and timing.
- Nothing the user already has is ever overwritten or deleted except old backups of this vault, and the current notes on an explicit, confirmed restore.

**Non-Goals:**
- Talking to Dropbox or any provider API. The folder is the whole integration.
- Incremental or deduplicated backups. Full snapshots are small at personal scale.
- Restoring a single note or merging a backup into the current notes.
- Backing up device-local settings (vim, theme, tag expansion) or attachments (none exist yet).
- Backups while the app is closed. There's no background service.

## Decisions

### 1. File format

```
offset  size  field
0       8     magic "NBBACKUP"
8       1     format version (1)
9       16    vault id
25      2     recovery entry length (u16 BE)
27      r     recovery entry (RecoveryEntry::to_bytes)
27+r    …     payload envelope (rest of file)
```

The payload envelope seals, under the **vault key** with a new envelope kind 48 bound to the vault ID, this plaintext:

```
32       SHA-256 of the header (bytes 0 to 27+r)
i64 BE   created_at (Unix ms)
u32 BE   note count
repeat:  note id (16) | u16 BE wrapped-key len | wrapped key | u32 BE snapshot len | encrypted snapshot
```

The header hash makes the cleartext header tamper-evident. The envelope context authenticates only the vault ID and kind, so without it a changed byte in the recovery entry would still open with the vault key.

The vault key is used directly with a distinct, authenticated kind, the same way it already wraps note keys (kind 16). An HKDF-derived subkey was considered and adds nothing, since the kind is authenticated and nonces are random 192-bit. The header's vault ID is authenticated through the envelope context, so swapping headers between vaults fails to open. The snapshots inside are still encrypted under each note key (double encryption). This keeps the payload a straight dump of what `local-storage` already accepts, so restore never handles plaintext outside a `NoteDocument`.

*Alternative:* a zip of per-note files. Rejected because it exposes note count and IDs, and needs an archive dependency for no gain. *Alternative:* copy the SQLite file. Rejected because it leaks IDs and sizes, depends on the schema version, and SQLite files copied mid-write by a sync client can be torn.

### 2. Core API (`notebook-core::backup`)

```rust
pub struct BackupNote { pub id: NoteId, pub wrapped_key: Vec<u8>, pub snapshot: EncryptedSnapshot }
// The vault is the recovery entry's.
pub fn build(recovery: &RecoveryEntry, vault_key: &VaultKey,
             created_at: Timestamp, notes: &[BackupNote]) -> Result<Vec<u8>, BackupError>;
pub struct BackupHeader { pub version: u8, pub vault: VaultId, pub recovery: RecoveryEntry }
impl BackupHeader { pub fn vault_key(&self, recovery: &RecoveryKey) -> Result<VaultKey, BackupError> } // WrongKey
pub fn read_header(bytes: &[u8]) -> Result<BackupHeader, BackupError>;   // NotABackup / TooNew / Damaged
pub fn open(bytes: &[u8], vault_key: &VaultKey) -> Result<OpenedBackup, BackupError>;
impl OpenedBackup { pub fn verify(&self, vault_key: &VaultKey) -> Result<(), BackupError> }  // unwrap every key, load every snapshot
```

`backup::file_name` and `BackupFileName` cover naming, and the shared stamp formatter is `notebook_core::time::stamp`. The writer and retention (Decisions 4 and 5) are `notebook_core::backup::folder`, compiled only for native targets, so the app only wires them up.

`verify` opens each note into a throwaway `NoteDocument` and checks it yields a `Note`. That is the "check everything before changing anything" step of restore. The module builds for wasm32, like the rest of core.

### 3. Building a backup from the open vault

Under the replica lock: for each loaded note, take `doc.snapshot()` and the wrapped key from storage, then release the lock. Encrypting the payload and writing the file happen on a blocking thread. Snapshots, not raw logs, keep the file compact and independent of the log layout, which `retire-sync-server` changes.

### 4. Writing: temp file then rename

Write to `.notebook-backup-<random>.partial` in the chosen folder with `create_new`, `fsync`, then hard-link it to the first free final name (`notebook-backup-<stamp>.nbbackup`, then ` 2`, …) and remove the partial. A hard link fails rather than replace a file a sync client created in the meantime; where hard links aren't supported (FAT, exFAT) it falls back to check-then-rename. Same-directory links and renames are atomic, so nothing under a backup name is ever partial (a sync client may still upload the hidden partial, which is ciphertext). Partial files with our prefix are deleted at the start of each backup. The folder's existence is checked before writing, and it is never created (spec "Backup folder"), so an unmounted cloud drive can't make us write into a stray local directory.

### 5. Retention

After a successful write, list the folder and keep only names matching `^notebook-backup-\d{8}-\d{6}( \d+)?\.nbbackup$`. Read each one's header (first 27 + r bytes) and keep only those for this vault. Sort by stamp then suffix, and delete beyond `backup.keep`. Dropbox "conflicted copy" files and renamed backups don't match the pattern and are never touched. A header read failure means skip that file, not delete it. Names and headers are unauthenticated, so the backup just written is never deleted and files stamped more than a day past now are ignored; otherwise planted or clock-skewed future-dated files would sort as newest and push out every real backup.

### 6. Scheduling

A tokio task in `notebook-app`, started on entering `Open`, ticks every 10 minutes and once at startup. It backs up when a folder is set, auto is on, `backup.dirty` is true, and `now - backup.last >= interval`. Any replica change event sets `backup.dirty` (written to device-local settings at most once per tick). A successful backup clears it and sets `backup.last`. Choosing a folder for the first time triggers an immediate backup. Settings keys (device-local, non-sensitive): `backup.folder`, `backup.auto` (default true once a folder is chosen), `backup.interval` (`hourly|daily|weekly`, default `daily`), `backup.keep` (default 14), `backup.last`, `backup.last_error`, `backup.dirty`. Failing means `last_error` is set from the latest attempt. Overdue means dirty and `now - last > 2 × interval`. A `backup-status-changed` event drives the UI.

### 7. Restore

1. Rust-side file picker → read the file → `read_header`.
2. If the header's vault equals the open vault, use the in-memory vault key. Otherwise the UI asks for the recovery key, and `RecoveryKey::parse` (mistyped) then `header.recovery.open` (wrong key) give the vault key.
3. `open` + `verify`. Failures here change nothing.
4. From Settings with a folder set: write a safety backup first (Decision 4). On failure, ask to confirm again.
5. Build `vault.sqlite3.restoring` with `SqliteStorage::open`, set the recovery entry, and `import_note` every note, then close it.
6. Leave `Open`, rename `vault.sqlite3` → `vault.sqlite3.before-restore` (replacing any older one), rename `.restoring` → `vault.sqlite3`, write the keychain `vault-key`, and reopen. If the keychain write fails, roll the renames back. Without a secure store, the next launch asks for the recovery key, as `local-only-desktop` specifies.

Keeping one `before-restore` database is a local undo of last resort. It is ciphertext and is replaced on the next restore.

### 8. Storage `import_note`

`Storage::import_note(id, wrapped_key, snapshot)` inserts the key and a base snapshot in one transaction, with no outbox entry and nothing pending. It is implemented for SQLite and memory and covered by the conformance suite. It is used only on a fresh store during restore.

### 9. Frontend

The adapter gains `chooseBackupFolder()`, `backupNow()`, `backupStatus()`, `setBackupOptions({auto, interval, keep})`, `pickBackup()` (returns header info: same-vault or needs key), and `restoreBackup({recoveryKey?})`. The Backup section in `Settings.svelte`, the welcome restore flow in `lib/setup/`, and `shell/BackupNotice.svelte` in the sidebar footer consume them. Paths are only ever displayed and never sent back from JS.

## Risks / Trade-offs

- [The user loses the recovery key and the computer at once] → Backups are unrecoverable by design. The settings copy says so, and markdown export remains the plaintext escape hatch.
- [Old backups keep permanently deleted notes] → Expected for backups and stated in `app-shell` "Delete permanently". Retention ages them out.
- [Sync client uploads while the app is renaming or pruning] → Each file is written once and never modified, so the worst case is a deleted file being re-uploaded and then pruned again.
- [Backups only while running] → Users who rarely open the app back up rarely, but they also change little. Overdue only counts when dirty.
- [Whole vault in memory twice during build] → Fine at personal scale. Revisit with attachments.
- [The folder path in device-local settings reveals the user's home path] → Non-sensitive by the settings rule, and never synced or logged beyond the file name.

## Migration Plan

Additive. No backups exist until the user chooses a folder. Format version 1. A later format bumps the version byte, and older apps report "needs a newer version".
