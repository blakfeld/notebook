# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

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

## REMOVED Requirements

### Requirement: Server address
**Reason**: There is no sync server.
**Migration**: None. The app is local-only.

### Requirement: Sign in
**Reason**: There are no accounts. The vault opens from the OS secure store (see "Open on launch").
**Migration**: Existing signed-in installs convert once (see "Upgrade from a server-backed install").

### Requirement: Sign-in failures are generic
**Reason**: There is no sign-in.
**Migration**: None.

### Requirement: Registration with an invite code
**Reason**: Accounts are replaced by local notebook creation (see "First launch creates a notebook").
**Migration**: None.

### Requirement: Account recovery with the recovery key
**Reason**: There is no account to recover. The recovery key now opens backups (`vault-backup`) and the local vault when no secure store exists.
**Migration**: Restore from a backup with the recovery key.

### Requirement: Passphrase change
**Reason**: There is no passphrase.
**Migration**: None.

### Requirement: Sign out
**Reason**: There is no session. Removing a notebook from a device is not offered yet.
**Migration**: None.

### Requirement: Unlock on launch
**Reason**: Replaced by "Open on launch", which has no sign-in or offline caveats.
**Migration**: None.

### Requirement: No secure store available
**Reason**: Replaced by "Recovery key entry without a secure store".
**Migration**: None.

### Requirement: Lock
**Reason**: With no passphrase there is nothing to unlock with. An OS-authenticated app lock is listed as later work.
**Migration**: Rely on the OS user session lock.

### Requirement: Unlock with the passphrase
**Reason**: Lock is removed.
**Migration**: None.

### Requirement: Passphrase changed on another device
**Reason**: There are no other devices or passphrases.
**Migration**: None.

### Requirement: Session expiry
**Reason**: There are no sessions.
**Migration**: None.

### Requirement: Sync status indicator
**Reason**: There is no sync. Backup status is reported by `vault-backup`.
**Migration**: None.

### Requirement: Live updates of sidebar and list
**Reason**: Restated without remote changes as "Views update live".
**Migration**: None.

### Requirement: Settings
**Reason**: Restated without account, passphrase, lock, and sign-out entries as "Settings screen".
**Migration**: None.
