# Spec Delta

## Purpose

Defines Bear-style inline tags: the syntax for writing them in a note body, which text counts as a tag, the nested tag tree derived from notes, the special views, and how renaming or moving a tag rewrites note bodies.

## ADDED Requirements

### Requirement: Single-word tags
A tag SHALL be written as `#` immediately followed by its name, where the `#` is at the start of a line or preceded by whitespace. The name SHALL run until whitespace, `#`, or the end of the line, and SHALL NOT include trailing sentence punctuation (`.`, `,`, `;`, `:`, `!`, `?`, `"`, `'`, `)`, `]`, `}`). A closing `#` directly after the name SHALL be allowed when followed by whitespace, the end of the line, or trailing punctuation.

#### Scenario: Tag in prose
- **WHEN** a body is `Notes for #work today`
- **THEN** the note carries exactly the tag `work`

#### Scenario: Tag at start of line
- **WHEN** a body is `#recipes\nPancakes`
- **THEN** the note carries the tag `recipes`

#### Scenario: Trailing punctuation is not part of the tag
- **WHEN** a body is `Filed under #reading, then #todo.`
- **THEN** the note carries the tags `reading` and `todo`

#### Scenario: Non-ASCII tag names
- **WHEN** a body is `#café and #日本語`
- **THEN** the note carries the tags `café` and `日本語`

#### Scenario: Closing hash on a single-word tag
- **WHEN** a body is `#ideas# for later`
- **THEN** the note carries exactly the tag `ideas`

### Requirement: Text that is not a tag
A `#` SHALL NOT start a tag when it is followed by whitespace or the end of the line, when it is preceded by a non-whitespace character (including a backslash escape), or when the resulting name consists only of digits. These rules SHALL make ATX heading markers, URL fragments, and issue numbers such as `#42` non-tags.

#### Scenario: Heading marker
- **WHEN** a body is `# Weekly review`
- **THEN** the note carries no tags

#### Scenario: Tag inside heading text
- **WHEN** a body is `## Standup #work`
- **THEN** the note carries exactly the tag `work`

#### Scenario: URL fragment
- **WHEN** a body is `See https://example.com/page#section and <https://example.com/#top>`
- **THEN** the note carries no tags

#### Scenario: Mid-word hash
- **WHEN** a body is `C# and F#minor`
- **THEN** the note carries no tags

#### Scenario: Escaped hash
- **WHEN** a body is `\#notatag`
- **THEN** the note carries no tags

#### Scenario: Numeric-only name
- **WHEN** a body is `Fixed in #42 and #2024`
- **THEN** the note carries no tags

#### Scenario: Digits with other characters
- **WHEN** a body is `#2024/q1 and #3d`
- **THEN** the note carries the tags `2024/q1` and `3d`

### Requirement: Tags inside code are ignored
Text inside inline code spans, fenced code blocks, and indented code blocks SHALL NOT produce tags.

#### Scenario: Inline code span
- **WHEN** a body is ``Use `#define` here and #c``
- **THEN** the note carries exactly the tag `c`

#### Scenario: Fenced code block
- **WHEN** a body is `#real\n```\n#fake\n```\n`
- **THEN** the note carries exactly the tag `real`

#### Scenario: Indented code block
- **WHEN** a body is `Intro\n\n    #fake\n\n#real`
- **THEN** the note carries exactly the tag `real`

### Requirement: Nested tags
A tag name SHALL be a path of one or more segments separated by `/`, so `#parent/child/grandchild` names a tag three levels deep. Empty segments SHALL be dropped, so `#a//b/` names `a/b`. A name that has no segments left SHALL NOT be a tag.

#### Scenario: Three-level tag
- **WHEN** a body is `#work/acme/q3`
- **THEN** the note carries the tag with path segments `work`, `acme`, `q3`

#### Scenario: Empty segments
- **WHEN** a body is `#a//b/ and #/`
- **THEN** the note carries exactly the tag `a/b`

### Requirement: Multi-word tags
A tag MAY contain spaces when written as `#` + name + `#` on a single line, where the name begins and ends with a non-whitespace character and contains no `#`. The closing `#` SHALL be followed by whitespace, the end of the line, or trailing punctuation. Trailing punctuation SHALL be dropped from the name as for single-word tags. Runs of whitespace inside the name SHALL be treated as a single space, and segment names SHALL be trimmed.

#### Scenario: Multi-word tag
- **WHEN** a body is `Ideas for #side project# this week`
- **THEN** the note carries exactly the tag `side project`

#### Scenario: Nested multi-word tag
- **WHEN** a body is `#reading list/to buy#`
- **THEN** the note carries the tag with segments `reading list` and `to buy`

#### Scenario: Unclosed multi-word candidate falls back to a single word
- **WHEN** a body is `#work and then #home`
- **THEN** the note carries the tags `work` and `home`

#### Scenario: Trailing punctuation inside a multi-word tag
- **WHEN** a body is `#side projects.# today`
- **THEN** the note carries exactly the tag `side projects`

#### Scenario: Multi-word tags do not span lines
- **WHEN** a body is `#first line\nsecond#`
- **THEN** the note carries exactly the tag `first`

### Requirement: Tag identity
Two tag paths SHALL be the same tag when their segments match after Unicode lowercasing. A note carrying the same tag more than once, in any spelling, SHALL carry it once. Wherever a single display spelling is needed, it SHALL come from the most recently updated note that uses the tag, with ties broken by the smaller note identifier.

#### Scenario: Case-insensitive identity
- **WHEN** one note carries `#Work` and another carries `#work`
- **THEN** both notes appear under a single tag

#### Scenario: Duplicate tags in one note
- **WHEN** a body is `#todo first, #TODO second`
- **THEN** the note carries the tag `todo` once

#### Scenario: Display spelling
- **WHEN** a note updated at T1 carries `#work` and a note updated at a later T2 carries `#Work`
- **THEN** the tag is displayed as `Work`

### Requirement: Derived tag tree
A tag tree SHALL be derivable from any set of notes, with one node per distinct tag path prefix. Each node SHALL list the notes that carry its tag or any descendant tag, so a note tagged `#a/b` appears under both `a` and `a/b`. Archived and trashed notes SHALL NOT contribute to the tree. The tree SHALL NOT be stored; it is rebuilt from note bodies.

#### Scenario: Ancestors are implied
- **WHEN** the only note carries `#work/acme`
- **THEN** the tree has a root node `work` with one child `acme`, and the note appears under both

#### Scenario: Note under several tags
- **WHEN** a note carries `#home` and `#work/acme`
- **THEN** the note appears under `home`, `work`, and `work/acme`

#### Scenario: Directly tagged parent
- **WHEN** note A carries `#work` and note B carries `#work/acme`
- **THEN** `work` lists A and B, and `work/acme` lists only B

#### Scenario: Archived and trashed notes are excluded
- **WHEN** the only note carrying `#old` is archived, and the only note carrying `#gone` is trashed
- **THEN** the tree has no `old` or `gone` node

### Requirement: Tag tree ordering
Sibling nodes in the tag tree SHALL be ordered by display name, compared case-insensitively, with ties broken by the exact display name. Notes listed under a node SHALL be ordered by updated time, most recent first, with ties broken by note identifier.

#### Scenario: Sibling order
- **WHEN** notes carry `#zeta`, `#Alpha`, and `#beta`
- **THEN** the root nodes are ordered `Alpha`, `beta`, `zeta`

#### Scenario: Note order under a tag
- **WHEN** notes A (updated T1) and B (updated at a later T2) both carry `#work`
- **THEN** `work` lists B before A

### Requirement: Special views
The system SHALL provide these derived views over a set of notes: **Notes** (not archived and not trashed), **Untagged** (in Notes and carrying no tags), **Pinned** (in Notes and pinned), **Archive** (archived and not trashed), and **Trash** (trashed). Each view SHALL order notes like the tag tree does.

#### Scenario: View membership
- **WHEN** the notes are A (tagged, pinned), B (untagged), C (archived), and D (trashed, archived)
- **THEN** Notes is {A, B}, Untagged is {B}, Pinned is {A}, Archive is {C}, and Trash is {D}

#### Scenario: Tags inside code do not make a note tagged
- **WHEN** a note's only `#` text is inside a code span
- **THEN** the note appears in Untagged

### Requirement: Tag rename and move
Renaming tag `old` to `new` SHALL rewrite every occurrence of `old` and of its descendants `old/...` in the given notes' bodies, so `old/x` becomes `new/x`. Matching SHALL follow tag identity rules. Moving a tag is a rename to a different parent path. The operation SHALL be pure: it returns the identifier and new body of each note whose body changed and modifies nothing itself.

#### Scenario: Simple rename
- **WHEN** `work` is renamed to `job` across a note with body `#work stuff`
- **THEN** the result contains that note with body `#job stuff`

#### Scenario: Descendants follow
- **WHEN** `work` is renamed to `job` across a note with body `#work/acme and #work`
- **THEN** the new body is `#job/acme and #job`

#### Scenario: Move under a new parent
- **WHEN** `acme` is renamed to `clients/acme` across a note with body `#acme/q3`
- **THEN** the new body is `#clients/acme/q3`

#### Scenario: Sibling prefixes are not touched
- **WHEN** `work` is renamed to `job` across a note with body `#workshop`
- **THEN** the result is empty

#### Scenario: Case-insensitive match
- **WHEN** `work` is renamed to `job` across a note with body `#Work/Acme`
- **THEN** the new body is `#job/Acme`

#### Scenario: Unaffected notes are omitted
- **WHEN** a rename is applied to notes where only one carries the tag
- **THEN** the result contains only that note

#### Scenario: Merge into an existing tag
- **WHEN** `job` is renamed to `work` and some notes already carry `#work`
- **THEN** the renamed notes carry `work` and the tree has a single `work` node

### Requirement: Rename preserves everything else
A rename SHALL change only the text of matching tag occurrences. Code, URLs, non-matching tags, and all other text SHALL remain byte for byte identical. A rewritten tag SHALL use the single-word form when its new name has no spaces and the `#...#` form when it does, keeping any trailing punctuation outside the tag.

#### Scenario: Code is not rewritten
- **WHEN** `work` is renamed to `job` across a note with body ``#work and `#work` ``
- **THEN** the new body is ``#job and `#work` ``

#### Scenario: Rename to a multi-word name
- **WHEN** `ideas` is renamed to `side projects` across a note with body `See #ideas.`
- **THEN** the new body is `See #side projects#.`

#### Scenario: Rename from a multi-word name
- **WHEN** `side projects` is renamed to `ideas` across a note with body `#side projects# today`
- **THEN** the new body is `#ideas today`

#### Scenario: Rename applies to archived and trashed notes
- **WHEN** a rename is applied to a set that includes an archived note and a trashed note carrying the tag
- **THEN** both notes are rewritten

### Requirement: Invalid rename targets are rejected
A rename SHALL fail without returning any rewrites when the new name is not a valid tag name: empty after dropping empty segments, containing `#` or a line break, consisting only of digits, or ending in trailing sentence punctuation. A rename whose old name is invalid SHALL also fail.

#### Scenario: Empty new name
- **WHEN** `work` is renamed to `/`
- **THEN** the rename fails with an invalid-name error

#### Scenario: Hash in new name
- **WHEN** `work` is renamed to `a#b`
- **THEN** the rename fails with an invalid-name error

#### Scenario: Numeric new name
- **WHEN** `work` is renamed to `42`
- **THEN** the rename fails with an invalid-name error

#### Scenario: Trailing punctuation in new name
- **WHEN** `work` is renamed to `job!`
- **THEN** the rename fails with an invalid-name error
