# Spec Delta

## Purpose

Defines the note entity every client, the sync layer, and export share: its identity, markdown body, derived title, timestamps, and pinned/archived/trashed lifecycle states.

## ADDED Requirements

### Requirement: Stable note identity
Every note SHALL have an identifier that is a ULID, assigned once when the note is created and never changed afterwards. Its text form SHALL be the 26-character Crockford base32 ULID encoding, and parsing that text form SHALL yield the same identifier.

#### Scenario: Identifier round-trips through text
- **WHEN** a note's identifier is formatted as text and the text is parsed back
- **THEN** the parsed identifier equals the original

#### Scenario: Identifier survives edits and state changes
- **WHEN** a note's body is edited and it is pinned, archived, and trashed
- **THEN** its identifier is unchanged

#### Scenario: Invalid identifier text is rejected
- **WHEN** text that is not a valid 26-character ULID is parsed as a note identifier
- **THEN** parsing fails with an error rather than producing an identifier

#### Scenario: Identifiers order by creation time
- **WHEN** two identifiers are generated with creation times 1000 ms and 2000 ms
- **THEN** the identifier generated at 1000 ms sorts before the other

### Requirement: Markdown body
A note SHALL hold its content as a single markdown text body, stored exactly as written. An empty body SHALL be valid.

#### Scenario: Body is preserved byte for byte
- **WHEN** a note is created with a body containing trailing whitespace, CRLF line endings, and non-ASCII text
- **THEN** reading the body returns exactly the same text

#### Scenario: Empty note
- **WHEN** a note is created with an empty body
- **THEN** creation succeeds and the body is empty

### Requirement: Creation and update timestamps
A note SHALL record a created time and an updated time as UTC instants with millisecond precision. At creation both SHALL equal the creation time supplied by the caller. The note model SHALL NOT read the system clock itself.

#### Scenario: New note timestamps
- **WHEN** a note is created at time T
- **THEN** both its created and updated times are T

#### Scenario: Created time never changes
- **WHEN** a note created at T is edited at a later time T2
- **THEN** its created time is still T

### Requirement: Updated time tracks body changes
Replacing a note's body with different text at time T SHALL set its updated time to T. Replacing the body with identical text, and changing pinned, archived, or trashed state, SHALL NOT change the updated time.

#### Scenario: Body edit bumps updated time
- **WHEN** a note created at T1 has its body changed to different text at T2
- **THEN** its updated time is T2

#### Scenario: No-op edit leaves updated time alone
- **WHEN** a note's body is replaced with the same text at a later time
- **THEN** its updated time is unchanged

#### Scenario: State changes leave updated time alone
- **WHEN** a note is pinned, archived, and trashed at later times
- **THEN** its updated time is unchanged

### Requirement: Derived title
A note's title SHALL be derived from its body on demand and never stored separately. The title SHALL be the first line containing non-whitespace text, with any leading ATX heading markers (`#` to `######` followed by a space) and any ATX closing `#` sequence removed, and surrounding whitespace trimmed.

#### Scenario: Heading as first line
- **WHEN** a note's body is `# Grocery list\n- eggs`
- **THEN** its title is `Grocery list`

#### Scenario: Plain first line
- **WHEN** a note's body is `\n\n  Call the plumber  \nabout the sink`
- **THEN** its title is `Call the plumber`

#### Scenario: Closing heading sequence removed
- **WHEN** a note's body is `## Ideas ##`
- **THEN** its title is `Ideas`

#### Scenario: Tags in the first line are kept
- **WHEN** a note's body is `#work standup notes`
- **THEN** its title is `#work standup notes`

#### Scenario: Title follows body edits
- **WHEN** a note's first line changes from `Draft` to `Final`
- **THEN** its title is `Final`

### Requirement: Empty title
A note whose body contains no non-whitespace text, or whose first non-blank line contains only heading markers, SHALL have an empty title. Choosing a placeholder such as "Untitled" is left to clients.

#### Scenario: Blank body
- **WHEN** a note's body is `  \n\t\n`
- **THEN** its title is empty

#### Scenario: Bare heading marker
- **WHEN** a note's body is `#\nsecond line`
- **THEN** its title is empty

### Requirement: Pinned state
A note SHALL have a pinned flag, false at creation, that can be set and cleared independently of its other states.

#### Scenario: Pin and unpin
- **WHEN** a new note is pinned and then unpinned
- **THEN** it reports pinned after the first step and not pinned after the second

### Requirement: Archived state
A note SHALL have an archived flag, false at creation, that can be set and cleared independently of its other states.

#### Scenario: Archive and unarchive
- **WHEN** a new note is archived and then unarchived
- **THEN** it reports archived after the first step and not archived after the second

### Requirement: Trashed state
A note SHALL have a trashed flag, false at creation. Trashing and restoring SHALL NOT change the pinned or archived flags, so a restored note returns to the state it had before it was trashed. Permanent deletion is outside the note model.

#### Scenario: Restore returns an archived note to the archive
- **WHEN** an archived note is trashed and then restored
- **THEN** it is not trashed and is still archived

#### Scenario: Trashing keeps the pin
- **WHEN** a pinned note is trashed
- **THEN** it is trashed and still reports pinned
