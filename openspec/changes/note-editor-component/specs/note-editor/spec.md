# Spec Delta

## Purpose

Defines the shared markdown note editor used by every client: how it displays and edits a note's body, how it merges concurrent edits from other devices live, and its optional per-device vim mode.

## ADDED Requirements

### Requirement: Edits the note's document
The editor SHALL show the current body of the open note and SHALL apply every change the user makes to that note's CRDT document, not to a separate copy of the text. Opening another note SHALL first hand over any edits not yet delivered for the previous note.

#### Scenario: Opening a note shows its body
- **WHEN** a note whose body is `# Groceries\n- eggs` is opened in the editor
- **THEN** the editor shows `# Groceries\n- eggs`

#### Scenario: Typing changes the document
- **WHEN** the user types ` and milk` at the end of the body
- **THEN** the note's document body becomes `# Groceries\n- eggs and milk`

#### Scenario: Switching notes keeps pending edits
- **WHEN** the user types in note A and immediately opens note B
- **THEN** note A's document contains the typed text and the editor shows note B's body

### Requirement: Edits are delivered without a save action
Local edits SHALL be delivered to the note's document automatically, without any save action. Edits SHALL be delivered within one second of the last keystroke, and immediately when the note is closed, another note is opened, or the editor is hidden or unloaded.

#### Scenario: Idle delivery
- **WHEN** the user types a word and stops typing
- **THEN** within one second the note's document contains the word

#### Scenario: Closing delivers immediately
- **WHEN** the user types and closes the note before the idle delay has passed
- **THEN** the note's document contains the typed text when closing completes

### Requirement: Failed delivery does not lose edits
If delivering local edits to the note's document fails, the editor SHALL keep those edits, SHALL report the failure to the hosting app, and SHALL include them in the next delivery attempt.

#### Scenario: Delivery fails then succeeds
- **WHEN** a delivery of typed text fails and a later delivery succeeds
- **THEN** the note's document contains all the typed text, and the host was told about the failure

### Requirement: Remote edits merge live
While a note is open, edits to the same note made elsewhere (another device or a core-side change such as a tag rename) SHALL appear in the editor without reopening the note, merged with the user's own edits so that neither is lost.

#### Scenario: Remote edit appears
- **WHEN** a note with body `one\ntwo` is open and another device changes `two` to `TWO`
- **THEN** the editor shows `one\nTWO` without the note being reopened

#### Scenario: Concurrent local and remote edits
- **WHEN** the user changes `one` to `ONE` while another device concurrently changes `two` to `TWO`
- **THEN** after both edits are exchanged, the editor and the note's document both have body `ONE\nTWO`

### Requirement: Cursor survives remote edits
A remote edit SHALL NOT move the user's cursor or selection relative to the text around it, and SHALL NOT take focus or scroll the cursor out of view.

#### Scenario: Remote insert before the cursor
- **WHEN** the cursor is just after `world` in `hello world` and a remote edit inserts `big ` before `world`
- **THEN** the body is `hello big world` and the cursor is still just after `world`

#### Scenario: Remote edit after the selection
- **WHEN** the user has `hello` selected in `hello world` and a remote edit appends `!`
- **THEN** `hello` is still selected

### Requirement: Undo is limited to local edits
Undo and redo SHALL reverse and reapply only edits the user made in this editor, never edits that arrived from elsewhere. Undo history SHALL be per note and SHALL start empty when a note is opened.

#### Scenario: Undo skips remote edits
- **WHEN** the user types `A`, a remote edit then inserts `B` elsewhere, and the user undoes
- **THEN** `A` is removed and `B` remains

#### Scenario: Redo
- **WHEN** the user types `A`, undoes, then redoes
- **THEN** `A` is back

#### Scenario: Fresh history on open
- **WHEN** a note is opened and the user immediately undoes
- **THEN** the body is unchanged

### Requirement: Markdown highlighting
The editor SHALL highlight markdown structure in the source text, including headings, emphasis, strong text, inline code, fenced code blocks, links, blockquotes, and list markers. The markdown text itself SHALL remain exactly what the user typed.

#### Scenario: Heading and emphasis are styled
- **WHEN** the body is `# Title\nsome *emphasis*`
- **THEN** `# Title` is styled as a heading and `*emphasis*` as emphasis, and the body text is unchanged

### Requirement: Tag highlighting matches the tag rules
The editor SHALL highlight exactly the text the `tags` capability treats as a tag occurrence, from the opening `#` through the name and any closing `#`, and nothing else. Text inside code, headings markers, URL fragments, and numeric-only names SHALL NOT be highlighted as tags.

#### Scenario: Single-word and multi-word tags
- **WHEN** the body is `Ideas for #side project# and #work/acme.`
- **THEN** `#side project#` and `#work/acme` are highlighted as tags, and the trailing `.` is not

#### Scenario: Non-tags are not highlighted
- **WHEN** the body is ``# Heading\nC# and #42 and `#code` and https://x.io/#frag``
- **THEN** nothing is highlighted as a tag

#### Scenario: Highlighting follows edits
- **WHEN** the user types `#todo` followed by a space
- **THEN** `#todo` is highlighted as a tag

### Requirement: Find and replace
The editor SHALL provide find and replace within the open note, including case-sensitive, whole-word, and regular-expression matching, replace-one, and replace-all. A replacement SHALL be an ordinary local edit.

#### Scenario: Replace all
- **WHEN** the body is `cat and cat` and the user replaces all `cat` with `dog`
- **THEN** the body is `dog and dog`, and one undo restores `cat and cat`

### Requirement: Soft wrap
The editor SHALL wrap long lines to the editor width for display without inserting line breaks into the body.

#### Scenario: Long line
- **WHEN** a single line longer than the editor width is shown
- **THEN** it wraps visually and the body still contains no added line break

### Requirement: List continuation
Pressing Enter at the end of a list item SHALL start a new item with the same marker and indentation; for an ordered list the number SHALL increment, and for a task item the new item SHALL be an unchecked task. Pressing Enter on an empty item SHALL end the list instead.

#### Scenario: Bullet list
- **WHEN** the cursor is at the end of `- eggs` and the user presses Enter
- **THEN** a new line `- ` follows with the cursor after the marker

#### Scenario: Ordered list
- **WHEN** the cursor is at the end of `1. first` and the user presses Enter
- **THEN** a new line `2. ` follows

#### Scenario: Task list
- **WHEN** the cursor is at the end of `- [x] done` and the user presses Enter
- **THEN** a new line `- [ ] ` follows

#### Scenario: Empty item ends the list
- **WHEN** the cursor is on an empty `- ` item and the user presses Enter
- **THEN** the marker is removed and the list ends

### Requirement: Optional vim mode
The editor SHALL offer vim key bindings (normal, insert, and visual modes, motions, operators, and registers) as an option that is off unless enabled. When off, the editor SHALL use standard key bindings.

#### Scenario: Vim off by default
- **WHEN** a device that has never changed the setting opens a note and the user presses `d`
- **THEN** the letter `d` is inserted

#### Scenario: Vim motions when on
- **WHEN** vim mode is on, the editor is in normal mode on the line `delete me`, and the user types `dd`
- **THEN** the line is deleted

### Requirement: Vim mode toggles without reloading
Turning vim mode on or off SHALL take effect immediately in the open editor, without reopening the note and without changing the body, cursor position, or undo history.

#### Scenario: Toggle while editing
- **WHEN** the user has typed text in an open note and turns vim mode on
- **THEN** the body and cursor are unchanged, the editor is in vim normal mode, and undo still removes the typed text

### Requirement: Vim mode is a per-device setting
The vim mode setting SHALL be stored on the current device through the platform's device-local settings, SHALL persist across restarts of the app on that device, and SHALL NOT be synced to other devices.

#### Scenario: Persists on the device
- **WHEN** the user turns vim mode on and restarts the app on the same device
- **THEN** vim mode is on

#### Scenario: Not synced
- **WHEN** the user turns vim mode on on one device
- **THEN** vim mode on the user's other devices is unchanged

### Requirement: Device-local preferences
The editor's preferences SHALL be read from and written to a device-local, non-sensitive settings store that the hosting platform provides and that is never synced. A preference that is missing or cannot be read SHALL take its default value. A preference that cannot be saved SHALL still apply for the rest of the session, and the failure SHALL be reported to the hosting app.

#### Scenario: Unreadable preference
- **WHEN** the settings store fails when the editor reads the vim mode preference
- **THEN** the editor opens with vim mode off

#### Scenario: Failed save
- **WHEN** the user turns vim mode on and saving the preference fails
- **THEN** vim mode stays on until the app is restarted, and the host is told about the failure

### Requirement: Vim write command
In vim mode, `:w` SHALL deliver any pending local edits immediately and show a brief confirmation that the note is saved, and SHALL NOT report an error. Commands that would close or quit the editor SHALL NOT close the note or the app.

#### Scenario: Write
- **WHEN** vim mode is on, the user has just typed text, and the user runs `:w`
- **THEN** the note's document contains the typed text and a saved confirmation is shown

#### Scenario: Quit does nothing destructive
- **WHEN** vim mode is on and the user runs `:q`
- **THEN** the note stays open and its body is unchanged

### Requirement: Read-only mode
The hosting app SHALL be able to open a note read-only. A read-only editor SHALL show the note and apply remote edits, and SHALL NOT change the note in response to typing, replace, undo, or vim commands.

#### Scenario: Read-only note
- **WHEN** a note is open read-only and the user types, runs replace-all, or runs vim `dd`
- **THEN** the body is unchanged and nothing is delivered to the note's document

#### Scenario: Remote edits still appear while read-only
- **WHEN** a note is open read-only and another device edits it
- **THEN** the editor shows the edit

### Requirement: Platform independence
The editor SHALL behave the same in the desktop app, the browser client, and the mobile app. It SHALL depend only on the note document bridge and device-settings interfaces that each platform provides, and SHALL NOT depend on how the core is reached.

#### Scenario: Reference bridge
- **WHEN** the editor is run against the in-memory reference bridge with no platform core present
- **THEN** opening, editing, merging remote edits, and undo behave as specified above

### Requirement: Works under a strict content security policy
The editor SHALL work under a content security policy that allows styles and scripts only from the app's own origin, without inline style attributes or inline style elements being allowed.

#### Scenario: Strict policy
- **WHEN** the browser client is served with `style-src 'self'` and no `'unsafe-inline'`
- **THEN** the editor's styles and highlighting are applied, and opening, typing, find/replace, and vim mode work as specified
