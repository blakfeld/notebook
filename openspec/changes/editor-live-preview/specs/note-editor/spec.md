# Spec Delta

## ADDED Requirements

### Requirement: Live preview is the default view
The editor SHALL render notes in live preview by default: markdown is formatted in place while the note text stays editable markdown. Live preview SHALL only change presentation. The note's text, and therefore what is stored, synced, copied, searched, and exported, SHALL be exactly the markdown the user typed.

#### Scenario: Formatting without altering text
- **WHEN** the user types `Some **bold** text` and moves the cursor to another line
- **THEN** the line shows `Some bold text` with `bold` in strong weight, and the note body is still `Some **bold** text`

#### Scenario: Copy yields markdown
- **WHEN** the user selects a rendered line containing a link and copies it
- **THEN** the clipboard contains the markdown source, including the link URL

### Requirement: Syntax hidden away from the selection
On every line that no selection range touches, live preview SHALL hide markdown syntax markers: emphasis and strong markers, strikethrough markers, inline code backticks, ATX heading markers and the space after them, blockquote markers, link brackets and URLs, autolink angle brackets, and fenced code block fence lines.

#### Scenario: Heading marker hidden
- **WHEN** a line is `## Plans` and the cursor is on another line
- **THEN** the line shows `Plans` styled as a level-2 heading with no `##` visible

#### Scenario: Link URL hidden
- **WHEN** a line is `See [the docs](https://example.com/docs)` and the cursor is on another line
- **THEN** the line shows `See the docs` with `the docs` styled as a link

### Requirement: Syntax revealed around the selection
Live preview SHALL show the full markdown source, with syntax markers visible and de-emphasized, on every line touched by any selection range, including the main cursor, additional cursors, and multi-line selections. Formatting of the text between markers SHALL remain applied while revealed. Reveal SHALL follow only the selection, never changes applied to the document from elsewhere.

#### Scenario: Cursor enters a line
- **WHEN** the cursor moves onto a line showing `Some bold text` whose source is `Some **bold** text`
- **THEN** the line shows `Some **bold** text` with the `**` markers visible and `bold` still in strong weight

#### Scenario: Multi-line selection
- **WHEN** a selection spans three lines containing heading and emphasis syntax
- **THEN** all three lines show their markdown syntax, and lines outside the selection stay rendered

#### Scenario: Cursor leaves a line
- **WHEN** the cursor moves from a line with `[link](https://a.example)` to the next line
- **THEN** the previous line hides the brackets and URL again

### Requirement: Rendered block and inline elements
Live preview SHALL render ATX and setext headings level 1 to 6 with distinct, decreasing type sizes, emphasis in italics, strong text in bold, strikethrough with a line through, inline code in monospace on a tinted background, blockquotes with an indented bar, unordered list markers as bullets, and thematic breaks as a horizontal rule.

#### Scenario: Heading sizes
- **WHEN** a note contains `# One`, `## Two`, and `### Three` and the cursor is elsewhere
- **THEN** `One` renders larger than `Two`, and `Two` larger than `Three`

#### Scenario: Thematic break
- **WHEN** a line is `---` preceded by a blank line and the cursor is elsewhere
- **THEN** the line renders as a horizontal rule

#### Scenario: Bullet marker
- **WHEN** a line is `- milk` and the cursor is elsewhere
- **THEN** the line shows a bullet glyph followed by `milk`

### Requirement: Unrendered markdown stays as source
Constructs live preview does not render, including tables, images, reference-style links, footnotes, and inline or block HTML, SHALL be shown as their markdown source text. Rendering SHALL never interpret note text as HTML.

#### Scenario: HTML stays text
- **WHEN** a note contains `<b onclick="x()">hi</b>`
- **THEN** the editor shows that text literally, creates no `b` element from it, and runs no script

### Requirement: No remote content
Live preview SHALL NOT load any resource referenced by note text, such as images, iframes, stylesheets, or link previews, so opening a note never makes a network request on its behalf.

#### Scenario: Image reference not fetched
- **WHEN** a note containing `![x](https://tracker.example/pixel.png)` is opened in live preview
- **THEN** the image markdown is shown as text and no request is made to `tracker.example`

### Requirement: Opening links
A plain mouse click on link text SHALL place the cursor for editing. Mod-click (Cmd on macOS, Ctrl elsewhere) on a link SHALL open the URL in the system browser, outside the app. Only `http`, `https`, and `mailto` URLs SHALL open; any other scheme SHALL do nothing.

#### Scenario: Plain click edits
- **WHEN** the user clicks link text without a modifier
- **THEN** the cursor is placed in the link and no browser opens

#### Scenario: Mod-click opens externally
- **WHEN** the user Mod-clicks a link to `https://example.com`
- **THEN** `https://example.com` opens in the system browser and the app stays on the note

#### Scenario: Disallowed scheme
- **WHEN** the user Mod-clicks a link whose URL is `javascript:alert(1)` or `file:///etc/passwd`
- **THEN** nothing is opened and no script runs

### Requirement: Task list checkboxes
On lines no selection touches, live preview SHALL render a task list item's `[ ]` or `[x]` marker (either case of `x`) as a checkbox showing its state. Activating the checkbox by click or tap SHALL toggle the marker between `[ ]` and `[x]` as a normal local edit, without moving the cursor.

#### Scenario: Check a task
- **WHEN** the line `- [ ] buy milk` renders a checkbox and the user clicks it
- **THEN** the note text becomes `- [x] buy milk`, the checkbox shows checked, and the cursor stays where it was

#### Scenario: Toggle is undoable and syncs
- **WHEN** the user checks a task and then undoes
- **THEN** the line returns to `- [ ] buy milk`, and both edits reach other devices like any typed edit

#### Scenario: Revealed on the active line
- **WHEN** the cursor is on the line `- [x] done`
- **THEN** the line shows its `- [x]` source text instead of a checkbox

### Requirement: Toggle task from the keyboard
The editor SHALL provide a keyboard command that toggles the task marker on every line touched by the selection. On a list item without a task marker, the command SHALL add `[ ] ` after the list marker. The command SHALL be available with and without vim mode.

#### Scenario: Toggle current line
- **WHEN** the cursor is on `- [ ] call Sam` and the user runs the toggle-task command
- **THEN** the line becomes `- [x] call Sam`

#### Scenario: Make a list item a task
- **WHEN** the cursor is on `- call Sam` and the user runs the toggle-task command
- **THEN** the line becomes `- [ ] call Sam`

### Requirement: Tag pills
Live preview SHALL render each tag as a pill-styled span covering its full text, including the `#` and, for multi-word tags, the closing `#`. Which text is a tag SHALL follow the `tags` capability rules, so text in code, headings markers, URL fragments, and numeric names never render as pills. Pills SHALL remain ordinary editable text.

#### Scenario: Single and multi-word tags
- **WHEN** a line is `Plan #work/acme and #side project# today` and the cursor is elsewhere
- **THEN** `#work/acme` and `#side project#` each render as a pill and `today` does not

#### Scenario: Not tags
- **WHEN** a line is ``Fixed #42 in `#define` at https://a.example/#top``
- **THEN** no part of the line renders as a pill

#### Scenario: Edit inside a pill
- **WHEN** the cursor is placed inside the pill `#work` and the user types `s`
- **THEN** the text becomes `#works` and renders as a single pill

### Requirement: Fenced code blocks
Live preview SHALL render fenced code block content in a monospace font on a tinted block background, with syntax highlighting for the language named in the info string when that language is known. An unknown or missing language SHALL render as plain monospace text. Text inside code blocks SHALL NOT receive any other live-preview formatting.

#### Scenario: Known language
- **WHEN** a note contains a fenced block with info string `rust` and the cursor is outside it
- **THEN** the fence lines are hidden and the Rust code is shown in monospace with syntax highlighting

#### Scenario: Markdown inside code is literal
- **WHEN** a fenced block contains `**not bold** #nottag`
- **THEN** the text is shown literally in monospace with no bold and no pill

#### Scenario: Unknown language
- **WHEN** a fenced block has info string `nosuchlang`
- **THEN** its content renders as plain monospace text without error

### Requirement: Live preview with vim mode
When vim mode is on, vim motions, operators, and text objects SHALL act on the underlying markdown text, including characters hidden by live preview. Syntax reveal SHALL follow the vim cursor and the vim visual selection exactly as it follows a normal selection.

#### Scenario: Motion counts hidden characters
- **WHEN** the vim cursor is at the start of a line whose source is `**bold** word` and the user presses `w`
- **THEN** the cursor lands on the same character of the source text that it would in source view

#### Scenario: Visual line selection reveals
- **WHEN** the user presses `V` and then `j` in vim normal mode
- **THEN** both selected lines show their markdown syntax

### Requirement: Readable typography and measure
In live preview, prose SHALL use a proportional font and code a monospace font, both bundled with the app so no font is fetched from another origin. Lines of text SHALL wrap at a maximum measure of 72 characters of body text, centered in the editor pane when wider, and the editor SHALL keep at least 16px of horizontal padding at any width.

#### Scenario: Wide window
- **WHEN** the editor pane is wider than the measure
- **THEN** text wraps at the measure and the text column is centered

#### Scenario: Narrow window
- **WHEN** the editor pane is 360px wide
- **THEN** text wraps to the pane with at least 16px padding on each side and no horizontal scrolling for prose

### Requirement: Focus mode
The editor SHALL offer a focus mode, off by default. When on, every top-level markdown block except the one containing the main cursor (a paragraph, heading, list item, blockquote, or code block) SHALL be dimmed, and the undimmed block SHALL follow the cursor as it moves.

#### Scenario: Dim other paragraphs
- **WHEN** focus mode is on and the cursor is in the second of three paragraphs
- **THEN** the first and third paragraphs are dimmed and the second is not

#### Scenario: Follow the cursor
- **WHEN** focus mode is on and the cursor moves into the third paragraph
- **THEN** the third paragraph is undimmed and the second is dimmed

### Requirement: Cursor and active line polish
The editor SHALL animate cursor movement smoothly and mark the line holding the main cursor with a subtle background. When the OS or browser requests reduced motion, cursor movement SHALL NOT be animated.

#### Scenario: Reduced motion
- **WHEN** the system requests reduced motion and the cursor moves
- **THEN** the cursor jumps to its new position with no animation

### Requirement: Raw source view
The editor SHALL offer a source view in which all live-preview rendering is off and the note shows as highlighted markdown source, as defined by the base editor. Switching between live preview and source view SHALL NOT reload the editor and SHALL keep the cursor, selection, scroll position, undo history, and vim state.

#### Scenario: Switch to source
- **WHEN** live preview shows `Plans` for `## Plans` and the user switches to source view
- **THEN** every line shows its full markdown source, including `## Plans`, and the cursor stays at the same text position

#### Scenario: Undo across a switch
- **WHEN** the user types a word, switches to source view, and undoes
- **THEN** the typed word is removed

### Requirement: View preferences are per device
Source view and focus mode SHALL each be togglable by a keyboard command and from the editor's view options, and each SHALL be stored as a device-local, non-sensitive preference that is not stored in the vault or its backups. Both SHALL default to off, and the chosen view SHALL apply to every note opened on that device.

#### Scenario: Preference persists
- **WHEN** the user turns on source view, closes the note, and opens another note
- **THEN** the other note opens in source view

#### Scenario: Not synced
- **WHEN** the user turns on focus mode and a backup of the notebook is restored on another computer
- **THEN** that computer still has focus mode off
