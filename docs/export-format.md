# Markdown export format

"Export all notes" writes every note as a plain markdown file. With end-to-end
encryption this is the user's own backup: the files are **not encrypted**, so
anyone who can read them can read the notes.

The export is made on the client from the notes it holds decrypted, works
offline, and is deterministic: the same notes exported at the same time give
the same files byte for byte. This document is the contract a future import
relies on. The implementation is `notebook-core`'s `export` module.

## Layout

All files sit directly in one folder, with no subfolders, whatever the tags:

```
notebook-export-20261021-140305/
  Grocery list.md
  Ideas.md
  Ideas 2.md
  Untitled.md
```

The folder is named `notebook-export-YYYYMMDD-HHMMSS` after the export time in
UTC.

- **Desktop** writes the folder inside a folder the user picks. An existing
  folder is never overwritten: the export takes the smallest free `-N` suffix
  (`notebook-export-20261021-140305-2`). The folder appears complete or not at
  all; it is built as `notebook-export-incomplete-<random>` and renamed when
  done, so a folder with that name is an interrupted export and can be
  deleted. On macOS and Linux the folder is readable only by you. Each file's
  modification time is the note's updated time where the filesystem supports
  it.
- **Browser and mobile** produce `notebook-export-YYYYMMDD-HHMMSS.zip`, whose
  only top-level entry is that folder. Entries are deflated, in file-name
  order, with UTF-8 names. Entry times are the notes' updated times written
  as UTC (zip stores no time zone, so unzip tools may show them shifted) and
  clamped to 1980–2107.

## Which notes

Every note that is not trashed, both active and archived. With the
include-trashed option, trashed notes too. One file per note.

## File contents

Each file is UTF-8: a frontmatter block, then the body exactly as stored.
Nothing is added to or removed from the body; line endings, trailing
whitespace, and inline tags (`#work/acme`, `#side project#`) are kept as
written.

```
---
id: 01JBX3Z7Q8M4N5P6R7S8T9V0W1
created: 2024-10-21T14:03:05.123Z
updated: 2024-10-22T14:03:05.000Z
pinned: true
archived: true
trashed: false
---
# Ideas

Ideas for #side project# and #work/acme
```

The frontmatter always has exactly these six keys, in this order, unquoted,
each line ending in `\n`:

| Key        | Value                                                       |
|------------|-------------------------------------------------------------|
| `id`       | The note's stable identifier, a 26-character ULID           |
| `created`  | When the note was created, UTC, `YYYY-MM-DDTHH:MM:SS.sssZ`  |
| `updated`  | When the body last changed, same format                     |
| `pinned`   | `true` or `false`                                           |
| `archived` | `true` or `false`                                           |
| `trashed`  | `true` or `false`                                           |

Tags are not repeated in the frontmatter; the body is their only source.

### Reading a file back

Remove the opening `---\n` line and everything up to and including the next
`---\n` line. What remains is the body, exactly. This works even when the body
itself starts with `---`, because no frontmatter line is ever `---`. A note
with an empty body is the frontmatter block alone.

Use `id`, not the file name, as the note's identity: names can change between
exports when notes are added.

## File names

A file is named after the note's title (its first non-blank line, without
heading markers), made safe for Windows, macOS, and Linux, plus `.md`:

1. `/ \ : * ? " < > | # ^ [ ]` and control characters become spaces.
2. Runs of whitespace become one space.
3. Leading dots and spaces, and trailing dots and spaces, are removed.
4. A name longer than 120 UTF-8 bytes is cut at the last character boundary
   within 120 bytes, then step 3 is applied again.
5. An empty name becomes `Untitled`.
6. A Windows reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`,
   `LPT1`–`LPT9`, any case) gets `_` appended: `con` → `con_`.

Case and all other Unicode text are kept as typed, without rewriting the
normalization form:
`Q3: plan/budget?` → `Q3 plan budget.md`, `#work standup notes` →
`work standup notes.md`, `Café 日本語` → `Café 日本語.md`.

### Collisions

Notes are named in ascending identifier order, which is creation order. A
note takes its name unless that name is already taken, compared after Unicode
case folding and NFC normalization, as macOS and Windows compare names
(composed and decomposed `é` are the same, and so are `Straße` and
`STRASSE`); then it takes the name plus ` N` for the smallest N ≥ 2 that is
free. Notes titled `Ideas`, `Ideas 2`, `Ideas` are named `Ideas.md`,
`Ideas 2.md`, `Ideas 3.md`; `todo` and `TODO` become `todo.md` and
`TODO 2.md`.

## Opening in other apps

The folder opens as an Obsidian vault or in any markdown editor. Obsidian
reads `created` and `updated` as date-time properties and nested tags like
`#work/acme` as tags. It does not understand Notebook's multi-word tags
(`#side project#`): it sees `#side` followed by the text `project#`. Bodies
are kept verbatim so that Notebook (and Bear) read them back exactly.

## Not yet included

Attachments. When they exist they will go in an `attachments/` folder with
relative links. If the format changes, a `format` frontmatter key will be
added; files without it are this version.
