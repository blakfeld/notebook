# Proposal

## Why

Users must be able to export all notes as plain markdown. With end-to-end encryption, a forgotten passphrase and lost recovery key mean lost data, so a portable plaintext export is both a data-ownership feature and the user's own backup. Because the server can't decrypt anything, export must run on the client.

## What Changes

- Export every non-trashed note (with an option to include archived and trashed) as one `.md` file per note in a **flat** layout:
  - Tags stay inline in the body exactly as written, so nothing about the tag hierarchy is lost.
  - YAML frontmatter carries metadata: stable `id`, `created`, `updated`, `pinned`, `archived`.
  - File names come from slugified titles, with deterministic suffixes on collisions.
- **Native**: export to a user-chosen folder.
- **Browser**: export as a `.zip` download generated in memory (in WASM).
- Format is deterministic and documented so a future import can round-trip it and so it's compatible with Bear and Obsidian conventions.
- Export logic lives in `notebook-core`. Clients only choose the destination.

## Capabilities

### New Capabilities
- `markdown-export`: Client-side export of the vault as markdown files: flat layout, inline tags, frontmatter metadata, file naming and collision rules, folder (native) and zip (browser) outputs, and inclusion options.

### Modified Capabilities
- `app-shell` (introduced by `desktop-app-shell`): settings gain an "Export all notes" action.

## Impact

- `notebook-core`: `export` module, plus a zip writer dependency (e.g. `zip` crate built for wasm).
- Desktop: a Tauri folder-picker dialog. Browser: Blob download.
- Attachments aren't in scope yet. When added, they'll export to an `attachments/` folder with relative links.
