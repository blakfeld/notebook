# app-shell Specification

## Purpose

Defines the Notebook client application experience shared by desktop, browser, and mobile: account and unlock flows, the sidebar tag tree and special views, the note list, note and tag operations, local search, sync status, and settings.

## Requirements

### Requirement: Server address
Sign-in, registration, and recovery SHALL take the server's address from the user. The app SHALL accept only `https` addresses, except `http` addresses whose host is a loopback address or `localhost`. Any other address SHALL be rejected before contacting a server, with a message naming the problem.

#### Scenario: HTTPS address accepted
- **WHEN** the user enters `https://notes.example.com` as the server
- **THEN** the app accepts the address and contacts that server

#### Scenario: Plain HTTP to a remote host rejected
- **WHEN** the user enters `http://notes.example.com` as the server
- **THEN** the app rejects it without making any network request and says HTTPS is required

#### Scenario: Plain HTTP to loopback accepted
- **WHEN** the user enters `http://127.0.0.1:8080` as the server
- **THEN** the app accepts the address

### Requirement: Sign in
A signed-out user SHALL be able to sign in with server address, username, and passphrase. On success the app SHALL unwrap the vault key locally, load the vault, and show the main layout. The passphrase SHALL NOT be sent to the server or stored anywhere.

#### Scenario: Successful sign-in
- **WHEN** a signed-out user enters a valid server, username, and passphrase
- **THEN** the app shows the main layout with that account's notes

#### Scenario: Sign-in on a fresh install downloads the vault
- **WHEN** a user signs in on a device that has never held their vault and the vault has notes
- **THEN** every note appears in the note list once the initial sync completes

### Requirement: Sign-in failures are generic
When sign-in fails because the username is unknown or the passphrase is wrong, the app SHALL show the same message in both cases. A network failure or rate limiting SHALL be reported as such, distinct from bad credentials.

#### Scenario: Unknown user and wrong passphrase look the same
- **WHEN** one sign-in uses an unknown username and another uses a known username with the wrong passphrase
- **THEN** both show the same "username or passphrase is incorrect" message

#### Scenario: Server unreachable
- **WHEN** the user signs in while the server cannot be reached
- **THEN** the app says the server could not be reached rather than that the credentials are wrong

### Requirement: Registration with an invite code
A signed-out user SHALL be able to register with server address, username, passphrase entered twice, and an invite code. The app SHALL reject a passphrase shorter than 12 characters or entries that do not match before contacting the server, and SHALL show the server's reason when it refuses the invite code or username.

#### Scenario: Mismatched passphrases
- **WHEN** the two passphrase entries differ
- **THEN** the app refuses to continue and no request is sent

#### Scenario: Invite code refused
- **WHEN** the server rejects the invite code as invalid, used, or expired
- **THEN** the app shows that the invite code was not accepted and no account is created

#### Scenario: Username taken
- **WHEN** the server refuses the username as unavailable
- **THEN** the app asks for a different username and keeps the other entries

#### Scenario: Successful registration
- **WHEN** registration completes
- **THEN** the user is signed in to an empty vault and sees the main layout

### Requirement: Recovery key is shown once and acknowledged
During registration the app SHALL generate the recovery key on the device and show it before the account is created. The user SHALL explicitly confirm they have saved it before registration proceeds. The app SHALL NOT store the recovery key or offer to show it again later.

#### Scenario: Acknowledgment required
- **WHEN** the recovery key is shown and the user has not confirmed saving it
- **THEN** the app does not let registration continue

#### Scenario: Recovery key never redisplayed
- **WHEN** registration has finished
- **THEN** no screen in the app shows the recovery key again

### Requirement: Account recovery with the recovery key
A signed-out user SHALL be able to recover an account with server address, username, recovery key, and a new passphrase entered twice. On success the new passphrase replaces the old one and the user is signed in with all notes intact. A malformed recovery key SHALL be reported as mistyped, distinct from a key that does not unlock the account.

#### Scenario: Successful recovery
- **WHEN** the user enters the correct recovery key and a new passphrase
- **THEN** the user is signed in, all notes are present, and the new passphrase signs in from then on

#### Scenario: Typo in recovery key
- **WHEN** the entered recovery key fails its checksum
- **THEN** the app says the key looks mistyped without contacting the server

### Requirement: Passphrase change
A signed-in user SHALL be able to change their passphrase from settings by entering the current passphrase and the new one twice, subject to the registration length rule. The change SHALL require a server connection. Afterwards the old passphrase SHALL no longer sign in or unlock this device.

#### Scenario: Passphrase changed
- **WHEN** the user changes their passphrase while online
- **THEN** signing in with the new passphrase works and with the old one fails

#### Scenario: Wrong current passphrase
- **WHEN** the current passphrase entered is wrong
- **THEN** the change is refused and the old passphrase still works

#### Scenario: Offline change refused
- **WHEN** the user tries to change the passphrase while the server is unreachable
- **THEN** the app says a connection is required and nothing changes

### Requirement: Sign out
Signing out SHALL revoke this device's session on the server when reachable, remove every stored credential, delete the device's local copy of the vault, and return to the sign-in screen. If unsynced local changes exist, the app SHALL show how many and require confirmation, since they would be lost.

#### Scenario: Sign out with everything synced
- **WHEN** the user signs out with no pending changes
- **THEN** the app returns to sign-in, and a relaunch shows the sign-in screen with no notes on the device

#### Scenario: Sign out with pending changes
- **WHEN** the user signs out with 3 unsynced changes
- **THEN** the app warns that 3 changes will be lost and signs out only if the user confirms

### Requirement: Credentials in the OS secure store
On native platforms, the session's refresh token and the key material needed to unlock the vault SHALL be stored only in the operating system's secure credential store. They SHALL NOT be written to plain files, the local database, logs, or web storage.

#### Scenario: Nothing secret in app files
- **WHEN** the app's data directory is inspected after sign-in
- **THEN** it contains no refresh token, vault key, or note key in usable form

### Requirement: Unlock on launch
On native platforms, when the app starts with valid stored credentials and was not locked, it SHALL open the vault without asking for the passphrase, including while offline.

#### Scenario: Relaunch while signed in
- **WHEN** a signed-in user quits and relaunches the app
- **THEN** the main layout appears without a passphrase prompt

#### Scenario: Relaunch offline
- **WHEN** a signed-in user relaunches the app with no network
- **THEN** the main layout appears with all local notes and the sync indicator shows offline

### Requirement: No secure store available
When the operating system's secure credential store is unavailable, the app SHALL still allow signing in but SHALL NOT persist credentials, SHALL tell the user they will need to sign in each launch, and SHALL NOT fall back to storing secrets in a file.

#### Scenario: Secret Service missing on Linux
- **WHEN** the user signs in on a system with no secure credential store
- **THEN** the session works, a notice explains credentials are not remembered, and the next launch shows the sign-in screen

### Requirement: Lock
A signed-in user SHALL be able to lock the app. Locking SHALL discard all decrypted note content and keys from memory, hide all note content, stop sync, and require the passphrase to unlock. The locked state SHALL persist across relaunches.

#### Scenario: Lock hides notes
- **WHEN** the user locks the app
- **THEN** the unlock screen is shown and no note titles or content are visible

#### Scenario: Locked app relaunches locked
- **WHEN** a locked app is quit and relaunched
- **THEN** it shows the unlock screen rather than the notes

### Requirement: Unlock with the passphrase
A locked app SHALL unlock when the user enters their current passphrase, whether or not the server is reachable. A wrong passphrase SHALL leave the app locked.

#### Scenario: Unlock online
- **WHEN** the user enters the correct passphrase on the unlock screen while online
- **THEN** the main layout reappears and sync resumes

#### Scenario: Unlock offline
- **WHEN** the user enters the correct passphrase while the server is unreachable
- **THEN** the main layout reappears with all local notes

#### Scenario: Wrong passphrase
- **WHEN** the user enters a wrong passphrase on the unlock screen
- **THEN** the app stays locked and says the passphrase is incorrect

### Requirement: Passphrase changed on another device
Once a device learns that the account's passphrase was changed elsewhere, the old passphrase SHALL no longer unlock that device. The next unlock SHALL require the new passphrase and a server connection.

#### Scenario: Old passphrase stops working locally
- **WHEN** the passphrase is changed on device A and device B syncs and is then locked
- **THEN** device B does not unlock with the old passphrase and unlocks with the new one when online

### Requirement: Session expiry
When the session can no longer be refreshed, the app SHALL keep the vault open and keep accepting edits locally, show that sign-in is required for sync, and resume syncing pending changes after the user re-enters their passphrase.

#### Scenario: Revoked session
- **WHEN** this device's session is revoked on the server while the app is open
- **THEN** the sync indicator says sign-in is required, local edits continue to save, and after re-entering the passphrase the pending edits sync

### Requirement: Decrypted data stays in memory
Note content, titles, tags, and the search index SHALL exist in decrypted form only in memory. Data written to the device's disk SHALL be ciphertext or non-sensitive settings, and logs SHALL NOT contain note content, keys, tokens, passphrases, or recovery keys.

#### Scenario: Search term not on disk
- **WHEN** a note containing a unique word is created and the app's data directory is searched for that word
- **THEN** the word is not found in any file

### Requirement: Three-pane layout
When unlocked, the app SHALL show three panes side by side: the sidebar, the note list for the sidebar selection, and the editor for the selected note. On first unlock the sidebar selection SHALL be Notes.

#### Scenario: Initial layout
- **WHEN** the vault is first opened
- **THEN** the sidebar, a note list showing Notes, and the editor pane are visible

### Requirement: Sidebar tag tree
The sidebar SHALL show the vault's tag tree with nested tags under their parents, in the tag tree's order and display spelling. Each tag with children SHALL expand and collapse, and the expansion state SHALL be remembered on the device.

#### Scenario: Nested tags shown under parents
- **WHEN** notes carry `#work/acme` and `#home`
- **THEN** the sidebar shows `home` and `work`, with `acme` under `work`

#### Scenario: Collapse remembered
- **WHEN** the user collapses `work` and relaunches the app
- **THEN** `work` is still collapsed

### Requirement: Sidebar special views
The sidebar SHALL list the special views Notes, Untagged, Pinned, Archive, and Trash above the tag tree. Selecting a view or a tag SHALL show its notes in the note list.

#### Scenario: Selecting a tag
- **WHEN** the user selects `work` and notes carry `#work` and `#work/acme`
- **THEN** the note list shows the notes carrying either tag

#### Scenario: Selecting Trash
- **WHEN** the user selects Trash
- **THEN** the note list shows only trashed notes

### Requirement: Note list
The note list SHALL show the notes of the current selection ordered by updated time, most recent first. Each entry SHALL show the note's title, or "Untitled" when the title is empty, a plain-text preview of the text after the title line, and its updated time.

#### Scenario: Most recent first
- **WHEN** note A was updated before note B and both are in the selection
- **THEN** B is listed above A

#### Scenario: Untitled note
- **WHEN** a note's body is empty
- **THEN** its entry shows the title "Untitled"

### Requirement: Live updates of sidebar and list
The sidebar, note list, and open editor SHALL reflect local and remote changes without a manual refresh: new, edited, retagged, moved between views, and deleted notes SHALL appear, move, or disappear as soon as the change is applied.

#### Scenario: Remote edit appears
- **WHEN** another device edits a note's title while it is listed on this device
- **THEN** the entry's title updates and the note moves to the top of the list

#### Scenario: New tag appears
- **WHEN** the user types `#ideas` into a note on this device
- **THEN** `ideas` appears in the sidebar tag tree

### Requirement: Editor pane
Selecting a note in the list SHALL open it in the editor pane using the note editor. When no note is selected, the editor pane SHALL show an empty state. Edits made in the editor SHALL be saved automatically with no save action.

#### Scenario: Open a note
- **WHEN** the user selects a note in the list
- **THEN** its body is shown in the editor and can be edited

#### Scenario: No selection
- **WHEN** the selection's note list is empty
- **THEN** the editor pane shows an empty state

### Requirement: Create a note
The user SHALL be able to create a note, which is selected and opened in the editor with the cursor ready for typing. When a tag is selected in the sidebar, the new note's body SHALL start with that tag on its own line so the note appears under it.

#### Scenario: New note under a tag
- **WHEN** the user creates a note while `work/acme` is selected
- **THEN** the new note's body starts with `#work/acme` and it appears in that tag's list

#### Scenario: New note in Notes
- **WHEN** the user creates a note while Notes is selected
- **THEN** an empty note is created, listed first, and opened in the editor

### Requirement: Pin and unpin
The user SHALL be able to pin and unpin a note from the note list and from the editor. Pinned notes SHALL appear in the Pinned view.

#### Scenario: Pin a note
- **WHEN** the user pins a note
- **THEN** it appears in Pinned and stays in its other lists

### Requirement: Archive and unarchive
The user SHALL be able to archive and unarchive a note. Archiving SHALL move it out of Notes, Untagged, Pinned, and the tag tree into Archive.

#### Scenario: Archive a note
- **WHEN** the user archives a note carrying `#work`
- **THEN** it leaves the `work` list and Notes and appears in Archive

### Requirement: Trash and restore
The user SHALL be able to move a note to Trash and to restore it from Trash. Restoring SHALL return the note to the views it was in before it was trashed.

#### Scenario: Restore an archived note
- **WHEN** an archived note is trashed and then restored
- **THEN** it appears in Archive again

### Requirement: Trashed notes are read-only
A trashed note SHALL open read-only in the editor, with an action to restore it. It SHALL become editable again once restored.

#### Scenario: Typing in a trashed note
- **WHEN** the user opens a trashed note and types
- **THEN** the body does not change

### Requirement: Delete permanently
The user SHALL be able to permanently delete a note only from Trash, after confirming. Permanent deletion SHALL require a server connection: the note SHALL be deleted on the server first, then disappear from every view on this device, and from the user's other devices once they sync. When the server cannot be reached, the app SHALL say a connection is required and the note SHALL stay in Trash.

#### Scenario: Permanent delete
- **WHEN** the user permanently deletes a trashed note and confirms
- **THEN** it disappears from Trash and does not reappear after other devices sync

#### Scenario: Cancelled delete
- **WHEN** the user cancels the confirmation
- **THEN** the note stays in Trash

#### Scenario: Delete while offline
- **WHEN** the user permanently deletes a trashed note while the server is unreachable
- **THEN** the app says a connection is required and the note stays in Trash

### Requirement: Rename and move a tag
The user SHALL be able to rename a tag from the sidebar, entering a full tag path so a rename can also move it under another parent. Every note carrying the tag or its descendants, including archived and trashed notes, SHALL be rewritten, and the changes SHALL sync like any edit.

#### Scenario: Rename
- **WHEN** the user renames `work` to `job` and a note's body is `#work/acme notes`
- **THEN** the note's body becomes `#job/acme notes` and the sidebar shows `job` with child `acme`

#### Scenario: Move under a new parent
- **WHEN** the user renames `acme` to `clients/acme`
- **THEN** `acme` appears under `clients` and its notes are rewritten

#### Scenario: Rename into an existing tag merges
- **WHEN** the user renames `job` to `work` while `work` already exists
- **THEN** the notes of both appear under a single `work` tag

### Requirement: Invalid tag rename rejected
A rename to a name that is not a valid tag name SHALL be refused with a message explaining why, and no note SHALL change.

#### Scenario: Invalid new name
- **WHEN** the user renames `work` to `42`
- **THEN** the app explains the name is not a valid tag and no note is rewritten

### Requirement: Local search
The user SHALL be able to search the notes in the current sidebar selection. The query SHALL be split on whitespace into terms, and a note SHALL match when every term occurs in its body, ignoring case. Results SHALL keep the note list's order. An empty query SHALL show the whole selection. Search SHALL work offline.

#### Scenario: All terms must match
- **WHEN** the query is `plumber sink` and one note mentions both words and another only `plumber`
- **THEN** only the note with both words is listed

#### Scenario: Case-insensitive
- **WHEN** the query is `GROCERY` and a note contains `grocery`
- **THEN** that note is listed

#### Scenario: Scoped to the selection
- **WHEN** Archive is selected and the query matches one archived and one non-archived note
- **THEN** only the archived note is listed

#### Scenario: Search updates live
- **WHEN** a search is active and the user edits a listed note so it no longer matches
- **THEN** the note leaves the results

### Requirement: Sync status indicator
The app SHALL always show the sync state (synced, syncing, or offline), the number of local changes not yet acknowledged by the server when it is greater than zero, and any condition needing the user: sign-in required, upgrade required, or storage full.

#### Scenario: Offline with pending edits
- **WHEN** the device is offline and the user has made 2 edits
- **THEN** the indicator shows offline and 2 pending changes

#### Scenario: Back online
- **WHEN** the connection returns and the pending changes are acknowledged
- **THEN** the indicator shows synced with no pending count

#### Scenario: Upgrade required
- **WHEN** a remote change arrives in a document format this app version does not support
- **THEN** the indicator says an app upgrade is required, and other notes keep syncing

#### Scenario: Storage full
- **WHEN** the server refuses local changes because the vault's storage quota is full
- **THEN** the indicator says storage is full, the refused changes stay pending and are counted, and remote changes still arrive

### Requirement: Offline editing is durable
Every note operation except permanent deletion SHALL work offline. Changes made offline SHALL survive quitting and relaunching the app and SHALL sync when the server becomes reachable again.

#### Scenario: Offline edit survives restart
- **WHEN** the user edits a note offline, quits, relaunches, and later goes online
- **THEN** the edit is present after relaunch and reaches the user's other devices once online

### Requirement: Settings
The app SHALL provide a settings screen that includes the server address, username, and sign-in status of this device, and actions to toggle vim mode, change the passphrase, lock, and sign out. The screen SHALL be organized in sections so other capabilities can add their own settings alongside these.

#### Scenario: Account information
- **WHEN** a signed-in user opens settings
- **THEN** the server address and username are shown along with the vim mode, change passphrase, lock, and sign-out actions

### Requirement: Vim mode setting
The vim mode toggle SHALL apply to the open editor immediately and SHALL be stored per device, defaulting to off. Changing it on one device SHALL NOT change it on another.

#### Scenario: Toggle applies immediately
- **WHEN** the user turns vim mode on while a note is open
- **THEN** the open editor accepts vim commands without reopening the note

#### Scenario: Per-device
- **WHEN** vim mode is turned on on one device
- **THEN** it remains off on the user's other devices
