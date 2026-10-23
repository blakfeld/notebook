# Tasks

## 1. Dependencies and module skeleton

- [ ] 1.1 Create the empty `export/{mod,name,time,dir}.rs` modules (`dir` gated on `cfg(not(target_arch = "wasm32"))`) re-exported from `lib.rs`; verify `cargo build -p notebook-core` and `cargo build -p notebook-core --target wasm32-unknown-unknown` both succeed

## 2. Export planning in notebook-core

- [ ] 2.1 Implement timestamp formatting in `export/time.rs` (or reuse the stamp formatter if `vault-backup` already added one) (`YYYY-MM-DDTHH:MM:SS.sssZ` and the `YYYYMMDD-HHMMSS` folder stamp from `Timestamp`, civil-from-days, no date library); verify unit tests for the spec's example instants, the Unix epoch, a leap day, and a negative timestamp pass
- [ ] 2.2 Implement name sanitizing in `export/name.rs` (unsafe-character replacement, whitespace collapse, leading/trailing trimming, `Untitled`, reserved device names, 120-byte cap on a char boundary); verify unit tests cover every scenario under "File names from titles" and "Fallback and length limits for names"
- [ ] 2.3 Implement collision assignment (identifier order, lowercased taken-set, smallest free ` N`); verify unit tests cover every scenario under "Deterministic collision suffixes"
- [ ] 2.4 Implement `ExportOptions`, `ExportFile`, `ExportPlan`, and `plan(notes, options, now)` (selection, frontmatter rendering, verbatim body, files sorted by name, folder name); verify unit tests cover the "Note selection", "Flat layout in one export folder", "File contents", and "Frontmatter metadata" scenarios, including a golden test of the exact frontmatter bytes and a test-only reader that recovers each body exactly (including a body starting with `---`)
- [ ] 2.5 Write the module rustdoc describing the format and `docs/export-format.md` (layout, frontmatter keys and formats, naming and collision rules, how an importer recovers the body, Obsidian caveat for multi-word tags); verify `cargo doc -p notebook-core --no-deps` builds without warnings and the doc's examples match the golden test

## 3. Folder output

- [ ] 3.1 Implement `write_dir(&ExportPlan, parent) -> Result<PathBuf, ExportError>` (free-name resolution with `-N`, staging folder in the same parent, `create_new` writes, best-effort `set_modified`, fsync, rename, cleanup on error); verify `tempfile`-based tests cover the "Folder output" scenarios (contents, `-2` suffix leaving the existing folder unchanged, mtimes equal note `updated`)
- [ ] 3.2 Add a failure test that makes one write fail (e.g. a pre-created read-only staging target or an injected writer error) and asserts an error is returned and no export or staging folder remains; verify `cargo test -p notebook-core` passes

## 4. Desktop wiring

- [ ] 4.1 Add `tauri-plugin-dialog` (unless `vault-backup` already did) to the workspace and `notebook-app`, initialize it in the builder without adding any JS dialog permission to `capabilities/default.json`; verify `cargo build -p notebook-app` succeeds and the capability file is unchanged apart from anything the plugin strictly requires
- [ ] 4.2 Implement the async `export_notes { include_trashed }` command: open the Rust-side folder picker (refusing while locked only if `local-only-desktop` hasn't landed yet), return `null` on cancel, otherwise collect current `Note`s from the loaded documents, call `plan` with the system clock and `write_dir`, and return `{ count, path }` or a string error; verify mock-runtime tests (in the existing `lib.rs` test style) for the cancel and success paths with the picker stubbed behind a small trait
- [ ] 4.3 Add `exportNotes({ includeTrashed })` to the frontend platform adapter (`web/src/platform`) with the Tauri implementation and fake/demo results; verify `npm run check` and `npm test` (adapter unit test) pass

## 5. Settings UI

- [ ] 5.1 Add the "Export all notes" section to settings: unencrypted-output warning with the advice against synced folders, include-trashed toggle (off by default), export button disabled while exporting, success message with count and location, error message on failure, nothing shown on cancel; verify component tests cover each scenario in `specs/app-shell/spec.md` with the adapter mocked
- [ ] 5.2 End-to-end check on desktop: run the app, create active, archived, and trashed notes (including two with the same title and one with a multi-word tag), export with and without include-trashed, and confirm the folder contents, names, frontmatter, and file times match the spec; open the folder in Obsidian or a text editor to confirm it reads cleanly

## Workflow follow-up

- Review with `code-reviewer`, then archive with `/opsx:archive`. It adds to `app-shell` alongside `local-only-desktop` and `vault-backup` (different requirements, so the archive order is free), and expect a small merge in `Settings.svelte`.
