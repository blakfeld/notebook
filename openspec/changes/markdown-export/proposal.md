# Proposal

## Why

Users must be able to export all notes as plain markdown. With end-to-end encryption, a forgotten passphrase and lost recovery key mean lost data, so a portable plaintext export is both a data-ownership feature and the user's own backup. Because the server can't decrypt anything, export must run on the client, from the notes it already holds decrypted in memory, and it must work offline.

## What Changes

- Export every non-trashed note (active and archived) as one `.md` file per note in a **flat** layout. An option also includes trashed notes.
  - The body is written byte for byte as stored. Tags stay inline exactly as written, so nothing about the tag hierarchy is lost.
  - YAML frontmatter carries metadata in a fixed order: stable `id`, `created`, `updated` (UTC, RFC 3339 with milliseconds), `pinned`, `archived`, `trashed`.
  - File names come from the note's title, sanitized to be safe on Windows, macOS, and Linux, with deterministic ` 2`, ` 3` suffixes on case-insensitive collisions. Untitled notes are named `Untitled`.
  - All files go inside one export folder named after the export time (`notebook-export-YYYYMMDD-HHMMSS`), so an export never mixes with or overwrites existing files.
- **Native (desktop)**: the user picks a parent folder; the export folder is written there, all-or-nothing. Each file's modification time is set to the note's updated time.
- **Browser**: the same export folder as a `.zip` built in memory (in WASM) and handed to the browser as a download.
- The format is deterministic and documented so a future import can round-trip it, and it opens cleanly in Obsidian and other markdown tools.
- Export logic (selection, naming, file contents, zip and folder writers) lives in `notebook-core`. Clients only choose the destination and show the result.
- Settings gain an "Export all notes" action that warns the export is not encrypted.

## Capabilities

### New Capabilities
- `markdown-export`: Client-side export of the vault as markdown files: note selection, flat layout, inline tags, frontmatter metadata, file naming and collision rules, the export folder, and the folder (native) and zip (browser) outputs.

### Modified Capabilities
- `app-shell` (introduced by `desktop-app-shell`): settings gain an "Export all notes" action with an include-trashed option, an unencrypted-output warning, and success/failure reporting.

## Impact

- `notebook-core`: new `export` module (pure planning and zip writing, wasm-compatible; folder writing native-only), plus the `zip` crate (pure-Rust deflate) as a dependency.
- `notebook-app`: an `export_notes` Tauri command that opens the native folder picker on the Rust side (`tauri-plugin-dialog`) so the webview never supplies a write path.
- Frontend: the export action in settings and an `exportNotes` method on the platform adapter.
- `browser-web-client` (later in the plan) wires the core zip output through its wasm bindings and triggers the download; `mobile-app` reuses the zip output for the share sheet.
- `docs/export-format.md` documents the format.
- Attachments aren't in scope yet. When added, they'll export to an `attachments/` folder with relative links.
