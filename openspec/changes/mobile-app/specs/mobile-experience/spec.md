# Spec Delta

## Purpose

Defines how the native iOS and Android app behaves where a phone or tablet differs from a desktop: touch editing, system navigation, secure credential storage, app lock, lifecycle-driven sync, quick capture, and sharing exports.

## ADDED Requirements

### Requirement: Native mobile builds
The app SHALL build and run on iOS and Android with the same note, tag, storage, sync, and encryption behavior as the desktop app, keeping a complete local copy of the vault that is readable and editable offline.

#### Scenario: Offline editing on a phone
- **WHEN** the phone has no network connection and the user edits a note
- **THEN** the edit is saved locally and is pushed to the server once a connection returns

#### Scenario: Edits converge across devices
- **WHEN** the same note is edited on the phone and on the desktop while both are online
- **THEN** both devices show the merged text without the user reloading

### Requirement: System back navigation
In the stacked layout, the Android system back gesture or button and the iOS edge-swipe back gesture SHALL move one level up the stack. On Android, back at the first screen SHALL leave the app as other Android apps do.

#### Scenario: Back from the editor
- **WHEN** a note is open in the stacked layout and the user performs the platform back gesture
- **THEN** the note list it was opened from is shown with its scroll position preserved

#### Scenario: Back at the root on Android
- **WHEN** the sidebar is shown and the user presses Android back
- **THEN** the app moves to the background

### Requirement: Formatting toolbar
While the editor has focus and the virtual keyboard is shown, a toolbar SHALL sit directly above the keyboard with these actions: heading, bold, italic, bulleted list, task, tag, link, undo, redo, and dismiss keyboard. Using a toolbar action SHALL NOT move focus out of the editor or close the keyboard, except dismiss.

#### Scenario: Toolbar appears with the keyboard
- **WHEN** the user taps into a note's text and the virtual keyboard opens
- **THEN** the formatting toolbar is shown directly above the keyboard

#### Scenario: Toolbar hidden without a virtual keyboard
- **WHEN** a hardware keyboard is attached and no virtual keyboard is shown
- **THEN** the formatting toolbar is not shown

#### Scenario: Dismiss keyboard
- **WHEN** the user taps dismiss on the toolbar
- **THEN** the keyboard and toolbar close and the note stays open

### Requirement: Inline formatting actions
Bold and italic SHALL wrap the selection in `**` or `*`, or remove those markers when the selection is already wrapped. With no selection they SHALL insert the marker pair with the caret between. Link SHALL turn the selection into `[selection]()` with the caret inside the parentheses.

#### Scenario: Bold a word
- **WHEN** `word` is selected and the user taps bold
- **THEN** the text becomes `**word**` and `word` stays selected

#### Scenario: Unbold
- **WHEN** `word` inside `**word**` is selected and the user taps bold
- **THEN** the text becomes `word`

#### Scenario: Bold with no selection
- **WHEN** the caret is in empty space and the user taps bold
- **THEN** `****` is inserted with the caret between the two pairs

#### Scenario: Link from selection
- **WHEN** `docs` is selected and the user taps link
- **THEN** the text becomes `[docs]()` with the caret between the parentheses

### Requirement: Line formatting actions
Heading SHALL cycle the caret line's prefix through none, `# `, `## `, `### `, and back to none. Bulleted list SHALL toggle a `- ` prefix on every selected line. Task SHALL run the editor's toggle-task command on every selected line, and on a line with no list marker SHALL add `- [ ] `.

#### Scenario: Cycle heading levels
- **WHEN** the caret is on the line `Plan` and the user taps heading three times
- **THEN** the line reads `# Plan`, then `## Plan`, then `### Plan`

#### Scenario: Make a list item a task
- **WHEN** the caret is on `- milk` and the user taps task
- **THEN** the line reads `- [ ] milk`

#### Scenario: Task on a plain line
- **WHEN** the caret is on `milk` and the user taps task
- **THEN** the line reads `- [ ] milk`

#### Scenario: Complete a task
- **WHEN** the caret is on `- [ ] milk` and the user taps task
- **THEN** the line reads `- [x] milk`

#### Scenario: Toggle list off
- **WHEN** the caret is on `- milk` and the user taps bulleted list
- **THEN** the line reads `milk`

### Requirement: Tag action
The tag action SHALL insert `#` at the caret. When text is selected, it SHALL tag that text: `#word` for a single word, and `#several words#` when the selection contains spaces, so the result parses as one tag under the tag rules.

#### Scenario: Insert tag marker
- **WHEN** the caret is after a space and the user taps tag
- **THEN** `#` is inserted and the caret follows it

#### Scenario: Tag a multi-word selection
- **WHEN** `side project` is selected and the user taps tag
- **THEN** the text becomes `#side project#`

### Requirement: Toolbar undo and redo
Undo and redo on the toolbar SHALL perform the editor's undo and redo, which revert only the user's own local edits, and SHALL be disabled when there is nothing to undo or redo.

#### Scenario: Undo a toolbar action
- **WHEN** the user bolds a word and then taps undo
- **THEN** the markers are removed and remote edits made meanwhile are kept

### Requirement: Caret stays visible above the keyboard
When the virtual keyboard opens, resizes, or the device rotates, the editor SHALL keep the caret visible above the toolbar and keyboard, and no editor content SHALL be reachable only behind them.

#### Scenario: Typing at the end of a long note
- **WHEN** the user taps the last line of a note longer than the screen and the keyboard opens
- **THEN** the editor scrolls so the caret line is visible above the toolbar

#### Scenario: Last lines reachable
- **WHEN** the keyboard is open and the user scrolls to the end of the note
- **THEN** the last line can be scrolled fully above the toolbar

### Requirement: Safe areas
Interactive controls and note text SHALL NOT be placed under the status bar, notch or camera cutout, rounded corners, or home indicator, in portrait and landscape.

#### Scenario: Landscape with a notch
- **WHEN** a phone with a camera cutout is rotated to landscape
- **THEN** no text or control is drawn under the cutout or the home indicator

### Requirement: Live preview on touch
Tapping rendered text in live preview SHALL place the caret at the corresponding position in the underlying markdown and reveal that line's syntax. Tapping a rendered task checkbox SHALL toggle it without raising the keyboard. Touch selection handles SHALL select underlying text across hidden syntax.

#### Scenario: Tap a formatted word
- **WHEN** the user taps inside the rendered bold word on a line showing `**word**` as bold text
- **THEN** the caret is placed inside `word` and the `**` markers become visible on that line

#### Scenario: Tap a task checkbox
- **WHEN** the keyboard is closed and the user taps an unchecked task's checkbox
- **THEN** the underlying text changes from `- [ ]` to `- [x]` and the keyboard stays closed

### Requirement: Vim mode on mobile
Vim mode SHALL be available in the mobile settings as the same per-device setting as on other clients, defaulting to off. With vim mode on, the formatting toolbar SHALL still apply its actions to the text.

#### Scenario: Default on a new device
- **WHEN** the app is installed on a phone and a note is opened
- **THEN** the editor accepts plain typing with vim mode off

#### Scenario: Vim with a hardware keyboard
- **WHEN** the user turns vim mode on and types with a hardware keyboard
- **THEN** vim motions and commands work in the editor

### Requirement: Secure credential storage on mobile
On iOS the OS secure store SHALL be the Keychain; on Android, storage encrypted under a non-exportable Android Keystore key. Stored credentials and key material SHALL be readable only on this device while the device is unlocked and SHALL NOT be included in backups or device transfers.

#### Scenario: Restored backup on a new phone
- **WHEN** the user restores a device backup of the phone onto a new phone and opens the app
- **THEN** the app is signed out and asks the user to sign in

#### Scenario: Device locked
- **WHEN** the device screen is locked
- **THEN** the app cannot read its stored credentials until the device is unlocked

### Requirement: Local data excluded from backups
The local database and any other app data SHALL be excluded from device and cloud backups, since the server holds the encrypted vault and the app re-downloads it after sign-in.

#### Scenario: Backup contents
- **WHEN** the device makes a backup
- **THEN** the backup contains no note data from the app

### Requirement: App lock
The user SHALL be able to turn on an app lock, which requires device authentication (biometrics, with the device passcode as fallback) or the passphrase to open the vault. Turning it on SHALL require a successful device authentication. It SHALL be unavailable, with an explanation, when the device has no passcode set.

#### Scenario: Turning on app lock
- **WHEN** the user turns on app lock in settings and passes device authentication
- **THEN** app lock is on and the next launch asks for device authentication before showing notes

#### Scenario: No device passcode
- **WHEN** the device has no passcode set and the user opens app lock settings
- **THEN** the setting is disabled with a message saying a device passcode is required

### Requirement: Stored keys gated by device authentication
When app lock is on, the key material that opens the vault without the passphrase SHALL be protected so the operating system releases it only after device authentication. Failing or cancelling authentication SHALL leave the vault locked with no note content shown, offering to retry or to unlock with the passphrase.

#### Scenario: Cold launch with app lock
- **WHEN** app lock is on and the app is launched after being closed
- **THEN** no note titles or content are shown until device authentication succeeds

#### Scenario: Cancelled authentication
- **WHEN** the user cancels the authentication prompt
- **THEN** the lock screen remains, offering to try again or to enter the passphrase

#### Scenario: Unlock with the passphrase instead
- **WHEN** the lock screen is shown after an auto-lock and the user enters the correct passphrase while offline
- **THEN** the vault opens with all local notes

### Requirement: Fallback to passphrase unlock
If the operating system can no longer release the device-authentication-protected key material (for example, the device passcode was removed), the app SHALL discard it, turn app lock off, and show the passphrase unlock screen. Local edits not yet pushed SHALL be kept and pushed after unlocking.

#### Scenario: Passcode removed
- **WHEN** app lock is on, the user removes the device passcode, and opens the app
- **THEN** the app asks for the passphrase, and after unlocking, pending local edits are synced

### Requirement: Auto-lock after time in background
When app lock is on, the app SHALL auto-lock once it has been in the background longer than the auto-lock delay: immediately, 1 minute, 5 minutes (default), 15 minutes, or 1 hour. Auto-lock SHALL discard in-memory keys and decrypted notes and stop sync, like the app-shell lock, but SHALL keep the stored key material so device authentication can unlock.

#### Scenario: Return within the delay
- **WHEN** the auto-lock delay is 5 minutes and the user returns to the app after 2 minutes
- **THEN** the app shows the note that was open without asking for authentication

#### Scenario: Return after the delay
- **WHEN** the auto-lock delay is 5 minutes and the user returns after 10 minutes
- **THEN** the app asks for device authentication, and after success shows the same note again

#### Scenario: Clock moved backwards
- **WHEN** the device clock is set back while the app is in the background
- **THEN** the app treats the delay as exceeded and locks

### Requirement: Explicit lock on mobile
The app-shell lock action SHALL behave as on other clients even when app lock is on: it SHALL remove the stored key material that opens the vault without the passphrase, so only the passphrase unlocks it.

#### Scenario: Lock from settings
- **WHEN** app lock is on and the user chooses Lock in settings
- **THEN** the unlock screen asks for the passphrase and offers no device authentication

### Requirement: App switcher privacy
When app lock is on, the app SHALL hide note titles and content from the system app switcher and task snapshots while it is in the background.

#### Scenario: App switcher preview
- **WHEN** app lock is on, a note is open, and the user opens the app switcher
- **THEN** the app's preview shows a neutral placeholder instead of the note

### Requirement: Sync on foreground
When the app returns to the foreground with the vault unlocked, it SHALL immediately pull remote changes and resume live updates, so changes made elsewhere appear without a manual refresh.

#### Scenario: Edit from the desktop while the phone was in the background
- **WHEN** a note is edited on the desktop while the phone app is in the background, and the phone app is brought to the foreground
- **THEN** the phone shows the edit without user action

### Requirement: Flush on background
When the app moves to the background, it SHALL commit any pending editor changes to local storage before the operating system can suspend it, and SHALL attempt to push the outbox. Changes not pushed SHALL remain in the outbox and be pushed on the next sync.

#### Scenario: Switch apps right after typing
- **WHEN** the user types a word and immediately switches to another app
- **THEN** the word is in local storage, and is pushed to the server if the network allows before suspension

#### Scenario: App killed while in the background
- **WHEN** the operating system terminates the app after it was backgrounded with unpushed edits
- **THEN** on next launch the edits are still present and are pushed

### Requirement: Quick capture
The app SHALL offer a "New Note" home-screen quick action on iOS and app shortcut on Android, and SHALL handle the link `notebook://new`. Each SHALL open the app on a new, empty note in the editor with the keyboard up, after device authentication if the app is locked.

#### Scenario: Quick action from the home screen
- **WHEN** the user long-presses the app icon and chooses New Note
- **THEN** the app opens in the editor on a new empty note with the keyboard shown

#### Scenario: Quick capture while locked
- **WHEN** app lock is on and the user chooses New Note
- **THEN** device authentication is requested first, then the new note opens

### Requirement: Quick capture is content-free
The `notebook://new` link SHALL ignore any path, query, or fragment beyond `new`, so other apps cannot inject note content. A quick-capture note left empty SHALL NOT remain in the note list.

#### Scenario: Link with content
- **WHEN** another app opens `notebook://new?text=hello`
- **THEN** a new empty note opens and does not contain `hello`

#### Scenario: Abandoned quick note
- **WHEN** the user opens a quick-capture note and navigates back without typing
- **THEN** no new note appears in the note list

### Requirement: Export through the share sheet
On mobile, exporting all notes SHALL produce the markdown-export zip archive and present it in the platform share sheet, so the user can save it to Files or send it to another app. The temporary archive SHALL be deleted when the share sheet closes, and any leftover archive SHALL be deleted on the next launch.

#### Scenario: Save export to Files
- **WHEN** the user chooses Export all notes and picks Save to Files in the share sheet
- **THEN** a `notebook-export-YYYYMMDD-HHMMSS.zip` file with the exported notes is saved where the user chose

#### Scenario: Cancelled share
- **WHEN** the user closes the share sheet without choosing a destination
- **THEN** no export archive remains in the app's storage
