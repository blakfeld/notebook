# Spec Delta

## ADDED Requirements

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

### Requirement: Delete a stored note
Deleting a note SHALL remove its wrapped key, base snapshot, and log entries in one atomic step. Deleting a note the store does not hold SHALL have no effect.

#### Scenario: Deleted note disappears
- **WHEN** note N is deleted
- **THEN** N is not listed and loading N reports it absent

## MODIFIED Requirements

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

### Requirement: Same behavior in memory and on disk
The in-memory store SHALL behave the same as the native store for every operation above, except that its contents last only as long as the process.

#### Scenario: Shared behavior
- **WHEN** the same sequence of operations is run against an in-memory store and a native store
- **THEN** every operation returns the same results from both

## REMOVED Requirements

### Requirement: Outbox durability
**Reason**: There is no outbox without sync.
**Migration**: Replaced by "Local updates are durable".

### Requirement: Outbox order
**Reason**: There is no outbox without sync.
**Migration**: None.

### Requirement: Update identifiers
**Reason**: Identifiers existed to deduplicate pushes and server echoes.
**Migration**: None. Existing identifiers are dropped by the schema upgrade.

### Requirement: Acknowledge outbox entries
**Reason**: There is no server to acknowledge updates.
**Migration**: None.

### Requirement: Sync cursor
**Reason**: There is no server log to follow.
**Migration**: None. The schema upgrade drops the cursor.

### Requirement: Apply remote batches atomically
**Reason**: There are no remote batches. Restore uses "Import a note from a snapshot" (`vault-backup`).
**Migration**: None.

### Requirement: Remote echoes of local updates
**Reason**: There are no remote updates.
**Migration**: None.

### Requirement: Remote wrapped keys do not replace local ones
**Reason**: There are no remote batches.
**Migration**: None.

### Requirement: Pending note keys
**Reason**: Keys were pending upload to the server.
**Migration**: None.

### Requirement: Create a note locally
**Reason**: Restated without the outbox as "Create a note".
**Migration**: None.

### Requirement: Record local updates
**Reason**: Restated without the outbox as "Append local updates".
**Migration**: None.

### Requirement: Compaction
**Reason**: Restated without the outbox as "Compact a note".
**Migration**: None.

### Requirement: Delete a note
**Reason**: Restated without the outbox as "Delete a stored note".
**Migration**: None.

### Requirement: Clear the vault
**Reason**: Nothing clears a store any more: sign-out is gone, and restore swaps database files (`vault-backup`).
**Migration**: None.
