# Spec Delta

## ADDED Requirements

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
