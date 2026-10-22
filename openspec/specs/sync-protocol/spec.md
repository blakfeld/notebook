# sync-protocol Specification

## Purpose
Defines the contract between Notebook clients and the server for relaying end-to-end encrypted note changes: how updates are pushed, ordered, stored, compacted, pulled, streamed live, and purged, without the server ever reading note content.

## Requirements

### Requirement: Authenticated, vault-scoped access
Every sync request and live connection SHALL require a valid access token from the `accounts` capability and SHALL act only on the vault of that token's account. No sync request SHALL name a vault, so one account can neither read nor change another vault's data. A missing, expired, or revoked token SHALL get `401` before any sync data is read or written.

#### Scenario: No token
- **WHEN** a client calls any sync endpoint without an access token
- **THEN** the server responds `401` and nothing is stored or returned

#### Scenario: Expired token
- **WHEN** a client pushes with an access token that has expired
- **THEN** the server responds `401` and the update is not stored

#### Scenario: Vaults are isolated
- **WHEN** users A and B each push an update for a note with the same note ID, and A pulls from cursor zero
- **THEN** A receives only A's update and none of B's data

### Requirement: Opaque payloads
The server SHALL store and return update, snapshot, and wrapped-key bytes exactly as received, and SHALL NOT parse, decrypt, merge, or create them. The only information the server uses about a payload is its note ID, its client-supplied identifiers, and its length.

#### Scenario: Bytes round-trip unchanged
- **WHEN** a client pushes an update and another client of the same vault pulls it
- **THEN** the pulled update bytes are identical to the pushed bytes

#### Scenario: Arbitrary bytes are accepted
- **WHEN** a client pushes an update whose bytes are not a valid ciphertext
- **THEN** the server stores and relays it like any other update

### Requirement: Sequenced append-only log
The server SHALL assign each accepted update a sequence number, unique within the vault and greater than every sequence number previously assigned in that vault, including to updates later compacted or purged. Sequence numbers SHALL never be reused, and SHALL NOT be assumed contiguous by clients.

#### Scenario: Increasing order
- **WHEN** a client pushes updates U1 and then U2 in separate requests
- **THEN** U2's sequence number is greater than U1's

#### Scenario: Concurrent pushes
- **WHEN** two clients of the same vault push at the same time
- **THEN** every accepted update receives a distinct sequence number

#### Scenario: No reuse after compaction
- **WHEN** a note's updates are compacted away and a new update is then pushed
- **THEN** the new update's sequence number is greater than any the compacted updates had

### Requirement: Batched push
A push, over HTTP or the live connection, SHALL carry one or more updates, each with a client-generated update ID, a note ID, and the update bytes, and SHALL be accepted or rejected as a whole. On success the response SHALL give each update's sequence number, in request order, and the vault's current head sequence.

#### Scenario: Batch accepted
- **WHEN** a client pushes three updates in one request and all are valid
- **THEN** all three are stored and the response lists a sequence number for each, in request order

#### Scenario: One bad update rejects the batch
- **WHEN** a client pushes a batch in which one update is over the size limit
- **THEN** the request is rejected and none of the batch's updates are stored

#### Scenario: Empty batch
- **WHEN** a client pushes a batch with no updates
- **THEN** the server responds `400`

### Requirement: Idempotent push
A pushed update whose update ID matches an update still in the vault's log SHALL NOT be stored again; the response SHALL return the originally assigned sequence number. If that update ID was stored with a different note ID or different bytes, the push SHALL be rejected with `409` and an update-ID conflict error.

#### Scenario: Retry after a lost response
- **WHEN** a client pushes an update, the response is lost, and the client pushes the same update with the same update ID again
- **THEN** the second response returns the same sequence number and the log holds the update once

#### Scenario: Reused ID with different content
- **WHEN** a client pushes an update ID that is already stored with different bytes
- **THEN** the server responds `409` with an update-ID conflict error and stores nothing from that batch

### Requirement: Note key registry
A push containing the first update for a note SHALL include that note's wrapped note key, which the server stores once per note. A first update without one SHALL be rejected with `422` and a missing-note-key error. A later wrapped key for a registered note SHALL be ignored if identical and rejected with `409` if different.

#### Scenario: New note registers its key
- **WHEN** a client pushes the first update for note N together with N's wrapped key
- **THEN** the push is accepted and later pulls include N's wrapped key

#### Scenario: Missing key for a new note
- **WHEN** a client pushes an update for a note the vault has never registered, without a wrapped key
- **THEN** the server responds `422` and nothing from the batch is stored

#### Scenario: Different key for an existing note
- **WHEN** a client pushes an update for registered note N with a wrapped key different from the stored one
- **THEN** the server responds `409` and the stored key is unchanged

### Requirement: Pull after a cursor
A client SHALL be able to pull every change after a cursor (a sequence number it has already processed). The response SHALL contain log entries with higher sequence numbers in ascending order (updates, each with its sequence number, note ID, client update ID, and bytes, and purge tombstones), any snapshots that replace updates it would otherwise miss, wrapped keys for notes registered after the cursor, and a new cursor.

#### Scenario: Incremental pull
- **WHEN** a client at cursor C pulls and another client has since pushed updates U1 and U2
- **THEN** the response contains U1 and U2 in sequence order and a new cursor at or above U2's sequence number

#### Scenario: Entries identify their push
- **WHEN** a client pushes an update with client update ID X and later pulls past it
- **THEN** the pulled entry carries update ID X and the sequence number the push was acknowledged with

#### Scenario: Nothing new
- **WHEN** a client pulls with a cursor equal to the vault's head
- **THEN** the response contains no updates, snapshots, keys, or tombstones, and the cursor is unchanged

#### Scenario: Note created on another device
- **WHEN** another client registers note N after this client's cursor and this client pulls
- **THEN** the response includes N's wrapped key along with N's updates

### Requirement: Pull across compaction
When a note's updates after a client's cursor have been compacted into a snapshot, a pull SHALL return that note's snapshot instead, so that applying the snapshot and the returned updates reproduces every update the client missed. No pull SHALL leave a client with a gap in any note's history.

#### Scenario: Client behind a compaction
- **WHEN** a client at cursor C pulls after another client uploaded a snapshot of note N covering sequence S greater than C
- **THEN** the response includes that snapshot and N's updates after S, and none of N's updates at or below S

#### Scenario: Client already past the snapshot
- **WHEN** a client at cursor C pulls and note N's latest snapshot covers a sequence at or below C
- **THEN** the response does not include N's snapshot

### Requirement: Bootstrap
A pull from cursor zero SHALL return the vault's full current state: every registered, unpurged note's wrapped key and latest snapshot, plus every update not covered by a snapshot. A fresh client SHALL need nothing else to reconstruct every note.

#### Scenario: Fresh client
- **WHEN** a vault has note A with a snapshot and later updates, and note B with only updates, and a new client pulls from cursor zero until no more pages remain
- **THEN** it receives both wrapped keys, A's snapshot and later updates, and all of B's updates

#### Scenario: Purged notes are absent
- **WHEN** a note has been permanently purged and a new client pulls from cursor zero
- **THEN** none of that note's updates, snapshot, or wrapped key are returned

### Requirement: Paginated pull
The server SHALL bound the size of a pull response. When more changes remain, the response SHALL say so and give a cursor from which the next pull continues without skipping or splitting any change. A single update or snapshot larger than the bound SHALL still be returned, alone.

#### Scenario: Large vault in pages
- **WHEN** a client bootstraps a vault whose changes exceed one page
- **THEN** each response except the last reports that more remain, and following the returned cursors yields every change exactly once

### Requirement: Cursor ahead of the server
A pull with a cursor greater than the vault's head SHALL be rejected with `409` and a cursor-ahead error, so a client whose server was restored from an older backup learns it must bootstrap again. A negative cursor SHALL get `400`.

#### Scenario: Server restored from backup
- **WHEN** a client pulls with a cursor greater than the vault's current head
- **THEN** the server responds `409` with a cursor-ahead error

### Requirement: Snapshot upload and compaction
A client SHALL be able to upload a snapshot of a registered note declaring the sequence S it covers. The server SHALL store it as the note's only snapshot and delete that note's updates at or below S, in one transaction, only if S is greater than the current snapshot's and not greater than the head.

#### Scenario: Snapshot compacts the log
- **WHEN** note N has updates at sequences 3, 5, and 9 and a client uploads a snapshot of N covering 5
- **THEN** the snapshot is stored, N's updates 3 and 5 are deleted, and update 9 remains

#### Scenario: Older snapshot is refused
- **WHEN** a client uploads a snapshot of N covering a sequence at or below that of N's stored snapshot
- **THEN** the server responds `409` with a stale-snapshot error and keeps the stored snapshot

#### Scenario: Covers beyond the head
- **WHEN** a client uploads a snapshot covering a sequence greater than the vault's head
- **THEN** the server responds `400` and nothing changes

#### Scenario: Unknown note
- **WHEN** a client uploads a snapshot for a note the vault has not registered
- **THEN** the server responds `404`

#### Scenario: Other notes untouched
- **WHEN** a snapshot of note N is uploaded
- **THEN** no other note's updates or snapshot change

### Requirement: Server never compacts on its own
The server SHALL delete updates only when a client's snapshot covers them or the note is purged, and SHALL NOT create, alter, or merge snapshots.

#### Scenario: Long log without snapshots
- **WHEN** a note accumulates many updates and no client uploads a snapshot
- **THEN** every update remains available to pull

### Requirement: Live updates
A client SHALL be able to open a live connection, authenticate it, and from the server's acknowledgement on receive every update appended and every note purged in its vault, in sequence order, until the connection closes. Each delivered entry SHALL be shaped like a pull's log entry, and an update that registered a note SHALL also carry its wrapped key.

#### Scenario: Update pushed elsewhere
- **WHEN** client A has an acknowledged live connection and client B of the same vault pushes an update
- **THEN** A receives that update over the live connection with its sequence number

#### Scenario: Other vaults are not delivered
- **WHEN** client A has a live connection and a user of another vault pushes an update
- **THEN** A receives nothing

#### Scenario: No gap between pull and live
- **WHEN** a client opens a live connection, receives the acknowledgement carrying the head H, and then pulls from its cursor
- **THEN** every change above its cursor is delivered either by the pull or on the connection

### Requirement: Live connection authentication
A live connection SHALL authenticate with an access token sent as the first message, not in the URL. A connection that sends no valid authentication within 10 seconds, or authenticates with an invalid token, SHALL be closed with an authentication-failure close code. A connection from a browser origin other than the server's own SHALL be refused.

#### Scenario: Token in first message
- **WHEN** a client opens a live connection and sends a valid access token as its first message
- **THEN** the server acknowledges the subscription with the vault's current head

#### Scenario: Silent connection
- **WHEN** a client opens a live connection and sends nothing for 10 seconds
- **THEN** the server closes it with the authentication-failure close code

#### Scenario: Foreign origin
- **WHEN** a live connection request carries an `Origin` header different from the server's public origin
- **THEN** the server refuses the upgrade with `403`

### Requirement: Live connection lifetime
A client SHALL be able to send a fresh access token on an open live connection to extend it. The server SHALL close a live connection when its current access token expires, when its session is revoked, when the client stops answering keepalive pings, or when the client falls too far behind to receive every change, using distinct close codes for re-authentication and for resynchronizing by pull. Each vault SHALL have a bounded number of live connections.

#### Scenario: Token expires
- **WHEN** a live connection's access token reaches its expiry time
- **THEN** the server closes the connection with the re-authentication close code

#### Scenario: Token refreshed in time
- **WHEN** a client sends a new valid access token for the same vault on its live connection before the old one expires
- **THEN** the connection stays open past the old token's expiry

#### Scenario: Session revoked
- **WHEN** the session behind a live connection is revoked or logged out
- **THEN** the server closes the connection with the re-authentication close code within one keepalive interval

#### Scenario: Slow consumer
- **WHEN** a live connection cannot keep up and changes would be dropped
- **THEN** the server closes it with the resynchronize close code rather than skip changes

#### Scenario: Too many connections
- **WHEN** a vault already has the maximum number of live connections and another is opened
- **THEN** the new connection is refused and existing connections are unaffected

### Requirement: Permanent purge
A client SHALL be able to permanently purge a registered note. The server SHALL delete its updates, snapshot, and wrapped key, assign the purge a sequence number, and keep a tombstone log entry at that sequence so later pulls and live connections report the note as purged.

#### Scenario: Purge removes data
- **WHEN** a client purges note N
- **THEN** N's updates, snapshot, and wrapped key are deleted and a later bootstrap returns none of them

#### Scenario: Other clients learn of the purge
- **WHEN** a client at cursor C pulls after note N was purged at a sequence greater than C
- **THEN** the response includes a purge entry for N at the purge's sequence number

#### Scenario: Purge is idempotent
- **WHEN** a client purges a note that is already purged
- **THEN** the server responds with success and changes nothing

### Requirement: Purged notes stay purged
Pushes or snapshot uploads for a purged note SHALL be rejected with `410` and a note-purged error, so an offline device cannot bring a purged note back.

#### Scenario: Offline device pushes a purged note
- **WHEN** a device that was offline during a purge of note N later pushes an update for N
- **THEN** the server responds `410` with a note-purged error and stores nothing from that batch

### Requirement: Size limits
The server SHALL reject with `413` any pushed update larger than the configured maximum update size (default 256 KiB) and any snapshot larger than the server's request body limit. These limits SHALL apply to the encrypted bytes as received.

#### Scenario: Oversize update
- **WHEN** a client pushes an update larger than the maximum update size
- **THEN** the server responds `413` and stores nothing from that batch

#### Scenario: Update at the limit
- **WHEN** a client pushes an update exactly the maximum update size
- **THEN** it is accepted

### Requirement: Push over the live connection
An authenticated live connection SHALL accept push messages, each tagged with a client-chosen request ID, with the same validation, atomicity, and idempotency as an HTTP push. The server SHALL answer each with a result carrying that request ID and either the assigned sequence numbers or an error code. A rejected push SHALL NOT close the connection.

#### Scenario: Push acknowledged on the socket
- **WHEN** a client sends a valid push over its live connection
- **THEN** it receives a result with the same request ID and a sequence number for each update, and other live connections of the vault receive the updates

#### Scenario: Rejected push keeps the socket open
- **WHEN** a client sends a push over its live connection that is rejected, for example for an oversize update
- **THEN** it receives a result with the same request ID and the error code, nothing from that push is stored or relayed, and the connection stays open

#### Scenario: Push before authentication
- **WHEN** a client sends a push on a live connection before authenticating
- **THEN** the server closes the connection with the authentication-failure close code and stores nothing

### Requirement: Single-transaction writes
Each push, snapshot upload, and purge SHALL take effect atomically: either all of its stored and deleted data changes and its sequence assignment become visible together, or none do, and nothing is relayed for a write that does not commit.

#### Scenario: Failed write leaves no trace
- **WHEN** a push fails partway through validating its updates
- **THEN** no update from it is stored, no sequence number is consumed, and no live connection receives anything

### Requirement: Machine-readable errors
Every sync error response, and every rejected push result on the live connection, SHALL carry a JSON body naming a stable error code, so clients can tell conditions with the same status apart. Error bodies SHALL NOT echo payload bytes or internal details.

#### Scenario: Distinguishable conflicts
- **WHEN** one push fails for an update-ID conflict and another for a note-key conflict
- **THEN** both get `409` with different error codes

### Requirement: Sync logging hygiene
The server SHALL NOT log update, snapshot, or wrapped-key bytes, or access tokens sent over the live connection.

#### Scenario: Live authentication is not logged
- **WHEN** a client authenticates a live connection with a token and the server logs at trace level
- **THEN** the log does not contain the token
