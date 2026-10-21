# Proposal

## Why

Notebook organizes notes the way Bear does: nested tags written inline in the markdown body (`#work/acme`) instead of folders. The tag hierarchy is *derived* from note content, never stored separately. That fits end-to-end encryption well, because the server never learns the organizational structure. The note model and tag parser are the foundation every client, the sync layer, and export build on, so they come first. They live in `notebook-core` so native and browser clients behave identically.

## What Changes

- Define the core note model in `notebook-core`: stable note ID (ULID), markdown body, created/updated timestamps, and state flags (pinned, archived, trashed).
- Derive a note's title from its first heading or first non-empty line.
- Implement a Bear-compatible inline tag parser:
  - Single-word tags `#tag`, nested tags `#parent/child/grandchild`, and multi-word tags `#multi word tag#`.
  - Ignore `#` inside code spans, fenced code blocks, URLs, and markdown headings (`# Heading`).
- Build a derived tag tree from a set of notes. A note appears under every tag it carries, and ancestors are implied (`#a/b` places the note under `a` as well).
- Provide derived special views: Untagged, Pinned, Archive, Trash.
- Implement tag rename and move as a pure body-rewrite operation: given notes and `old -> new`, return the rewritten bodies, including descendant tags.

## Capabilities

### New Capabilities
- `notes`: The note entity: identity, markdown body, title derivation, timestamps, and pinned/archived/trashed lifecycle states.
- `tags`: Inline Bear-style tag syntax, parsing rules, derived nested tag tree, special views, and tag rename/move semantics.

### Modified Capabilities
<!-- None. -->

## Impact

- `notebook-core`: new `note` and `tags` modules. Pure Rust with no I/O, and must compile to `wasm32`.
- Likely dependency: a CommonMark parser (`pulldown-cmark` or `comrak`) to locate code spans and blocks accurately.
- Later changes depend on this: storage, sync, editor tag highlighting, the app sidebar, and export.
