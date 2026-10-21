# Spec Delta

## MODIFIED Requirements

### Requirement: Three-pane layout
When unlocked on a viewport at least 900 CSS pixels wide, the app SHALL show three panes side by side: the sidebar, the note list for the sidebar selection, and the editor for the selected note. On narrower viewports it SHALL use the stacked layout instead. On first unlock the sidebar selection SHALL be Notes.

#### Scenario: Initial layout
- **WHEN** the vault is first opened on a viewport at least 900 CSS pixels wide
- **THEN** the sidebar, a note list showing Notes, and the editor pane are visible

#### Scenario: Tablet in landscape
- **WHEN** the app runs on a viewport 1180 CSS pixels wide
- **THEN** the sidebar, note list, and editor are shown side by side

## ADDED Requirements

### Requirement: Stacked layout on narrow viewports
On viewports narrower than 900 CSS pixels, the app SHALL show one of sidebar, note list, or editor at a time, filling the screen. This SHALL apply on every client: desktop, browser, and mobile.

#### Scenario: Phone in portrait
- **WHEN** the app runs on a viewport 390 CSS pixels wide
- **THEN** exactly one of sidebar, note list, or editor fills the screen

#### Scenario: Narrow desktop window
- **WHEN** a desktop window is resized below 900 CSS pixels wide
- **THEN** the app switches to the stacked layout

### Requirement: Layout changes keep context
Switching between the three-pane and stacked layouts, for example by rotating a tablet or resizing a window, SHALL keep the selected tag or view, the open note, the caret position, and unsaved editor state. In the stacked layout the visible pane SHALL be the editor if a note is open, otherwise the note list.

#### Scenario: Rotate while editing
- **WHEN** a note is open in the three-pane layout with the caret mid-sentence and the tablet is rotated to a narrow portrait width
- **THEN** the stacked layout shows the editor with the same note and caret position

#### Scenario: Widen with no note open
- **WHEN** the stacked layout shows the note list for `#work` and the viewport widens past the breakpoint
- **THEN** the three-pane layout shows `#work` selected in the sidebar and its notes in the list

### Requirement: Stacked navigation
In the stacked layout the order SHALL be sidebar, then note list, then editor. Choosing a tag or view SHALL show its note list, opening a note SHALL show the editor, and a visible back control SHALL move one level up. The app SHALL start on the note list of the Notes view.

#### Scenario: Open a note from a tag
- **WHEN** the user taps `#work` in the sidebar and then a note in the list
- **THEN** the note list for `#work` is shown, then the editor with that note

#### Scenario: Back control in the editor
- **WHEN** the editor is shown and the user taps the back control
- **THEN** the note list it came from is shown

#### Scenario: Start screen
- **WHEN** the app is opened and unlocked in the stacked layout
- **THEN** the note list of the Notes view is shown, with a back control leading to the sidebar

### Requirement: Stacked navigation follows browser history
Each step down the stack SHALL add a history entry, and history back SHALL move one level up, so browser back buttons and platform back gestures behave the same as the back control. Leaving the editor by any of these SHALL keep every edit.

#### Scenario: Browser back on a phone
- **WHEN** the browser client is open on a phone with a note in the editor and the user presses the browser's back button
- **THEN** the note list is shown and the note's edits are kept

### Requirement: Actions reachable in the stacked layout
Every action available in the three-pane layout SHALL remain reachable in the stacked layout: search and the sync indicator from the note list, note actions (pin, archive, trash, restore, delete permanently) from the editor and from a long press on a note in the list, tag rename and move from a long press on a tag, and settings from the sidebar.

#### Scenario: Trash from the list
- **WHEN** the user long-presses a note in the stacked note list and chooses Trash
- **THEN** the note moves to Trash

#### Scenario: Rename a tag on a phone
- **WHEN** the user long-presses `#work` in the stacked sidebar and renames it to `#job`
- **THEN** every note carrying `#work` now carries `#job`
