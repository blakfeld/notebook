# app-shell Specification

## Purpose

Defines the Notebook client application experience shared by desktop, browser, and mobile: account and unlock flows, the sidebar tag tree and special views, the note list, note and tag operations, local search, sync status, and settings.

## Requirements

### Requirement: Recovery key is shown once and acknowledged
When a notebook is created, the app SHALL generate the recovery key on the device and show it. It SHALL explain that the key is needed to open a backup on another computer or after the secure store is lost. The user SHALL confirm they have saved it before the notebook opens. The app SHALL NOT store the recovery key or offer to show it again later.

#### Scenario: Acknowledgment required
- **WHEN** the recovery key is shown and the user has not confirmed saving it
- **THEN** the app does not open the notebook

#### Scenario: Recovery key never redisplayed
- **WHEN** notebook creation has finished
- **THEN** no screen in the app shows the recovery key again

### Requirement: Credentials in the OS secure store
On native platforms, the vault key SHALL be stored only in the operating system's secure credential store. It SHALL NOT be written to plain files, the local database, logs, or web storage. The recovery entry MAY be stored in the local database, since it opens only with the recovery key.

#### Scenario: Nothing secret in app files
- **WHEN** the app's data directory is inspected after a notebook is created
- **THEN** it contains no vault key, note key, or recovery key in usable form

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
The user SHALL be able to permanently delete a note only from Trash, after confirming. Permanent deletion SHALL remove the note from the device at once, with no network needed. Backups taken earlier still contain it.

#### Scenario: Permanent delete
- **WHEN** the user permanently deletes a trashed note and confirms
- **THEN** it disappears from Trash and does not reappear after a relaunch

#### Scenario: Cancelled delete
- **WHEN** the user cancels the confirmation
- **THEN** the note stays in Trash

#### Scenario: Delete while offline
- **WHEN** the user permanently deletes a trashed note with no network connection
- **THEN** the note is deleted at once, exactly as it would be online

### Requirement: Rename and move a tag
The user SHALL be able to rename a tag from the sidebar, entering a full tag path so a rename can also move it under another parent. Every note carrying the tag or its descendants, including archived and trashed notes, SHALL be rewritten as ordinary edits.

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

### Requirement: Offline editing is durable
Every note operation SHALL work with no network connection. Every change SHALL survive quitting and relaunching the app, and a change reported as saved SHALL survive a crash.

#### Scenario: Offline edit survives restart
- **WHEN** the user edits a note with no network connection, quits, and relaunches
- **THEN** the edit is present after relaunch

### Requirement: Vim mode setting
The vim mode toggle SHALL apply to the open editor immediately and SHALL be stored on the device, defaulting to off.

#### Scenario: Toggle applies immediately
- **WHEN** the user turns vim mode on while a note is open
- **THEN** the open editor accepts vim commands without reopening the note

#### Scenario: Per-device
- **WHEN** vim mode is turned on
- **THEN** the setting is stored only on this device and is not part of the vault or its backups

#### Scenario: Remembered across launches
- **WHEN** vim mode is turned on and the app is relaunched
- **THEN** vim mode is still on

### Requirement: First launch creates a notebook
When the app starts with no local vault, it SHALL show a welcome screen that offers to create a new notebook. Creating one SHALL generate a new vault ID, vault key, and recovery key on the device, with no account, username, passphrase, or network request.

#### Scenario: Create a notebook
- **WHEN** a user launches the app for the first time and chooses to create a notebook
- **THEN** after the recovery key is acknowledged, the main layout appears with an empty Notes view

#### Scenario: No network needed
- **WHEN** the first launch happens with no network connection
- **THEN** the notebook is created exactly as it would be online

### Requirement: Open on launch
On every launch after the first, when the vault key is in the OS secure credential store, the app SHALL open the vault and show the main layout without any prompt.

#### Scenario: Relaunch
- **WHEN** the user quits and relaunches the app
- **THEN** the main layout appears with all notes and no prompt

### Requirement: Recovery key entry without a secure store
When the operating system's secure credential store is unavailable, the app SHALL still create and open notebooks. It SHALL NOT persist the vault key. At each launch it SHALL ask for the recovery key and open the vault with it, and it SHALL say why it is asking. It SHALL NOT fall back to storing the vault key in a file.

#### Scenario: Secret Service missing on Linux
- **WHEN** the app is relaunched on a system with no secure credential store
- **THEN** it asks for the recovery key, explains that the system has no secure store, and opens the notes once a correct key is entered

#### Scenario: Wrong recovery key
- **WHEN** a valid-looking recovery key that does not belong to this vault is entered
- **THEN** the vault stays closed and the app says the key does not open this notebook

#### Scenario: Mistyped recovery key
- **WHEN** the entered recovery key fails its checksum
- **THEN** the app says the key looks mistyped, distinct from a key that does not open the notebook

### Requirement: Upgrade from a server-backed install
When the app starts and finds a device previously signed in to a sync server, it SHALL convert it to a local notebook once and keep every note on the device. It SHALL obtain the vault key from the stored automatic-unlock entry, or else ask for the old passphrase and use the stored device-local unlock. It SHALL then show and require acknowledgment of a newly generated recovery key, store the vault key under the local scheme, and delete the old session and unlock entries.

#### Scenario: Signed-in install upgrades silently except for the recovery key
- **WHEN** a device that was signed in and not locked starts the new version
- **THEN** the app shows a new recovery key to acknowledge, and then the main layout with every note that was on the device

#### Scenario: Locked install asks for the old passphrase once
- **WHEN** a device that was locked starts the new version and the user enters the old passphrase
- **THEN** the upgrade completes and later launches do not ask for any passphrase

#### Scenario: Upgrade cannot recover the key
- **WHEN** neither entry is present, or the user cannot supply the old passphrase and chooses to start over
- **THEN** the old database is kept beside the new one under a different name, not deleted, and a new empty notebook is created

### Requirement: No network access
The desktop app SHALL make no network requests on its own. Its content security policy SHALL permit connections only to the app itself and its host IPC channel. Opening a link in the system browser at the user's request is not a network request by the app.

#### Scenario: No outbound connections
- **WHEN** the app is used to create, edit, tag, search, and delete notes while network traffic is monitored
- **THEN** the app makes no outbound connection

### Requirement: Views update live
The sidebar, note list, and open editor SHALL reflect changes without a manual refresh: new, edited, retagged, moved between views, and deleted notes SHALL appear, move, or disappear as soon as the change is applied.

#### Scenario: Edited title moves the note
- **WHEN** the user edits the title of a listed note that is not the most recently updated
- **THEN** the entry's title updates and the note moves to the top of the list

#### Scenario: New tag appears
- **WHEN** the user types `#ideas` into a note on this device
- **THEN** `ideas` appears in the sidebar tag tree

### Requirement: Settings screen
The app SHALL provide a settings screen with an action to toggle vim mode and the app's version. The screen SHALL be organized in sections so other capabilities can add their own settings alongside these.

#### Scenario: Settings contents
- **WHEN** the user opens settings
- **THEN** the vim mode toggle and the app version are shown, and nothing refers to a server, account, passphrase, lock, or sign-out
