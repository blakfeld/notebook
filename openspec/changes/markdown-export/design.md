# Design

## Context

See proposal.md (Why). Requirements are in `specs/markdown-export/spec.md` and `specs/app-shell/spec.md`.

`notebook-core` has the `note` module (`Note`, `NoteId` as a ULID newtype, `Timestamp(i64)` Unix ms, `Note::title()`) and `tags`. It forbids `unsafe`, takes time from the caller, and CI builds it for `wasm32-unknown-unknown`. It has no date/time library: the note-model design deferred calendar formatting to "export, at its boundary". `crdt-note-documents` exposes each document's current state as a `Note` (`NoteDocument::note()`), so by the time this change is implemented the app holds every note, including archived and trashed, as `Note` values in memory. That is export's only input.

Ordering (TODO.md): this change follows `desktop-app-shell` (it adds to `app-shell`) and precedes `browser-web-client` and `mobile-app`. So the desktop path is wired end to end here, while the browser and mobile paths get a ready-made core API that their own changes call.

## Goals / Non-Goals

**Goals:**
- One pure function that turns notes + options + export time into an exact list of files, so every output (folder, zip, share sheet) is the same export.
- A format a later import can parse without guessing, pinned by golden tests.
- No plaintext touches disk except where the user explicitly asked for it.

**Non-Goals:**
- Import. The format is designed for it; the importer is a later change.
- Attachments (none exist yet).
- Exporting a single note or a tag subset. The core API takes any `&[Note]`, so a later UI can add this without format changes.
- Rewriting bodies for other apps (e.g. converting `#multi word#` tags for Obsidian). Bodies stay verbatim.
- Progress reporting or streaming. Personal-scale vaults export in well under a second.
- The browser's wasm binding and download trigger, and the mobile share sheet; those belong to `browser-web-client` and `mobile-app`.

## Decisions

### Module layout

```
crates/notebook-core/src/export/
  mod.rs      ExportOptions, ExportFile, ExportPlan, plan(), ExportError, rustdoc for the format
  name.rs     sanitize(title) -> String, collision assignment
  time.rs     RFC 3339 formatting and folder-name stamp from Timestamp (civil-from-days)
  zip.rs      write_zip(&ExportPlan) -> Result<Vec<u8>, ExportError>   (all targets)
  dir.rs      write_dir(&ExportPlan, parent: &Path) -> PathBuf   (cfg(not(target_arch = "wasm32")))
```

```rust
pub struct ExportOptions { pub include_trashed: bool }
pub struct ExportFile { pub name: String, pub contents: String, pub modified: Timestamp, pub id: NoteId }
pub struct ExportPlan { pub folder_name: String, pub files: Vec<ExportFile> }  // files sorted by name
pub fn plan(notes: &[Note], options: ExportOptions, now: Timestamp) -> ExportPlan;
```

`plan` is pure and infallible: selection, naming, and rendering. The writers only serialize a plan. That keeps all format rules in one testable place and guarantees the folder and zip outputs agree (a spec requirement).

**Alternative considered:** a streaming `Exporter` trait with folder and zip sinks. Rejected: it spreads format rules across sinks and buys nothing at this scale.

### Selection includes archived by default

The original proposal wording was ambiguous ("non-trashed, with an option to include archived and trashed"). Export is the user's backup, and archived notes are kept-on-purpose content, so default = everything not trashed. Trashed notes are opt-in because the user already signalled they don't want them. `archived`/`trashed` flags in the frontmatter mean nothing is lost either way.

### Frontmatter: fixed, minimal, hand-written

Six keys in fixed order, written by hand rather than through a YAML library, because every value is a ULID, a timestamp, or a bool, none of which need quoting. `trashed` is always written (not only for trashed notes) so every file has the same shape. Tags are not duplicated into a `tags:` list: the body is the source of truth, and a second copy could disagree (e.g. tags inside code spans) and would have to be ignored by an importer anyway. The frontmatter always uses `\n`; the body keeps whatever line endings it has.

Timestamps are formatted `YYYY-MM-DDTHH:MM:SS.sssZ` with a small civil-from-days routine (Howard Hinnant's algorithm) on `i64` ms. Obsidian reads these as datetime properties.

**Alternative considered:** `chrono` or `time`. Rejected: one formatting function doesn't justify a date library in a wasm-built core. **Alternative considered:** `serde_yaml`. Rejected: unmaintained, and unnecessary for six scalar keys.

### Readable names, not slugs

The proposal draft said "slugified titles". Names are instead the title with only unsafe characters replaced (`Grocery list.md`, not `grocery-list.md`). Obsidian and similar tools use the file name as the note's display name and link target, and Bear exports the same way, so readable names make the export usable as a vault rather than only an archive. The replaced set is the union of Windows-illegal characters and characters that break Obsidian wikilinks (`# ^ [ ]`); the 120-byte cap leaves room for ` NNN.md` and the folder path under common 255-byte name limits. The name keeps the title's Unicode form as typed (no NFC/NFD rewrite).

### Collision suffixes by identifier order

Names are assigned walking notes in ascending `NoteId` order (which is creation order), checking a set of names already taken, keyed by the case-folded (upper- then lowercased) NFC form. Folding and NFC make the export safe on case-insensitive filesystems (macOS, Windows) and in zips extracted there. ` N` matches what Finder and Obsidian do.

**Alternative considered:** suffixing with part of the note ID (`Ideas (01JBX3).md`) for names that stay stable when notes are added. Rejected: ugly for the common case, and stability across different note sets isn't a requirement; determinism for the same set is.

### Zip via the `zip` crate, deflate, deterministic

`zip` with `default-features = false, features = ["deflate"]`, which uses `flate2` over `zlib-rs` (pure Rust) and builds for wasm32. Writes into `Cursor<Vec<u8>>`. Determinism: entries in plan order (sorted by name), a directory entry for the export folder first, the UTF-8 name flag set, each entry's DOS timestamp from the note's `updated` (UTC, clamped to the DOS range 1980–2107; the folder entry uses `now`), and fixed compression level. A test asserts two writes of the same plan are byte-identical. `write_zip` returns a `Result` rather than panicking on a zip error.

**Alternative considered:** a hand-written store-only zip writer (~150 lines plus CRC32). Rejected: deflate matters for text, and zip edge cases (UTF-8 names, CRCs, central directory) are better left to a maintained crate.

### Folder writer: stage then rename

`write_dir` resolves a free folder name (`notebook-export-…`, then `-2`, `-3`, …), creates a staging folder `notebook-export-incomplete-<random>` in the same parent (visible, so one left by a crash is noticed; mode 0700 on Unix, kept by the rename, so the export is private to the user), writes every file with `create_new(true)`, sets each mtime with `File::set_modified`, fsyncs, then renames the staging folder to the final name. On any error it removes the staging folder and returns the error. Same-parent rename is atomic on all desktop platforms, so the user either sees a complete export folder or none. The final name is re-checked immediately before the rename; if it was taken in the meantime the next suffix is used.

### Desktop: the picker runs in Rust

The `export_notes { include_trashed }` command opens the folder picker itself through `tauri-plugin-dialog`'s Rust API, then calls `plan` + `write_dir`, and returns `{ count, path }` or `null` when cancelled. The webview never passes a filesystem path, so a compromised webview cannot direct plaintext writes to an arbitrary location, and no JS dialog permission is added to the capability file. `now` comes from the system clock in the app crate, keeping core clock-free.

**Alternative considered:** JS `open({ directory: true })` then `invoke('export_notes', { path })`. Rejected for the reason above.

### Frontend: one adapter method

The platform adapter gains `exportNotes({ includeTrashed }): Promise<ExportResult | null>`. The Tauri implementation invokes the command. The browser implementation (added by `browser-web-client`) will call a wasm `export_zip(include_trashed, now_ms)` returning bytes plus the suggested file name, wrap them in a `Blob`, click a temporary `<a download>`, and revoke the object URL. The settings UI is platform-neutral: it shows the warning and option, calls the adapter, and reports the result.

## Risks / Trade-offs

- [Plaintext export on disk or in Downloads defeats E2EE for that copy] → The UI warns before every export; nothing is written without an explicit user action; desktop writes only to a user-picked folder.
- [Browser "nothing persisted" rule (`web-client`) vs. a downloaded zip] → The download is an explicit user action producing a user-owned file, not app storage. `browser-web-client` should state this exception in its spec.
- [Multi-word tags (`#side project#`) aren't understood by Obsidian] → Accepted; bodies stay verbatim so Notebook and Bear round-trip exactly. Documented in `docs/export-format.md`.
- [Whole export held in memory] → Fine at personal scale (thousands of notes of text). Revisit with streaming when attachments arrive.
- [Names shift when an older note with the same title appears] → Accepted; same input gives same output, and `id` in the frontmatter is the stable identity for import.
- [`File::set_modified` unsupported on some filesystems (e.g. some network shares)] → Treat a failure to set mtime as non-fatal; the contents matter, times are a convenience. (Spec scenario uses a local disk.)
- [DOS timestamps in zips are local-time without zone] → Written as UTC; unzip tools may show a shifted time. The authoritative times are in the frontmatter.

## Migration Plan

None: new, additive feature. The format is versionless for now; if it changes, a `format` frontmatter key can be added and its absence will mean this version.
