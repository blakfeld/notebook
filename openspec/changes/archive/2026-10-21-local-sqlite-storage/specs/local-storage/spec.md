# Spec Delta

## Purpose

Defines how a client keeps its local copy of a vault: encrypted note documents, wrapped note keys, the outbox of unacknowledged local updates, the sync cursor, and device-local settings, with durable native storage and an equivalent in-memory store.

## ADDED Requirements

### Requirement: Store bound to one vault
A store SHALL hold note data for exactly one vault, identified when it is opened. A native store with no vault data SHALL accept any vault. Opening a native store whose data belongs to a different vault SHALL fail with an error that names the mismatch, and SHALL leave the stored data unchanged.

#### Scenario: Reopen for the same vault
- **WHEN** a native store is opened for vault V, written to, closed, and opened again for vault V
- **THEN** opening succeeds and the earlier data is present

#### Scenario: Open for a different vault
- **WHEN** a native store holding data for vault V is opened for vault W
- **THEN** opening fails with a vault-mismatch error and the data for V is unchanged

#### Scenario: Open after clearing
- **WHEN** a native store holding data for vault V is cleared and then opened for vault W
- **THEN** opening succeeds and the store holds no notes

### Requirement: Only ciphertext is stored for notes
A store SHALL accept note content only as encrypted update and snapshot bytes and note keys only in wrapped form. It SHALL NOT accept, persist, or derive any plaintext note body, title, tag, or search index. Note IDs, the vault ID, sequence numbers, and byte sizes MAY be stored in the clear.

#### Scenario: Marker text never reaches disk
- **WHEN** a note whose body contains a distinctive marker string is created and edited, its encrypted updates and a snapshot are saved in a native store, and the store is closed
- **THEN** the marker string does not appear in any file the store wrote

### Requirement: Create a note locally
Creating a note SHALL store its wrapped note key and its initial encrypted update in one atomic step, and SHALL place that update in the outbox. Creating a note whose ID already exists in the store SHALL fail and leave the existing note unchanged.

#### Scenario: New note is loadable and pending
- **WHEN** note N is created with a wrapped key and an initial update
- **THEN** loading N returns that wrapped key and that update, and the outbox contains the update

#### Scenario: Duplicate note ID
- **WHEN** a note is created with an ID that already exists in the store
- **THEN** creation fails with a duplicate-note error and the existing note's key and log are unchanged

### Requirement: Record local updates
Appending a local update to an existing note SHALL add it to the end of that note's log and to the outbox in one atomic step. Appending to a note that is not in the store SHALL fail.

#### Scenario: Local edit is logged and queued
- **WHEN** local update U is appended to note N
- **THEN** loading N returns U as its last log entry, and the outbox contains U

#### Scenario: Unknown note
- **WHEN** a local update is appended to a note ID the store does not hold
- **THEN** appending fails with a note-not-found error

### Requirement: Load a note
Loading a note SHALL return its wrapped key, its base snapshot if one exists, and every log entry after it in the order the entries were stored, each marked as an update or a snapshot. Loading a note the store does not hold SHALL report that it is absent rather than fail.

#### Scenario: Entries come back in stored order
- **WHEN** remote update R1, local update L1, and remote snapshot S1 are stored for note N in that order
- **THEN** loading N returns entries R1, L1, S1 in that order, with S1 marked as a snapshot

#### Scenario: Absent note
- **WHEN** a note ID the store does not hold is loaded
- **THEN** the store reports the note as absent

### Requirement: Enumerate notes
A store SHALL list the IDs of every note it holds, so a client can load all documents at startup and rebuild its in-memory tag tree and search index from them.

#### Scenario: All notes listed
- **WHEN** notes A and B are created locally and note C arrives in a remote batch
- **THEN** listing notes returns exactly A, B, and C

### Requirement: Outbox durability
An update in the outbox SHALL remain there, across process crashes and restarts of a native store, until it is acknowledged. Once a native store reports that an update was appended, that update SHALL survive a crash or power loss.

#### Scenario: Pending update survives restart
- **WHEN** a local update is appended to a native store and the process exits without acknowledging it
- **THEN** after reopening the store the update is still in the outbox and in the note's log

### Requirement: Outbox order
The outbox SHALL return pending updates in the order they were appended, across all notes, each with its note ID and its update ID.

#### Scenario: Cross-note order
- **WHEN** local updates are appended to note A, then note B, then note A again
- **THEN** the outbox returns them in exactly that order

### Requirement: Update identifiers
Every stored update SHALL carry an identifier supplied by the caller and unique within the store. Storing a second update with an identifier already in the store SHALL NOT create a second copy of that update.

#### Scenario: Duplicate local identifier rejected
- **WHEN** a local update is appended with an identifier that is already stored
- **THEN** appending fails with a duplicate-update error and the note's log is unchanged

### Requirement: Acknowledge outbox entries
Acknowledging an update by its identifier SHALL remove it from the outbox and record the server sequence number assigned to it, while keeping it in the note's log. Acknowledging an identifier that is not pending SHALL have no effect.

#### Scenario: Acknowledged update leaves the outbox
- **WHEN** pending update U is acknowledged with server sequence 42
- **THEN** the outbox no longer contains U, and loading its note still returns U

#### Scenario: Repeated acknowledgment
- **WHEN** an update that was already acknowledged is acknowledged again
- **THEN** the call succeeds and nothing changes

### Requirement: Sync cursor
A store SHALL hold one sync cursor for its vault: the highest server sequence number whose updates have been stored. A new store's cursor SHALL be zero. The cursor SHALL never move backwards.

#### Scenario: Fresh store
- **WHEN** a store is opened for the first time
- **THEN** its cursor is zero

#### Scenario: Cursor cannot regress
- **WHEN** a remote batch with cursor 10 is applied and then a batch with cursor 7 is applied
- **THEN** the second batch is rejected and the cursor stays at 10

### Requirement: Apply remote batches atomically
A remote batch, holding wrapped keys for notes new to the store, encrypted updates and snapshots with their identifiers, and a new cursor, SHALL be applied in one atomic step. If any part fails, nothing in the batch SHALL be stored and the cursor SHALL be unchanged.

#### Scenario: Batch with a new note
- **WHEN** a batch carries the wrapped key and first update for note N and cursor 5
- **THEN** after applying it, N is listed, loading N returns that key and update, and the cursor is 5

#### Scenario: Crash mid-batch
- **WHEN** the process of a native store stops while a remote batch is being applied
- **THEN** after reopening, either the whole batch is stored and the cursor advanced, or none of it is stored and the cursor is unchanged

#### Scenario: Update for an unknown note
- **WHEN** a batch carries an update for a note that neither the store nor the batch has a wrapped key for
- **THEN** applying fails, nothing from the batch is stored, and the cursor is unchanged

### Requirement: Remote echoes of local updates
When a remote batch contains an update whose identifier is already stored, the store SHALL NOT store it again. If that update is still in the outbox, the store SHALL treat it as acknowledged with the sequence number from the batch.

#### Scenario: Own update comes back
- **WHEN** pending local update U is pushed and later arrives in a remote batch with sequence 9
- **THEN** the note's log holds U once, U is no longer in the outbox, and U's server sequence is 9

### Requirement: Remote wrapped keys do not replace local ones
When a remote batch carries a wrapped key for a note the store already holds, the store SHALL keep the key it has.

#### Scenario: Key repeated in a later batch
- **WHEN** note N is already stored and a batch carries a wrapped key for N again
- **THEN** the batch applies and loading N returns the original wrapped key

### Requirement: Pending note keys
A wrapped key for a note created locally SHALL be reported as pending upload until it is marked uploaded. Keys that arrived in a remote batch SHALL never be reported as pending.

#### Scenario: Local key pending until uploaded
- **WHEN** note N is created locally
- **THEN** N's wrapped key is reported as pending, and after it is marked uploaded it is not

#### Scenario: Remote key not pending
- **WHEN** note N arrives in a remote batch with its wrapped key
- **THEN** N's key is not reported as pending

### Requirement: Compaction
Replacing a note's base snapshot SHALL store the new snapshot and remove that note's log entries up to a given position, in one atomic step. Entries still in the outbox SHALL NOT be removed. Entries after the given position SHALL be kept.

#### Scenario: Acknowledged entries are folded in
- **WHEN** note N has acknowledged entries E1 and E2 and a snapshot covering them replaces N's base through E2
- **THEN** loading N returns the new snapshot and no entries

#### Scenario: Pending entries survive compaction
- **WHEN** note N's log holds acknowledged E1, pending E2, and acknowledged E3, and a snapshot replaces N's base through E3
- **THEN** loading N returns the new snapshot followed only by E2, and E2 is still in the outbox

#### Scenario: Entries stored after the snapshot was taken
- **WHEN** a snapshot is taken through entry E2, entry E3 is stored, and then the snapshot replaces N's base through E2
- **THEN** loading N returns the new snapshot followed by E3

### Requirement: Delete a note
Deleting a note SHALL remove its wrapped key, base snapshot, log entries, and outbox entries in one atomic step. Deleting a note the store does not hold SHALL have no effect.

#### Scenario: Purged note disappears
- **WHEN** note N with a pending outbox entry is deleted
- **THEN** N is not listed, loading N reports it absent, and the outbox holds no entries for N

### Requirement: Clear the vault
Clearing a store SHALL remove every note, log entry, outbox entry, and pending key, reset the cursor to zero, and release the vault binding, in one atomic step. Device-local settings SHALL be kept.

#### Scenario: Sign-out clears note data
- **WHEN** a store with notes, pending updates, a cursor of 12, and a vim-mode setting is cleared
- **THEN** reopened for the same vault, it lists no notes, the outbox is empty, the cursor is zero, and the vim-mode setting is unchanged

### Requirement: Device-local settings
A store SHALL hold device-local settings as named text values that are never synced. Reading a setting that was never written SHALL report it absent. Settings SHALL hold only non-sensitive values, since they are stored unencrypted.

#### Scenario: Setting round trip
- **WHEN** setting `editor.vim` is written as `true` and read back after the native store is reopened
- **THEN** the value is `true`

#### Scenario: Missing setting
- **WHEN** a setting that was never written is read
- **THEN** the store reports it absent

### Requirement: Schema versioning
A native store SHALL record the version of its on-disk layout and upgrade older layouts in place when opened, keeping all data. Opening a store whose layout is newer than the client understands SHALL fail with an error that names it as too new, without changing the file.

#### Scenario: Upgrade on open
- **WHEN** a native store written by an earlier layout version is opened by a client with a later version
- **THEN** opening succeeds, all notes, outbox entries, the cursor, and settings are preserved, and the layout version is updated

#### Scenario: Newer layout
- **WHEN** a native store whose layout version is higher than any the client knows is opened
- **THEN** opening fails with a layout-too-new error and the file is unchanged

### Requirement: Corruption is reported, not hidden
A native store that cannot be read as a valid store SHALL fail to open with an error, and SHALL NOT be silently reset or overwritten.

#### Scenario: Garbage file
- **WHEN** a native store is opened at a path holding a file that is not a valid store
- **THEN** opening fails with an error and the file is unchanged

### Requirement: Same behavior in memory and on disk
The in-memory store SHALL behave the same as the native store for every operation above, except that its contents last only as long as the process. The in-memory store SHALL be available in the browser build.

#### Scenario: Shared behavior
- **WHEN** the same sequence of operations is run against an in-memory store and a native store
- **THEN** every operation returns the same results from both
