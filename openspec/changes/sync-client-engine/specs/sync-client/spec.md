# Spec Delta

## Purpose

Defines how every Notebook client keeps its local copy of a vault in step with the server: durable queuing and pushing of local edits, pulling and applying remote ones, bootstrap, live updates, snapshotting, purge propagation, and the sync status and change notifications the UI shows.

## ADDED Requirements

### Requirement: Local edits are durable before they are reported done
A local change to a note SHALL be applied to the note's document and written to local storage, as both a stored update and an outbox entry, in one atomic step before the edit operation returns. An edit that fails to store SHALL leave the document, storage, and outbox unchanged.

#### Scenario: Edit survives a crash before push
- **WHEN** a note is edited offline on a client with persistent storage and the client process is killed before any push
- **THEN** after restart the note shows the edit and the outbox still holds the update

#### Scenario: Storage failure rolls back the edit
- **WHEN** storage fails while a local edit is being written
- **THEN** the edit returns an error and the note's state and the outbox are as they were before the edit

### Requirement: New notes carry their wrapped key
Creating a note SHALL generate a fresh note key, wrap it with the vault key for that note, and store the wrapped key locally as not yet uploaded. The client SHALL send the wrapped key with the note's updates until a push carrying it is acknowledged, and only then record it as uploaded.

#### Scenario: Note created on one device opens on another
- **WHEN** device A creates a note and syncs, and device B of the same vault then syncs
- **THEN** device B shows the note with the same body

#### Scenario: Key upload interrupted
- **WHEN** a new note's first push fails, and the client restarts and pushes again
- **THEN** the retried push still carries the note's wrapped key, and the key is recorded as uploaded only after the server acknowledges it

### Requirement: Outbox entries leave only on acknowledgment
Outbox entries SHALL be pushed in the order they were queued and removed only after the server acknowledges them. Each entry SHALL keep the client update ID it was given when queued, and every retry SHALL reuse it.

#### Scenario: Push fails and is retried
- **WHEN** a push fails because the network drops
- **THEN** the entries stay in the outbox and are pushed again later with the same client update IDs

#### Scenario: Acknowledgment lost
- **WHEN** the server stores a pushed update but the client never receives the acknowledgment, and the client retries
- **THEN** the server holds the update once and the client removes the entry when the retry is acknowledged

### Requirement: Pull applies remote updates in order
The client SHALL fetch updates after its stored cursor, apply them to the note documents in server sequence order, and store them. The cursor SHALL advance only after the updates up to it are stored, so a crash mid-pull never skips an update.

#### Scenario: Remote edit arrives
- **WHEN** another device edits a note and this client pulls
- **THEN** this client's copy of the note shows the edit and its cursor is at the edit's sequence number

#### Scenario: Crash during pull
- **WHEN** the client crashes after receiving a page of updates but before storing them
- **THEN** after restart it pulls that page again from the old cursor and ends in the same state

### Requirement: Own updates are not reported as remote changes
When the client receives an update it pushed itself, it SHALL advance its cursor past it, let local storage treat it as acknowledged (per `local-storage`), and SHALL NOT report the note as remotely changed.

#### Scenario: Echo of own push
- **WHEN** the client pushes an update and later receives that update from the server
- **THEN** no remote-change notification is emitted for it

#### Scenario: Echo arrives before the acknowledgment
- **WHEN** the push response is lost but the pushed update arrives over the live connection
- **THEN** the update leaves the outbox without being pushed again and no remote-change notification is emitted

### Requirement: Bootstrap a fresh client
A client whose cursor is zero SHALL bootstrap by pulling from cursor zero until no more pages remain, storing every note's wrapped key, latest snapshot, and later updates. Each page SHALL be stored together with its cursor. Updates already in its outbox SHALL be kept and pushed afterwards.

#### Scenario: New device sees the whole vault
- **WHEN** a client signs in to a vault holding 3 notes with no local data
- **THEN** after bootstrap it holds all 3 notes with the same bodies as the server's other clients

#### Scenario: Interrupted bootstrap
- **WHEN** a bootstrap fails after some pages were stored
- **THEN** the next sync attempt continues from the cursor of the last stored page and the client ends with the full vault

### Requirement: Catch-up after server compaction
When a pull returns a note's snapshot because updates after the client's cursor were removed by compaction, the client SHALL merge the snapshot and the updates after it into its local state, and SHALL NOT lose its own unpushed edits.

#### Scenario: Long-offline device returns
- **WHEN** a device that was offline while another device snapshotted a note and the server compacted it reconnects, with an unpushed edit to that note
- **THEN** the device's copy contains both the other device's edits and its own, and its own edit is then pushed

### Requirement: Live updates
While online, the client SHALL hold a live connection to the server and apply updates it delivers as they arrive, without waiting for a poll. Before relying on the live connection, the client SHALL pull everything after its cursor.

#### Scenario: Edit appears without polling
- **WHEN** two connected clients are open and one edits a note
- **THEN** the other applies the edit within a few seconds and without its polling interval elapsing

### Requirement: Fallback polling and reconnection with backoff
When the live connection cannot be established or drops, the client SHALL pull on a fixed interval and retry the live connection with exponential backoff and jitter, capped at a maximum delay and reset after a successful connection. A server rate-limit response SHALL delay the next attempt by at least the server's requested time.

#### Scenario: Server unreachable
- **WHEN** the server is unreachable for several consecutive attempts
- **THEN** the delay between attempts grows each time up to the cap, and the client reports offline

#### Scenario: Rate limited
- **WHEN** the server answers a sync request with a rate-limit response asking the client to wait 30 seconds
- **THEN** the client makes no further sync request for at least 30 seconds

#### Scenario: Server asks to resynchronize
- **WHEN** the server closes the live connection asking the client to resynchronize
- **THEN** the client pulls from its cursor and then reopens the live connection

#### Scenario: Connection restored
- **WHEN** the server becomes reachable again
- **THEN** the client pulls missed updates, pushes its outbox, reopens the live connection, and resets its backoff

### Requirement: Local edits trigger a prompt push
While online, the client SHALL push a newly queued local update promptly, coalescing edits made in quick succession into one push, rather than waiting for a polling interval.

#### Scenario: Typing burst
- **WHEN** a user makes 20 edits within half a second while online
- **THEN** the updates are pushed within a few seconds, in fewer than 20 requests

### Requirement: Client-driven snapshots
When a note has more stored entries since its last snapshot than a configured threshold, the client SHALL, once caught up and with no outbox entries for that note, upload an encrypted snapshot declaring that it covers the note through the client's cursor. After the server accepts it, the client SHALL compact the note's local log with that snapshot.

#### Scenario: Threshold reached
- **WHEN** a note accumulates more updates than the threshold and the client is caught up
- **THEN** the client uploads one snapshot for the note and afterwards stores the snapshot instead of the log entries it covers

#### Scenario: Snapshot rejected
- **WHEN** the server rejects a snapshot upload, for example because another device already uploaded a newer one
- **THEN** the client keeps its stored log unchanged and does not retry that note's snapshot until more entries arrive

#### Scenario: Snapshot does not change content
- **WHEN** a fresh client bootstraps after another client uploaded a snapshot
- **THEN** its notes have the same bodies and flags as they had before the snapshot

### Requirement: Permanent purge
Purging SHALL apply only to trashed notes and SHALL require a connection to the server. After the server confirms, or reports it never received the note, the client SHALL delete the note, including its outbox entries, from the replica and storage. A purge that cannot reach the server SHALL fail and change nothing.

#### Scenario: Purge propagates
- **WHEN** device A purges a trashed note and device B syncs
- **THEN** the note is gone from both devices' replicas and storage

#### Scenario: Purging a live note
- **WHEN** a purge is requested for a note that is not trashed
- **THEN** the request fails and nothing changes

#### Scenario: Purge while offline
- **WHEN** a purge is requested while the server is unreachable
- **THEN** the request fails, the note stays trashed, and the user can retry later

#### Scenario: Never-synced note
- **WHEN** a trashed note that was never pushed is purged
- **THEN** it is removed locally and its pending updates are discarded

### Requirement: Purges from other devices
When a pull or live message reports a note as purged, or the server rejects a push because the note was purged, the client SHALL delete the note and its stored data, including outbox entries, and report it as changed. Other notes' outbox entries SHALL still be pushed.

#### Scenario: Offline edit to a note purged elsewhere
- **WHEN** a device edits note N offline while another device purges N, and the first device reconnects
- **THEN** N is removed from the first device, its edit is discarded, and its edits to other notes are pushed

### Requirement: Bad remote input is contained
A remote update or snapshot that fails decryption or decoding SHALL be skipped without changing the note, the cursor SHALL still advance past it, and the client SHALL report an integrity warning naming the note. One bad update SHALL NOT stop other updates or notes from syncing.

#### Scenario: Tampered update from the server
- **WHEN** the server delivers an update with a modified byte followed by a valid update for another note
- **THEN** the tampered update is not applied, the valid one is, and an integrity warning names the first note

### Requirement: Unknown document format waits for an upgrade
A remote update or snapshot in a document format this client does not support SHALL be stored but not applied, the client SHALL report that a client upgrade is required, and the stored data SHALL be applied when a client that supports the format starts.

#### Scenario: Newer client wrote a note
- **WHEN** an update in an unknown document format arrives for a note
- **THEN** the note keeps its previous state, the status reports upgrade required, and other notes keep syncing

### Requirement: Session expiry
When the server rejects a sync request as unauthenticated, the client SHALL refresh the session once and retry. If refresh fails, the client SHALL stop sync requests, keep its outbox, and report that sign-in is required until it is given a new session.

#### Scenario: Access token expired
- **WHEN** the access token expires and the refresh succeeds
- **THEN** sync continues without the user doing anything

#### Scenario: Live connection outlives a token
- **WHEN** the live connection stays open past the access token's lifetime and refresh succeeds
- **THEN** the client renews the connection's authentication before the token expires, without reconnecting

#### Scenario: Refresh token revoked
- **WHEN** the refresh fails because the device's session was revoked
- **THEN** the status reports sign-in required, pending changes remain queued, and they are pushed after the user signs in again

### Requirement: Push rejections are surfaced
When the server refuses a push because the vault's storage quota is full or an update is larger than the server accepts, the client SHALL keep the update queued, stop pushing until the condition changes, keep pulling, and report the reason in its status.

#### Scenario: Quota full
- **WHEN** a push is rejected because the vault's quota is full
- **THEN** the status reports quota full, the pending count includes the rejected updates, and remote updates still arrive

### Requirement: Resync when the server is behind
When the server reports that the client's cursor is ahead of its log, or that a note this client registered is unknown, as after a server restore from backup, the client SHALL bootstrap again from the server and re-send its own state of every note, so no note content held by the client is lost.

#### Scenario: Server restored from backup
- **WHEN** a pull is rejected because the client's cursor is ahead of the server, and the client holds a note created after the backup was taken
- **THEN** the client bootstraps from the server, the note is registered and stored on the server again, and another device that then syncs shows it

#### Scenario: Edits the restored server lost
- **WHEN** the restored server lacks the last edits this client pushed to a note before the restore
- **THEN** after resync the server's copy of the note includes those edits

### Requirement: Sync status
The client SHALL report a sync status of synced, syncing, or offline, together with the number of pending local updates, and any condition needing the user (sign-in required, upgrade required, quota full, update too large, integrity warning). It SHALL report each status change as it happens.

#### Scenario: Offline with pending edits
- **WHEN** the client is disconnected and 3 local updates are queued
- **THEN** the status is offline with 3 pending

#### Scenario: Back in sync
- **WHEN** the client reconnects, pushes its outbox, and catches up
- **THEN** the status becomes synced with 0 pending

### Requirement: Remote change notifications
After applying remote changes, the client SHALL report the IDs of notes whose state changed, including created and purged notes, batching changes applied together into one notification. Local edits SHALL NOT be reported as remote changes.

#### Scenario: Batch of remote edits
- **WHEN** one pull applies updates to notes A and B
- **THEN** one notification names A and B

#### Scenario: Local edit
- **WHEN** the user edits a note on this client
- **THEN** no remote-change notification is emitted for it

### Requirement: Flush on demand
The client SHALL offer a flush operation that pushes all queued updates immediately and completes when the outbox is empty or a push fails, reporting which.

#### Scenario: Flush before backgrounding
- **WHEN** the host flushes while online with 2 queued updates
- **THEN** the flush completes successfully and the outbox is empty

#### Scenario: Flush while offline
- **WHEN** the host flushes while the server is unreachable
- **THEN** the flush completes with a failure and the updates stay queued

### Requirement: Same behavior on every platform
Native and browser clients SHALL run the same sync logic, differing only in transport, timers, and storage. Clients on different platforms editing the same vault SHALL converge to the same state.

#### Scenario: Native and browser converge
- **WHEN** a native client and a browser client edit the same note concurrently while connected to one server
- **THEN** after both sync, both show the same body containing both edits
