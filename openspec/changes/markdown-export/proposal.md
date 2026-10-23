# Proposal

## Why

Users must be able to export all notes as plain markdown. Encrypted backups (`vault-backup`) protect against losing a computer, but only Notebook can read them, and only with the recovery key. A readable markdown export is the data-ownership escape hatch: it opens in any editor, survives a lost recovery key, and is the basis for importing into other tools. It runs on the device from the notes the app already holds decrypted in memory.

## What Changes

- Export every non-trashed note (active and archived) as one `.md` file per note in a **flat** layout. An option also includes trashed notes.
  - The body is written byte for byte as stored. Tags stay inline exactly as written, so nothing about the tag hierarchy is lost.
  - YAML frontmatter carries metadata in a fixed order: stable `id`, `created`, `updated` (UTC, RFC 3339 with milliseconds), `pinned`, `archived`, `trashed`.
  - File names come from the note's title, sanitized to be safe on Windows, macOS, and Linux, with deterministic ` 2`, ` 3` suffixes on case-insensitive collisions. Untitled notes are named `Untitled`.
  - All files go inside one export folder named after the export time (`notebook-export-YYYYMMDD-HHMMSS`), so an export never mixes with or overwrites existing files.
- The user picks a parent folder; the export folder is written there, all-or-nothing. Each file's modification time is set to the note's updated time.
- The format is deterministic and documented so a future import can round-trip it, and it opens cleanly in Obsidian and other markdown tools.
- Export logic (selection, naming, file contents, folder writer) lives in `notebook-core`. The app only chooses the destination and shows the result.
- Settings gain an "Export all notes" action that warns the export is not encrypted (unlike backups) and advises against putting it in a cloud-synced folder.

## Capabilities

### New Capabilities
- `markdown-export`: On-device export of the vault as markdown files: note selection, flat layout, inline tags, frontmatter metadata, file naming and collision rules, and the export folder.

### Modified Capabilities
- `app-shell` (introduced by `desktop-app-shell`): settings gain an "Export all notes" action with an include-trashed option, an unencrypted-output warning, and success and failure reporting.

## Impact

- `notebook-core`: new `export` module (pure planning, wasm-compatible; folder writing native-only). No new dependencies.
- `notebook-app`: an `export_notes` Tauri command that opens the native folder picker on the Rust side (`tauri-plugin-dialog`, shared with `vault-backup`; whichever lands first adds it) so the webview never supplies a write path.
- Frontend: the export action in settings and an `exportNotes` method on the platform adapter.
- The `YYYYMMDD-HHMMSS` stamp formatter is shared with `vault-backup` (whichever lands first adds it to core).
- `docs/export-format.md` documents the format.
- Attachments aren't in scope yet. When added, they'll export to an `attachments/` folder with relative links.
