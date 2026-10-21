# Design

## Context

See proposal.md (Why). `notebook-core` currently holds only the `version()` placeholder from `scaffold-workspace`. It has no dependencies, forbids `unsafe`, and CI builds it for `wasm32-unknown-unknown`, so everything here must be pure Rust with no I/O, no system clock, and no OS randomness. Later changes (`crdt-note-documents`, `local-sqlite-storage`, `markdown-export`, the editor's tag highlighting) consume the note model and the tag parser, and the editor must highlight exactly what the parser treats as a tag.

## Goals / Non-Goals

**Goals:**
- A small, owned note type with the behavior in `specs/notes/spec.md`.
- A tag parser that returns byte ranges, so the same scan drives the tag tree, rename, and later editor highlighting.
- Tag tree, views, and rename as pure functions over `&[Note]`.

**Non-Goals:**
- Persistence, serialization formats, and CRDT representation. `crdt-note-documents` decides how a note is stored and synced; this change adds no `serde` derives so it doesn't fix a wire format early.
- Incremental tag indexing. The tree is rebuilt from scratch; caching is a client concern.
- Full-text search and note sorting options beyond what the specs fix.

## Decisions

### Module layout

```
crates/notebook-core/src/
  lib.rs           re-exports note and tags
  note.rs          NoteId, Timestamp, Note, title derivation
  tags/
    mod.rs         TagPath, public API
    parse.rs       scanner + code-range exclusion
    tree.rs        TagTree, TagNode
    views.rs       View enum and selection
    rename.rs      rename/move rewrite
```

### Identity and time are supplied by the caller

- `NoteId` wraps `ulid::Ulid`, with `Display`/`FromStr`, `Ord`, and `NoteId::from_parts(timestamp_ms, random: u128)`. The `ulid` crate is used with `default-features = false` so it doesn't pull in `rand`/`getrandom`, which need extra configuration on wasm32. Native and browser callers supply randomness from their own platform source.
- `Timestamp` is a newtype over `i64` Unix milliseconds (UTC). Every mutating method takes `now: Timestamp`.
- **Alternative considered:** `chrono` or `time` types. Rejected because nothing here formats or does calendar math; export can convert at its boundary. An `i64` also maps directly to SQLite and CRDT maps later.
- **Alternative considered:** generating IDs inside core with `getrandom`'s `wasm_js` backend. Rejected for now because it adds wasm-specific build configuration to the core and makes tests nondeterministic.

### Note API

`Note` has private fields with getters (`id`, `body`, `created`, `updated`, `is_pinned`, `is_archived`, `is_trashed`, `title`) and mutators (`set_body(body, now)`, `set_pinned(bool)`, `set_archived(bool)`, `set_trashed(bool)`). Only `set_body` takes `now`, since only body changes bump `updated`. `title()` is computed on each call (it only scans to the first non-blank line), so the title can never get out of sync with the body.

### Tag scanning: CommonMark for exclusions, hand-written scanner for tags

1. Run `pulldown-cmark` (`default-features = false`) with `into_offset_iter()` over the body, collecting the byte ranges of `Code` events (inline spans) and `CodeBlock` start/end ranges (fenced and indented). HTML is not excluded: the spec only exempts code, and attribute values like `href="#x"` already fail the whitespace-before-`#` rule.
2. Scan the raw text for `#` positions outside those ranges and apply the rules in `specs/tags/spec.md`: the boundary before `#`, the multi-word closer search on the same line, trailing-punctuation trimming, segment splitting and normalization, and rejection of numeric-only names.
3. Return `Vec<TagOccurrence { range: Range<usize>, path: TagPath, form: TagForm }>`, where `TagForm` is `Bare` (`#a`), `Closed` (`#a#`), or `MultiWord` (`#a b#`), and `range` covers the source text from the opening `#` through the name and any closing `#`. Trailing punctuation, `/`, and whitespace are trimmed from the end of the name in both forms, so no tag name ends in punctuation and every tag can be renamed.

- **Why not tags from `Text` events alone:** CommonMark splits text at emphasis, entity, and escape boundaries, so a multi-word tag like `#my *big* idea#` would arrive as several events and the offsets would be awkward for rename. Scanning raw text keeps exact byte ranges, and the parser is used only for what it's good at: knowing where code is.
- **Alternative considered:** `comrak`. It's heavier, builds an arena AST, and has less direct source offsets. `pulldown-cmark` is a small pure-Rust crate that builds for wasm32.
- Heading markers need no special case: `# Heading` has whitespace after the `#`, and an ATX closing sequence is preceded by whitespace and followed by end-of-line.

### TagPath and identity

`TagPath` stores the normalized display segments (`Vec<String>`) and derives a comparison key from per-segment `to_lowercase()`. `Eq`/`Hash`/`Ord` use the key, and `Display` joins the display segments with `/`. `TagPath::parse(&str)` validates user input for rename using the same segment rules as the scanner, and `is_prefix_of` supports descendant matching.

### Tag tree

`TagTree::build(notes: &[Note])` skips archived and trashed notes, parses each remaining body once, and inserts each distinct `TagPath` into a tree keyed by lowercased segment. Each node keeps `(updated, id)` per note to sort lists and choose display spelling: each node takes its segment spelling from the most recently updated contributing note, which implements the tag identity rule. The output is an owned `TagNode { name, path, notes: Vec<NoteId>, children: Vec<TagNode> }` with siblings and notes already sorted per spec, so clients can render it directly.

### Views

`View` is an enum `{ Notes, Untagged, Pinned, Archive, Trash }` with `View::contains(self, &Note)` and `View::select(self, notes: &[Note]) -> Vec<NoteId>`, which shares the tree's sort. Selecting by tag is `TagTree::find(&TagPath)`.

### Rename

`rename(notes, old: &str, new: &str) -> Result<Vec<(NoteId, String)>, RenameError>` validates both names with `TagPath::parse`. For each note it rescans the body, takes the occurrences whose path has `old` as a prefix (by key), and rebuilds the body front to back, copying the text between occurrences unchanged. Each replacement is `new`'s segments followed by the occurrence's remaining suffix segments, in their original spelling, emitted as `#name#` when the result contains a space or the original used the `Closed` form, and `#name` otherwise. Keeping a closing `#` the author wrote preserves their style, and the next character is already known to be a boundary. Notes with no matches are omitted. Because only occurrence ranges change, all other bytes are preserved.

## Risks / Trade-offs

- [The multi-word rule is greedy within a line: `#foo is like C# here` reads as the tag `foo is like C`] → This matches Bear's behavior, and the spec scenarios pin down the common cases. If it surprises people, the closer rule can be tightened later in its own change.
- [Two parse passes per body (CommonMark plus scanner)] → Both are linear and bodies are small. The tree is rebuilt only when notes change.
- [Rename can produce a body whose new tag changes meaning, e.g. renaming to a name ending in punctuation] → `TagPath::parse` rejects names the scanner wouldn't round-trip. A unit test asserts `parse(rendered tag) == new path` for each rewritten occurrence.
- [No `serde` derives yet] → Each consuming change adds the derives or DTOs it needs, which keeps the persisted format decision in `crdt-note-documents`.
