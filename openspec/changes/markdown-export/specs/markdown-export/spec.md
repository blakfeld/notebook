# Spec Delta

## Purpose

Defines how a client exports the user's decrypted notes as portable markdown files: which notes are included, the file format, file naming, and the folder and zip outputs, so the export can serve as the user's own backup.

## ADDED Requirements

### Requirement: Client-side export
Export SHALL be produced entirely on the client from the notes it holds in decrypted form, including local edits not yet synced. It SHALL NOT contact the server or require a network connection, and it SHALL be available only while the vault is unlocked.

#### Scenario: Export while offline
- **WHEN** the client has no network connection and the user exports
- **THEN** the export completes and contains every selected note

#### Scenario: Unsynced edits are included
- **WHEN** a note has a local edit that the server has not yet acknowledged and the user exports
- **THEN** the exported file contains the edited body

#### Scenario: Locked vault
- **WHEN** the vault is locked
- **THEN** export is not available until the user unlocks it

### Requirement: Note selection
By default an export SHALL include every note that is not trashed, both active and archived. When the include-trashed option is set, it SHALL also include trashed notes. Each included note SHALL produce exactly one file.

#### Scenario: Default selection
- **WHEN** the notes are A (active), B (archived), and C (trashed) and the user exports with default options
- **THEN** the export contains files for A and B only

#### Scenario: Including trashed notes
- **WHEN** the same notes are exported with include-trashed set
- **THEN** the export contains files for A, B, and C

#### Scenario: Empty vault
- **WHEN** no notes are selected
- **THEN** the export succeeds and contains an empty export folder

### Requirement: Flat layout in one export folder
All note files SHALL be placed directly inside a single export folder with no subfolders, regardless of tags. The export folder SHALL be named `notebook-export-YYYYMMDD-HHMMSS` from the export time in UTC.

#### Scenario: Nested tags do not create folders
- **WHEN** a note carrying `#work/acme` is exported
- **THEN** its file sits directly in the export folder

#### Scenario: Export folder name
- **WHEN** the user exports at 2026-10-21 14:03:05 UTC
- **THEN** the export folder is named `notebook-export-20261021-140305`

### Requirement: File contents
Each file SHALL be UTF-8 and consist of a YAML frontmatter block followed immediately by the note body exactly as stored. The frontmatter SHALL be `---` on its own line, the metadata lines, then `---` on its own line, all ending in `\n`. Nothing SHALL be added to or removed from the body.

#### Scenario: Body preserved byte for byte
- **WHEN** a note whose body has CRLF line endings, trailing whitespace, and non-ASCII text is exported
- **THEN** the file contents after the closing `---\n` line equal the body exactly

#### Scenario: Inline tags kept as written
- **WHEN** a note with body `Ideas for #side project# and #work/acme` is exported
- **THEN** the file's body text is `Ideas for #side project# and #work/acme`

#### Scenario: Empty body
- **WHEN** a note with an empty body is exported
- **THEN** the file consists of the frontmatter block only

#### Scenario: Body recoverable for import
- **WHEN** an exported file is read back by removing everything up to and including the first closing `---\n` line after the opening line
- **THEN** the remaining text equals the note body, even when the body itself starts with `---`

### Requirement: Frontmatter metadata
The frontmatter SHALL contain exactly these keys in this order: `id` (the note's ULID text), `created`, `updated` (UTC timestamps formatted `YYYY-MM-DDTHH:MM:SS.sssZ`), `pinned`, `archived`, and `trashed` (`true` or `false`). Values SHALL be unquoted. Tags SHALL NOT be repeated in the frontmatter.

#### Scenario: Frontmatter for an archived, pinned note
- **WHEN** note `01JBX3Z7Q8M4N5P6R7S8T9V0W1`, created at 1729519385123 ms and updated at 1729605785000 ms, pinned and archived, is exported
- **THEN** its frontmatter is `---\nid: 01JBX3Z7Q8M4N5P6R7S8T9V0W1\ncreated: 2024-10-21T14:03:05.123Z\nupdated: 2024-10-22T14:03:05.000Z\npinned: true\narchived: true\ntrashed: false\n---\n`

#### Scenario: Trashed note metadata
- **WHEN** a trashed note is exported with include-trashed set
- **THEN** its frontmatter has `trashed: true`

### Requirement: File names from titles
A file's name SHALL be the note's title made filesystem-safe, plus `.md`. Characters `/ \ : * ? " < > | # ^ [ ]` and control characters SHALL become spaces, whitespace runs SHALL collapse to one space, and leading dots, leading spaces, and trailing dots or spaces SHALL be removed. Case and other Unicode text SHALL be kept.

#### Scenario: Plain title
- **WHEN** a note titled `Grocery list` is exported
- **THEN** its file is named `Grocery list.md`

#### Scenario: Unsafe characters
- **WHEN** a note titled `Q3: plan/budget?` is exported
- **THEN** its file is named `Q3 plan budget.md`

#### Scenario: Tag in the title
- **WHEN** a note titled `#work standup notes` is exported
- **THEN** its file is named `work standup notes.md`

#### Scenario: Non-ASCII title
- **WHEN** a note titled `Café 日本語` is exported
- **THEN** its file is named `Café 日本語.md`

### Requirement: Fallback and length limits for names
A name that is empty after sanitizing SHALL become `Untitled`. A name equal to a Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, compared case-insensitively) SHALL have `_` appended. A name longer than 120 UTF-8 bytes SHALL be cut at the last character boundary within 120 bytes and re-trimmed.

#### Scenario: Empty title
- **WHEN** a note with a blank body is exported
- **THEN** its file is named `Untitled.md`

#### Scenario: Reserved device name
- **WHEN** a note titled `con` is exported
- **THEN** its file is named `con_.md`

#### Scenario: Long title
- **WHEN** a note's title is 300 bytes long
- **THEN** its file name before `.md` is at most 120 bytes and is valid UTF-8

### Requirement: Deterministic collision suffixes
Notes SHALL be named in ascending note-identifier order. A note SHALL take its sanitized name unless that name is already taken, compared after Unicode case folding and NFC normalization (the name itself keeps its form); otherwise it SHALL take the name followed by ` N` for the smallest N ≥ 2 that is not taken. The same set of notes SHALL always produce the same names.

#### Scenario: Two notes with the same title
- **WHEN** notes A and B, with A's identifier smaller, are both titled `Ideas`
- **THEN** A is `Ideas.md` and B is `Ideas 2.md`

#### Scenario: Case-insensitive collision
- **WHEN** notes titled `todo` and `TODO` are exported
- **THEN** their file names differ by more than letter case

#### Scenario: Suffix does not reuse an existing name
- **WHEN** notes in identifier order are titled `Ideas`, `Ideas 2`, and `Ideas`
- **THEN** they are named `Ideas.md`, `Ideas 2.md`, and `Ideas 3.md`

#### Scenario: Repeat export is identical
- **WHEN** the same notes are exported twice
- **THEN** both exports have the same file names with the same contents

### Requirement: Folder output
On native clients, export SHALL write the export folder inside a parent folder the user chooses. It SHALL NOT overwrite existing files: if a folder with the export name exists, the name SHALL get the smallest `-N` suffix (N ≥ 2) that is free. Each file's modification time SHALL be set to the note's updated time where the filesystem supports it.

#### Scenario: Export into a chosen folder
- **WHEN** the user picks `~/Backups` and exports at 2026-10-21 14:03:05 UTC
- **THEN** `~/Backups/notebook-export-20261021-140305/` contains one `.md` file per selected note

#### Scenario: Existing folder is not overwritten
- **WHEN** `notebook-export-20261021-140305` already exists in the chosen folder
- **THEN** the export is written to `notebook-export-20261021-140305-2` and the existing folder is unchanged

#### Scenario: File times match notes
- **WHEN** a note last updated at 2024-10-22T14:03:05Z is exported
- **THEN** its file's modification time is 2024-10-22T14:03:05Z

### Requirement: Folder output is all-or-nothing
If writing any file fails, the folder export SHALL fail with an error and SHALL leave no export folder or partially written files behind.

#### Scenario: Write failure
- **WHEN** the disk fills up partway through an export
- **THEN** the user sees an error and no export folder remains in the chosen location

### Requirement: Zip output
Export SHALL be able to produce a single zip archive, built in memory, named `<export folder name>.zip`, whose only top-level entry is the export folder holding the same files the folder output would write. Entry names SHALL be UTF-8 and entries SHALL appear in file-name order.

#### Scenario: Zip contents match folder output
- **WHEN** the same notes are exported once as a folder and once as a zip at the same export time
- **THEN** extracting the zip yields the same folder name, file names, and file contents

#### Scenario: Non-ASCII names in the zip
- **WHEN** a note titled `Café` is exported as a zip
- **THEN** a standard unzip tool extracts it as `Café.md`

#### Scenario: Deterministic zip
- **WHEN** the same notes are exported as a zip twice with the same export time
- **THEN** the two archives are byte-identical
