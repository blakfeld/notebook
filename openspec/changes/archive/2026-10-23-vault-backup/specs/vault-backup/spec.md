# Spec Delta

## Purpose

Defines encrypted whole-vault backups written to a user-chosen folder, typically one a sync service like Dropbox uploads, and how they are named, kept, verified, and restored with the recovery key.

## ADDED Requirements

### Requirement: Backup contents
A backup SHALL contain everything needed to rebuild the notebook on another computer: the vault ID, the recovery entry, and, for every note including archived and trashed notes, its ID, wrapped note key, and an encrypted snapshot of its current state. Permanently deleted notes SHALL NOT be included.

#### Scenario: Full round trip
- **WHEN** a notebook with active, archived, pinned, and trashed notes is backed up and the backup is restored on an empty device with the recovery key
- **THEN** every note is present with the same ID, body, timestamps, and pinned, archived, and trashed states

### Requirement: Backups are encrypted
A backup file SHALL reveal in the clear only its format version, its vault ID, and the recovery entry. Every other byte, including note IDs, note count, and per-note sizes, SHALL be inside an authenticated ciphertext that opens only with the vault key. No plaintext note content, title, or tag SHALL appear in the file.

#### Scenario: Marker text not in the file
- **WHEN** a note containing a distinctive marker string is backed up
- **THEN** the marker does not appear in the backup file, and neither does the note's ID in any encoding

### Requirement: Backups are tamper-evident
Opening a backup SHALL fail if any byte has been changed, the file is truncated, or the file belongs to a different vault than its header claims. The failure SHALL NOT return partial contents.

#### Scenario: Flipped byte
- **WHEN** any single byte of a backup file is changed and it is opened
- **THEN** opening fails with an error that the backup is damaged

#### Scenario: Truncated file
- **WHEN** a backup file is truncated to any shorter length and opened
- **THEN** opening fails without panicking

### Requirement: Versioned backup format
A backup file SHALL begin with a fixed magic value and a format version. A file without the magic SHALL be reported as not a backup. A file with an unknown version SHALL be reported as needing a newer app, distinct from damage.

#### Scenario: Not a backup
- **WHEN** a markdown file is chosen as a backup
- **THEN** the app says it is not a Notebook backup

#### Scenario: Newer format
- **WHEN** a backup whose version is higher than this app knows is opened
- **THEN** the app says a newer version of Notebook is needed

### Requirement: Backup folder
Backups SHALL be written only to a folder the user chose through the system folder picker, and only while that folder exists. If the chosen folder is missing or not writable, the backup SHALL fail with an error naming the problem, and the app SHALL NOT create the folder or write anywhere else.

#### Scenario: Folder chosen
- **WHEN** the user chooses `~/Dropbox/Notebook` as the backup folder and a backup runs
- **THEN** the backup file appears in that folder

#### Scenario: Folder unavailable
- **WHEN** the chosen folder has been removed or its drive is not mounted and a backup runs
- **THEN** the backup fails with a folder-unavailable error, and no folder is created

### Requirement: Backup file naming
Each backup SHALL be named `notebook-backup-YYYYMMDD-HHMMSS.nbbackup` from its creation time in UTC. If that name is taken, a ` 2`, ` 3`, and so on suffix SHALL be added before the extension, and an existing file SHALL never be overwritten.

#### Scenario: Name from time
- **WHEN** a backup is created at 2026-10-22T14:03:09Z
- **THEN** it is named `notebook-backup-20261022-140309.nbbackup`

### Requirement: Atomic backup writes
A backup SHALL be written under a temporary hidden name in the backup folder, flushed to disk, and then renamed to its final name. A failed or interrupted backup SHALL leave no file under a backup name. Its temporary file SHALL be removed on failure, or at the next backup if the process stopped.

#### Scenario: Failure mid-write
- **WHEN** writing a backup fails partway through
- **THEN** no `.nbbackup` file for it exists and the temporary file is gone

### Requirement: Automatic backups
When a backup folder is set and automatic backups are on, the app SHALL write a backup while running whenever notes have changed since the last successful backup and the chosen interval (hourly, daily, or weekly, default daily) has passed. The app SHALL NOT write a backup when nothing has changed.

#### Scenario: Due backup runs
- **WHEN** the interval is daily, notes were edited, and the last backup is more than a day old
- **THEN** a backup is written within an hour of the app running

#### Scenario: Nothing changed
- **WHEN** a day passes with no note changes since the last backup
- **THEN** no new backup is written

#### Scenario: First backup
- **WHEN** the user sets a backup folder for the first time
- **THEN** a backup is written right away

### Requirement: Back up now
The user SHALL be able to start a backup immediately whenever a backup folder is set, whether or not anything has changed. The app SHALL report its success or failure.

#### Scenario: Manual backup
- **WHEN** the user chooses Back up now
- **THEN** a new backup file is written and the app reports the time of the backup

### Requirement: Retention
After each successful backup, the app SHALL delete the oldest backups of this vault in the backup folder beyond the keep count (default 14, at least 1). It SHALL consider only files that are named like backups and whose header names this vault. It SHALL never delete any other file.

#### Scenario: Old backups pruned
- **WHEN** the keep count is 3 and a fourth backup of this vault is written
- **THEN** the oldest of the four is deleted and three remain

#### Scenario: Other files untouched
- **WHEN** the folder also holds another vault's backups, a renamed backup, and unrelated files
- **THEN** none of them are deleted

### Requirement: Backup status
The app SHALL keep the time of the last successful backup and the last backup error on the device. It SHALL report backups as failing when the most recent attempt failed, and as overdue when a backup is due and none has succeeded for twice the interval.

#### Scenario: Failure reported
- **WHEN** an automatic backup fails because the folder is unavailable
- **THEN** the backup status reports failing with the folder-unavailable error, and the last successful time is unchanged

### Requirement: Restore is all-or-nothing
Restoring SHALL open the backup with the recovery key, or with this device's vault key when the backup belongs to this device's vault. It SHALL check that every note's key unwraps and every snapshot opens and parses before changing anything. On any failure the device's existing notes SHALL be untouched.

#### Scenario: Wrong recovery key
- **WHEN** a backup is restored with a valid-looking recovery key from another notebook
- **THEN** restore fails saying the key does not open this backup, and nothing on the device changes

#### Scenario: Damaged backup
- **WHEN** a damaged backup is restored
- **THEN** restore fails saying the backup is damaged, and nothing on the device changes

#### Scenario: Same-vault restore needs no key
- **WHEN** the user restores a backup of the notebook already open on this device
- **THEN** no recovery key is asked for

### Requirement: Restored notebook replaces the device's notebook
A successful restore SHALL make the backup's vault the device's notebook, with its vault ID, recovery entry, and notes, and SHALL store its vault key as the device's vault key. The notes present before the restore SHALL be replaced, not merged.

#### Scenario: Restore over existing notes
- **WHEN** a device with notes A and B restores a backup holding notes B' and C
- **THEN** afterwards the device holds exactly B' and C, and later launches open them with no prompt

### Requirement: Backup logs carry no secrets
Logs about backups SHALL NOT contain note content, keys, or the recovery key. They MAY contain file names, sizes, and error kinds.

#### Scenario: Restore with a wrong key
- **WHEN** a restore fails because of a wrong recovery key
- **THEN** no log line contains the entered key
