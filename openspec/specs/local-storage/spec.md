# local-storage Specification

## Purpose
Defines how the app stores a vault on the device: encrypted note documents, wrapped note keys, and device-local settings, with durable native storage, local compaction, and an equivalent in-memory store.

## Requirements

### Requirement: Store bound to one vault
A store SHALL hold note data for exactly one vault, identified when it is opened. A native store with no vault data SHALL accept any vault. Opening a native store whose data belongs to a different vault SHALL fail with an error that names the mismatch, and SHALL leave the stored data unchanged.

#### Scenario: Reopen for the same vault
- **WHEN** a native store is opened for vault V, written to, closed, and opened again for vault V
- **THEN** opening succeeds and the earlier data is present

#### Scenario: Open for a different vault
- **WHEN** a native store holding data for vault V is opened for vault W
- **THEN** opening fails with a vault-mismatch error and the data for V is unchanged

#### Scenario: Open after clearing
- **WHEN** the database file of a native store holding data for vault V is deleted and a store is opened at the same path for vault W
- **THEN** opening succeeds and the store holds no notes

### Requirement: Only ciphertext is stored for notes
A store SHALL accept note content only as encrypted update and snapshot bytes and note keys only in wrapped form. It SHALL NOT accept, persist, or derive any plaintext note body, title, tag, or search index. Note IDs, the vault ID, sequence numbers, and byte sizes MAY be stored in the clear.

#### Scenario: Marker text never reaches disk
- **WHEN** a note whose body contains a distinctive marker string is created and edited, its encrypted updates and a snapshot are saved in a native store, and the store is closed
- **THEN** the marker string does not appear in any file the store wrote

### Requirement: Load a note
Loading a note SHALL return its wrapped key, its base snapshot if one exists, and every log entry after it in the order the entries were stored. Loading a note the store does not hold SHALL report that it is absent rather than fail.

#### Scenario: Entries come back in stored order
- **WHEN** updates U1, U2, and U3 are appended to note N in that order
- **THEN** loading N returns U1, U2, U3 in that order

#### Scenario: Absent note
- **WHEN** a note ID the store does not hold is loaded
- **THEN** the store reports the note as absent

### Requirement: Enumerate notes
A store SHALL list the IDs of every note it holds, so a client can load all documents at startup and rebuild its in-memory tag tree and search index from them.

#### Scenario: All notes listed
- **WHEN** notes A, B, and C are created and B is then deleted
- **THEN** listing notes returns exactly A and C

### Requirement: Device-local settings
A store SHALL hold device-local settings as named text values that are never synced. Reading a setting that was never written SHALL report it absent. Settings SHALL hold only non-sensitive values, since they are stored unencrypted.

#### Scenario: Setting round trip
- **WHEN** setting `editor.vim` is written as `true` and read back after the native store is reopened
- **THEN** the value is `true`

#### Scenario: Missing setting
- **WHEN** a setting that was never written is read
- **THEN** the store reports it absent

### Requirement: Schema versioning
A native store SHALL record the version of its on-disk layout and upgrade older layouts in place when opened, keeping all notes, their logs, and settings. Opening a store whose layout is newer than the client understands SHALL fail with an error that names it as too new, without changing the file.

#### Scenario: Upgrade on open
- **WHEN** a native store written by an earlier layout version is opened by a client with a later version
- **THEN** opening succeeds, all notes, log entries, and settings are preserved, and the layout version is updated

#### Scenario: Sync data dropped on upgrade
- **WHEN** a store written while the app synced with a server, holding outbox entries, server sequence numbers, and a sync cursor, is opened
- **THEN** every note and log entry is preserved, including entries that were never acknowledged, and the sync data is gone

#### Scenario: Newer layout
- **WHEN** a native store whose layout version is higher than any the client knows is opened
- **THEN** opening fails with a layout-too-new error and the file is unchanged

### Requirement: Corruption is reported, not hidden
A native store that cannot be read as a valid store SHALL fail to open with an error, and SHALL NOT be silently reset or overwritten.

#### Scenario: Garbage file
- **WHEN** a native store is opened at a path holding a file that is not a valid store
- **THEN** opening fails with an error and the file is unchanged

### Requirement: Local updates are durable
Once a native store reports that a note was created or an update was appended, that change SHALL survive a process crash or power loss.

#### Scenario: Update survives restart
- **WHEN** a local update is appended to a native store and the process exits immediately afterwards
- **THEN** after reopening the store the update is in the note's log

### Requirement: Create a note
Creating a note SHALL store its wrapped note key and its initial encrypted update in one atomic step. Creating a note whose ID already exists in the store SHALL fail and leave the existing note unchanged.

#### Scenario: New note is loadable
- **WHEN** note N is created with a wrapped key and an initial update
- **THEN** loading N returns that wrapped key and that update

#### Scenario: Duplicate note ID
- **WHEN** a note is created with an ID that already exists in the store
- **THEN** creation fails with a duplicate-note error and the existing note's key and log are unchanged

### Requirement: Append local updates
Appending a local update to an existing note SHALL add it to the end of that note's log in one atomic step. Appending to a note that is not in the store SHALL fail.

#### Scenario: Local edit is logged
- **WHEN** local update U is appended to note N
- **THEN** loading N returns U as its last log entry

#### Scenario: Unknown note
- **WHEN** a local update is appended to a note ID the store does not hold
- **THEN** appending fails with a note-not-found error

### Requirement: Compact a note
Replacing a note's base snapshot SHALL store the new snapshot and remove that note's log entries up to a given position, in one atomic step. Entries after the given position SHALL be kept.

#### Scenario: Entries are folded in
- **WHEN** note N has entries E1 and E2 and a snapshot covering them replaces N's base through E2
- **THEN** loading N returns the new snapshot and no entries

#### Scenario: Entries stored after the snapshot was taken
- **WHEN** a snapshot is taken through entry E2, entry E3 is stored, and then the snapshot replaces N's base through E2
- **THEN** loading N returns the new snapshot followed by E3

### Requirement: Long logs are compacted locally
Once a note's log holds more than 200 entries after its base, the app SHALL replace the base with a snapshot covering every stored entry for that note and remove those entries. A note holding data only a newer build can read SHALL never be compacted. A failed compaction SHALL NOT fail the edit that triggered it.

#### Scenario: Long log compacts and reloads the same
- **WHEN** a note is edited until its log passes 200 entries
- **THEN** it is stored as a base snapshot with no log entries, and reloading it gives the same note

#### Scenario: Entries written by another process are kept
- **WHEN** another process appends an update to a note and this process then edits the note past the threshold
- **THEN** after compaction the note still reflects the other process's update

#### Scenario: Newer data is never compacted away
- **WHEN** a note holds an entry in a format this build can't read and is edited past the threshold
- **THEN** its base and log are kept as they were, with the new edits appended

#### Scenario: Failed compaction
- **WHEN** compaction fails after an edit crosses the threshold
- **THEN** the edit is stored, and compaction is not tried again until 200 more entries are stored

### Requirement: Delete a stored note
Deleting a note SHALL remove its wrapped key, base snapshot, and log entries in one atomic step. Deleting a note the store does not hold SHALL have no effect.

#### Scenario: Deleted note disappears
- **WHEN** note N is deleted
- **THEN** N is not listed and loading N reports it absent

### Requirement: Same behavior in memory and on disk
The in-memory store SHALL behave the same as the native store for every operation above, except that its contents last only as long as the process.

#### Scenario: Shared behavior
- **WHEN** the same sequence of operations is run against an in-memory store and a native store
- **THEN** every operation returns the same results from both
