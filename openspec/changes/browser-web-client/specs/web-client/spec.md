# Spec Delta

## Purpose

Defines how Notebook runs as a thin browser client: the vault is decrypted only in the tab's memory for one session, nothing sensitive is left on the device, edits sync live with the native apps and survive brief disconnections, and the delivered code is integrity-checked and runs under the server's strict content security policy.

## ADDED Requirements

### Requirement: In-memory vault unlock
After a successful sign-in, the browser client SHALL unwrap the key bundle, download the whole encrypted vault, and decrypt it into memory before showing the note list, displaying progress while it works. The unlocked client SHALL offer the same browsing, editing, tag, and search behavior as the native apps.

#### Scenario: Sign in and see notes
- **WHEN** a user with existing notes signs in from a fresh browser tab with the correct username and passphrase
- **THEN** the client shows bootstrap progress, then the tag tree and note list with every note's current content

#### Scenario: Edit from the browser
- **WHEN** a signed-in browser user edits a note
- **THEN** the change appears in the editor immediately and is sent to the server encrypted

#### Scenario: Wrong passphrase
- **WHEN** a user signs in with an incorrect passphrase
- **THEN** the client shows a generic sign-in failure, does not reveal whether the username exists, and stores nothing

### Requirement: No sensitive data persisted in the browser
The browser client SHALL NOT write keys, passphrases, recovery keys, session or refresh tokens, plaintext note content, ciphertext, note IDs, usernames, or tag names to localStorage, sessionStorage, IndexedDB, the origin private file system, Cache Storage, or cookies, and SHALL NOT register a service worker. The sole exception is a markdown export the user explicitly starts, which is handed to the browser as a file download and is not kept in any origin storage.

#### Scenario: Storage audit after a session
- **WHEN** a user signs in, edits and creates notes, and the browser's origin storage is then inspected
- **THEN** IndexedDB, the origin private file system, and Cache Storage are empty, no service worker is registered, no cookie was set by client code, and local and session storage hold only allowlisted preference keys

#### Scenario: Marker text never stored
- **WHEN** a user types a distinctive marker string into a note and origin storage is inspected
- **THEN** the marker appears in no stored value

#### Scenario: User-initiated export download
- **WHEN** a signed-in user chooses "Export all notes" and confirms
- **THEN** the browser downloads one `.zip` file containing the plaintext export, and afterwards origin storage still holds only allowlisted preference keys and the page keeps no reference to the archive

### Requirement: Allowlisted device preferences
The browser client SHALL persist only an explicit allowlist of non-sensitive UI preferences, at least the vim mode toggle and the theme choice, in local storage, so they survive a new session on the same browser. Values outside the allowlist SHALL NOT be written.

#### Scenario: Vim mode survives a new session
- **WHEN** a user turns on vim mode, closes the tab, opens the client again, and signs in
- **THEN** vim mode is on

#### Scenario: Storage unavailable
- **WHEN** local storage is blocked or throws, as in some private browsing modes
- **THEN** the client works normally with default preferences and shows no error

### Requirement: Session ends with the page
Closing, reloading, or navigating away from the client's page SHALL end the session. Afterwards the user SHALL have to enter the passphrase again, and no keys or plaintext from the earlier session SHALL be reachable by the new page.

#### Scenario: Reload asks for the passphrase
- **WHEN** a signed-in user reloads the page
- **THEN** the sign-in screen is shown and no notes are visible until the user signs in again

#### Scenario: Back/forward cache
- **WHEN** a signed-in user navigates to another site and then presses the browser's Back button
- **THEN** the client shows the sign-in screen rather than the previously unlocked vault

### Requirement: Sign out and lock in the browser
Signing out SHALL discard all in-memory keys, tokens, and decrypted notes, ask the server to revoke the session, and return to the sign-in screen. Because the browser keeps no key material to unlock with, the lock action SHALL behave exactly like sign-out.

#### Scenario: Sign out
- **WHEN** a signed-in user signs out
- **THEN** the sign-in screen is shown, no note content remains in the page, and the server session is revoked

#### Scenario: Sign out with no connection
- **WHEN** a user signs out while the server is unreachable
- **THEN** the local session is still discarded and the sign-in screen is shown

#### Scenario: Lock
- **WHEN** a signed-in user chooses lock
- **THEN** the result is the same as signing out

### Requirement: Live sync with other clients
While connected, the browser client SHALL receive other clients' edits in real time and apply them to the open editor, note list, and tag tree without a reload, and SHALL send its own edits as they are made.

#### Scenario: Edit on desktop appears in the browser
- **WHEN** the same note is open in the browser and the desktop app and the desktop user types
- **THEN** the typed text appears in the browser editor within a few seconds without moving the browser user's cursor

#### Scenario: Concurrent edits converge
- **WHEN** the browser and another client edit the same note at the same time
- **THEN** both end up showing the same text containing both edits

### Requirement: Offline buffering in memory
When the connection to the server is lost, the browser client SHALL keep accepting edits, hold unsent updates in memory, show an unsynced-changes indicator with the pending count, and retry with backoff. When the connection returns it SHALL send the buffered updates and clear the indicator once the server acknowledges them.

#### Scenario: Edit while offline
- **WHEN** the network drops and the user keeps editing a note
- **THEN** editing continues and the client shows that changes are unsynced

#### Scenario: Reconnect flushes the buffer
- **WHEN** the network returns after edits were made offline
- **THEN** the buffered edits reach the server, other clients receive them, and the indicator returns to synced

### Requirement: Unload warning while changes are pending
While any update has not been acknowledged by the server, the browser client SHALL ask the browser to confirm before the page is closed, reloaded, or navigated away. With no pending updates it SHALL NOT prompt.

#### Scenario: Close with pending changes
- **WHEN** the user tries to close the tab while edits are unsynced
- **THEN** the browser shows its leave-page confirmation

#### Scenario: Close when synced
- **WHEN** the user closes the tab with all edits acknowledged
- **THEN** the tab closes without a confirmation

### Requirement: Re-authentication keeps buffered edits
When the session expires and cannot be refreshed, the browser client SHALL ask the same user to enter the passphrase again in the same page while keeping unsent edits in memory, and SHALL send them after re-authentication succeeds.

#### Scenario: Session expires with pending edits
- **WHEN** the session can no longer be refreshed while edits are buffered and the user re-enters the correct passphrase
- **THEN** the buffered edits are sent and no edit is lost

#### Scenario: Different user re-authenticates
- **WHEN** the re-authentication prompt is answered with a different username
- **THEN** the client discards the old session's data and starts a fresh session for the new user

### Requirement: Secrets kept out of URLs
The browser client SHALL NOT place passphrases, recovery keys, tokens, or keys in the page URL, its history, or the URL of any request it makes, including the live-update connection.

#### Scenario: Live connection URL
- **WHEN** the client opens its live-update connection
- **THEN** the connection URL contains no token or other credential

#### Scenario: Address bar after sign-in
- **WHEN** a user signs in
- **THEN** the page URL is unchanged and contains no credential

### Requirement: Account flows in the browser
The browser client SHALL let a user register a new account, recover an account with the recovery key, and change the passphrase, using the same flows and confirmations as the native apps. After registration or recovery the user SHALL be signed in for the current page only.

#### Scenario: Register from the browser
- **WHEN** a new user completes registration in the browser
- **THEN** the recovery key is shown once and must be acknowledged, and the user then has an empty unlocked vault in this tab

#### Scenario: Recover from the browser
- **WHEN** a user enters a valid recovery key and a new passphrase in the browser
- **THEN** the vault is unlocked in this tab and the new passphrase works on every client

### Requirement: Bundle integrity
The client's entry script and WebAssembly module SHALL be loaded with Subresource Integrity hashes declared in the entry page. If a loaded file does not match its hash, the client SHALL NOT run and SHALL show a load-failure message.

#### Scenario: Integrity attributes present
- **WHEN** the built entry page is inspected
- **THEN** every script it loads and the WebAssembly module reference carry a `sha384` integrity value matching the file

#### Scenario: Tampered module
- **WHEN** the served WebAssembly module differs from the one the entry page declares
- **THEN** the client does not start and shows a message that it failed to load

### Requirement: Runs under the server's content security policy
The browser client SHALL run fully, including the editor and WebAssembly, under the server's content security policy without any policy violation and without inline scripts, `eval`, or inline style attributes.

#### Scenario: No violations during use
- **WHEN** a user signs in, edits a note with vim mode on, searches, and signs out while served by the real server
- **THEN** the browser reports no content security policy violations

### Requirement: Unsupported browsers are told so
When the browser lacks a capability the client requires, such as WebAssembly or a secure random source, the client SHALL show a message naming the problem instead of a blank or broken page.

#### Scenario: WebAssembly disabled
- **WHEN** the client is opened in a browser with WebAssembly disabled
- **THEN** a message says the browser is not supported and why

### Requirement: Independent tabs
Each browser tab SHALL hold its own independent session. Signing out or closing one tab SHALL NOT affect another, and edits made in one tab SHALL reach the other through the server like any other client.

#### Scenario: Two tabs
- **WHEN** a user is signed in in two tabs and edits a note in one
- **THEN** the edit appears in the other tab, and closing the first tab leaves the second signed in

### Requirement: Trust model is disclosed
The sign-in screen SHALL state that nothing is stored on the device and that closing the tab signs out, and the project documentation SHALL state that browser users trust the server to deliver unmodified client code on every load and recommend the native apps when that trust is not acceptable.

#### Scenario: Sign-in notice
- **WHEN** the sign-in screen is shown
- **THEN** it states that nothing is stored on this device and that closing the tab signs out

#### Scenario: Documentation
- **WHEN** a reader opens the security documentation
- **THEN** it explains the browser code-delivery trust limit and what SRI does and does not protect against
