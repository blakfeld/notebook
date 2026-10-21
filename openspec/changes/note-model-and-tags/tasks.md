# Tasks

## 1. Dependencies and module skeleton

- [x] 1.1 Add `ulid` (`default-features = false`) and `pulldown-cmark` (`default-features = false`) to `[workspace.dependencies]` and `notebook-core`; create the empty `note` and `tags` modules (`tags/{mod,parse,tree,views,rename}.rs`) re-exported from `lib.rs`; verify `cargo build -p notebook-core` and `cargo build -p notebook-core --target wasm32-unknown-unknown` both succeed

## 2. Note model

- [x] 2.1 Implement `NoteId` (ULID newtype with `from_parts`, `Display`, `FromStr`, `Ord`) and `Timestamp` (`i64` Unix ms); verify unit tests for round-trip, invalid text rejection, and creation-time ordering pass
- [x] 2.2 Implement `Note` with `new(id, body, now)`, getters, `set_body(body, now)`, and `set_pinned`/`set_archived`/`set_trashed`; verify unit tests cover every scenario under the timestamp, body, and pinned/archived/trashed requirements in `specs/notes/spec.md`
- [x] 2.3 Implement `Note::title()` per the derived-title and empty-title requirements; verify unit tests for each title scenario pass
- [x] 2.4 Add rustdoc for the public `note` API (including that callers supply time and randomness); verify `cargo doc -p notebook-core --no-deps` builds without warnings

## 3. Tag parsing

- [x] 3.1 Implement `TagPath` (normalized display segments, lowercased key for `Eq`/`Hash`/`Ord`, `Display`, `parse` with validation, `is_prefix_of`); verify unit tests for segment normalization, case-insensitive equality, and every invalid-name case in the rename spec
- [x] 3.2 Implement code-range exclusion with `pulldown-cmark` offsets (inline code, fenced and indented code blocks, inline and block HTML); verify unit tests that return the expected byte ranges for each construct
- [x] 3.3 Implement the scanner returning `TagOccurrence { range, path, multi_word }` and a `tags_of(body)` helper returning distinct paths; verify unit tests cover every scenario in the single-word, not-a-tag, code, nested, multi-word, and tag identity (duplicates) requirements
- [x] 3.4 Add rustdoc on the parser describing the tag grammar (the editor highlighter will mirror it); verify `cargo doc -p notebook-core --no-deps` builds without warnings

## 4. Tag tree and views

- [x] 4.1 Implement `TagTree::build`, `TagNode`, and `TagTree::find`, including display-spelling selection and sorting; verify unit tests cover the derived tag tree, ordering, and display-spelling scenarios
- [x] 4.2 Implement `View` (`Notes`, `Untagged`, `Pinned`, `Archive`, `Trash`) and `View::select`; verify unit tests cover the view membership and code-only-hash scenarios

## 5. Tag rename and move

- [x] 5.1 Implement `rename(notes, old, new)` with `RenameError`, back-to-front splicing, and single-word versus `#...#` output; verify unit tests cover every scenario in the rename, preservation, and invalid-target requirements
- [x] 5.2 Add a round-trip test: for a corpus of bodies and renames, every rewritten occurrence re-parses to the expected new path and the text outside the occurrences is unchanged; verify `cargo test -p notebook-core` passes

## 6. Integration checks

- [x] 6.1 Run `scripts/check.sh` and verify it exits 0 (fmt, clippy with `-D warnings`, workspace tests, wasm32 core build)
- [x] 6.2 Run `openspec validate note-model-and-tags --strict` and verify it passes

## Workflow follow-up

- Review the change, then archive it with `/opsx:archive` so the `notes` and `tags` specs land in `openspec/specs/`.
- Update `TODO.md`: mark `note-model-and-tags` as `[x]` once archived.
