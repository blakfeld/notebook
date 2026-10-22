# note-documents Specification

## Purpose

Defines how each note is held as a CRDT document that every client edits and merges locally, how its encrypted updates and snapshots are produced and applied, and the current note state those documents yield.

## Requirements

### Requirement: Document per note
Each note SHALL be represented by its own CRDT document, bound to one vault ID and one note ID for its whole life. The document SHALL hold the note's markdown body as shared text and its created time, updated time, and pinned, archived, and trashed flags as shared metadata.

#### Scenario: New document matches the note it was created from
- **WHEN** a document is created for note N in vault V with body `# Hello\nworld` at time T
- **THEN** its current state has ID N, body `# Hello\nworld`, created and updated times T, and pinned, archived, and trashed all false

#### Scenario: Creation produces an initial update
- **WHEN** a document is created
- **THEN** creation also yields one encrypted update that, applied to an empty document for the same note, reproduces the same state

### Requirement: Local edits produce encrypted updates
Every local change to the body or metadata SHALL yield an encrypted update that carries that change. A change that leaves the state as it was SHALL yield no update.

#### Scenario: Body edit yields an update
- **WHEN** a document's body is changed from `Draft` to `Final draft`
- **THEN** an encrypted update is returned, and applying it to another copy of the document makes that copy's body `Final draft`

#### Scenario: Flag change yields an update
- **WHEN** a document is pinned
- **THEN** an encrypted update is returned that makes another copy of the document report pinned

#### Scenario: No-op change yields nothing
- **WHEN** a document's body is set to its current text, or a flag is set to its current value
- **THEN** no update is returned

### Requirement: Timestamps follow the note model
Documents SHALL keep the `notes` timestamp rules: a body change at time T sets the updated time to T, flag changes and no-op edits leave it alone, and the created time never changes. Times SHALL be supplied by the caller, not read from the system clock.

#### Scenario: Body edit bumps updated time
- **WHEN** a document created at T1 has its body changed at T2
- **THEN** its updated time is T2 and its created time is still T1

#### Scenario: Flag change leaves updated time alone
- **WHEN** a document created at T1 is archived at T2
- **THEN** its updated time is still T1

### Requirement: Body edits merge at character level
Concurrent edits to different parts of a note's body SHALL both be kept once the documents exchange updates. Setting the body to new text SHALL change only the spans of text that differ, even when there are several, so it does not overwrite concurrent edits elsewhere in the body, including between the changed spans.

#### Scenario: Concurrent edits to different lines
- **WHEN** two copies of a document with body `one\ntwo\nthree` change `one` to `ONE` and `three` to `THREE` independently, then exchange updates
- **THEN** both copies have body `ONE\ntwo\nTHREE`

#### Scenario: Setting the body keeps concurrent edits between changed spans
- **WHEN** one copy of a document with body `#a\nmiddle\n#a` sets its body to `#b\nmiddle\n#b` while another copy independently changes `middle` to `center`, then they exchange updates
- **THEN** both copies have body `#b\ncenter\n#b`

#### Scenario: Concurrent inserts at the same place
- **WHEN** two copies insert different text at the same position independently and then exchange updates
- **THEN** both copies contain both insertions, in the same order

### Requirement: Convergence
Copies of a document that have applied the same set of updates SHALL have identical state, regardless of the order in which the updates were applied and of how many times each was applied.

#### Scenario: Any order
- **WHEN** the same set of updates from three copies is applied to two fresh documents in two different orders
- **THEN** both documents have identical body and metadata

#### Scenario: Duplicates are harmless
- **WHEN** an update that was already applied is applied again
- **THEN** the document's state is unchanged

#### Scenario: Replicas that drew the same identity
- **WHEN** a document receives an update made by another replica under its own internal identity
- **THEN** it switches to a fresh identity, keeps its state, and later exchanges still converge

#### Scenario: Concurrent flag changes
- **WHEN** one copy pins a note and another unpins it concurrently, then they exchange updates
- **THEN** both copies report the same pinned value

### Requirement: Updates may arrive before their dependencies
An update that depends on changes the document has not yet seen SHALL be accepted and held. Its effect SHALL appear once the missing updates arrive, without the caller re-applying anything.

#### Scenario: Out-of-order delivery
- **WHEN** updates U1 then U2 are made on one copy, and another copy receives U2 before U1
- **THEN** after both are applied, the second copy's state equals the first's

### Requirement: Snapshots
A document SHALL produce an encrypted snapshot of its full state, including any received updates it is still holding for missing dependencies, so a snapshot never loses data the document has seen. Loading a snapshot and then applying any later updates SHALL reproduce the same state as applying every update from the start, including updates already covered by the snapshot.

#### Scenario: Snapshot alone
- **WHEN** a snapshot is taken of a document and loaded into a fresh document for the same note
- **THEN** the fresh document's state equals the original's

#### Scenario: Snapshot plus later updates
- **WHEN** a snapshot is taken, further edits produce updates U1 and U2, and a fresh document loads the snapshot and applies U1 and U2
- **THEN** its state equals the original's

#### Scenario: Overlapping updates
- **WHEN** a fresh document loads a snapshot and then also applies updates that were made before the snapshot was taken
- **THEN** its state equals the snapshot's

#### Scenario: Snapshot keeps held updates
- **WHEN** a document holding update U2 whose dependency U1 is missing takes a snapshot, a fresh document loads that snapshot, and U1 is then applied to it
- **THEN** the fresh document's state includes the effects of both U1 and U2

### Requirement: Updates and snapshots are encrypted to their note
Every update and snapshot a document produces for storage or sync SHALL be encrypted with the note's key and bound to its vault, note, and kind (update or snapshot) using the `vault-crypto` envelope. A document SHALL never hand plaintext updates or snapshots to storage or sync. Plaintext SHALL cross only the editor exchange that `note-editor-component` defines.

#### Scenario: Ciphertext hides the body
- **WHEN** a document whose body contains a distinctive marker string emits an update and a snapshot
- **THEN** the marker does not appear in either

#### Scenario: Update from another note is rejected
- **WHEN** an update emitted by note A's document is applied to note B's document
- **THEN** applying fails and B's state is unchanged

#### Scenario: Update presented as a snapshot is rejected
- **WHEN** an encrypted update is loaded as a snapshot
- **THEN** loading fails

### Requirement: Bad input leaves the document unchanged
Applying an update or loading a snapshot that fails decryption, has an unknown document format, does not decode, exceeds the size limit, or holds content a note document cannot contain SHALL return an error and leave the document's state exactly as it was. Bad input SHALL NOT cause a panic or an allocation out of proportion to its size.

#### Scenario: Tampered update
- **WHEN** an update with one byte changed is applied
- **THEN** applying fails with a decryption error and the state is unchanged

#### Scenario: Garbage that decrypts
- **WHEN** a correctly encrypted update whose plaintext is not a valid document update is applied
- **THEN** applying fails with a malformed-update error and the state is unchanged

#### Scenario: Unknown document format
- **WHEN** a correctly encrypted update declares a document format this client does not know
- **THEN** applying fails with an unsupported-format error, distinct from a malformed update

#### Scenario: Oversized update
- **WHEN** a correctly encrypted update whose plaintext is larger than the document size limit is applied
- **THEN** applying fails with a malformed-update error and the state is unchanged

#### Scenario: Content outside the document schema
- **WHEN** a correctly encrypted update adds a root other than the body and metadata, an item whose parent is another item, or content other than text and plain values
- **THEN** applying fails with a malformed-update error and the state is unchanged, even if part of the update would have been valid on its own

### Requirement: Versioned document format
The plaintext of every update and snapshot SHALL begin with a document format identifier, so a later client can change the encoding while still reading data written in the current format. The encoding of the current format SHALL be fixed and covered by a test vector.

#### Scenario: Fixed output for fixed input
- **WHEN** a document is created with a fixed internal identity, body, and time
- **THEN** its initial update's plaintext matches the recorded test vector byte for byte

### Requirement: Current note state
A document SHALL expose its current state as a note value from the `notes` capability (ID, body, created and updated times, flags, derived title), so tag parsing, search, and export use the same note model. A document that has not yet received its creation update SHALL report that it has no current state rather than invent one. Metadata values of the wrong type SHALL be read as their defaults instead of failing.

#### Scenario: Title derived from the merged body
- **WHEN** a document's merged body is `## Plans ##\n- ship`
- **THEN** its current note's title is `Plans`

#### Scenario: Unreadable metadata falls back to defaults
- **WHEN** a correctly encrypted update sets a flag to a value that is not a boolean, or the updated time to a value that is not a timestamp
- **THEN** the current note reports that flag as false, or its updated time as its created time, and does not fail

#### Scenario: Document missing its creation update
- **WHEN** a fresh document for a note has received only a later update whose dependencies are missing
- **THEN** it reports no current state

### Requirement: Editor exchange
A note document SHALL let the open editor obtain its full current state and later changes as unencrypted updates, and SHALL accept unencrypted updates from that editor. This exchange is for the in-device editor only. Its updates SHALL NOT be handed to storage or sync, and SHALL be distinguishable by type from encrypted updates and snapshots.

#### Scenario: Editor starts from the full state
- **WHEN** an editor replica is started from a document's editor state
- **THEN** the replica's body equals the document's body

#### Scenario: Remote change reaches the editor
- **WHEN** an encrypted update from another device is applied to a document that has an editor attached, and the editor's pending changes are then taken
- **THEN** applying those changes to the editor replica makes its body equal the document's body

#### Scenario: Nothing new
- **WHEN** the editor's pending changes are taken twice with no document change in between
- **THEN** the second take returns nothing

### Requirement: Editor updates become encrypted updates
Applying an editor update to a document SHALL change the document as any local edit would and SHALL yield one encrypted update carrying that change. An editor update that changes the body at time T SHALL set the updated time to T; one that changes nothing SHALL yield no update.

#### Scenario: Typed text is encrypted for sync
- **WHEN** an editor update that appends `!` to body `Hi` is applied at time T
- **THEN** the document body is `Hi!`, its updated time is T, and the returned encrypted update, applied to another copy of the document, makes that copy's body `Hi!`

#### Scenario: Duplicate editor update
- **WHEN** the same editor update is applied twice
- **THEN** the second application yields no encrypted update and the state is unchanged

#### Scenario: Updated time reaches the editor
- **WHEN** an editor update changes the body and the editor's pending changes are then taken
- **THEN** the taken changes include the new updated time, so the editor replica and the document converge

### Requirement: Bad editor updates are rejected
An editor update that does not decode SHALL be rejected with a malformed-update error, leave the document unchanged, and yield no encrypted update. It SHALL NOT cause a panic.

#### Scenario: Garbage editor update
- **WHEN** random bytes are applied as an editor update
- **THEN** applying fails with a malformed-update error and the document's state is unchanged
