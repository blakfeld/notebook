# Tasks

## 1. Dependencies and module skeleton

- [ ] 1.1 Confirm `note-editor-component` and `desktop-app-shell` are archived (`openspec/specs/note-editor/` and `openspec/specs/app-shell/` exist) and note the existing editor module, tag scanner, and device-local settings API; verify by listing those spec paths and the editor module files
- [ ] 1.2 Add `@codemirror/language-data`, `@fontsource-variable/inter`, `@fontsource-variable/jetbrains-mono`, and `@tauri-apps/plugin-opener` to `web/package.json`; create empty `web/src/theme/` and live-preview module files under the editor module; verify `npm install` and `npm run build` succeed in `web/`
- [ ] 1.3 Add `tauri-plugin-opener` to `crates/notebook-app` and register it, with `opener:allow-open-url` scoped to `http://*`, `https://*`, and `mailto:*` in `capabilities/default.json`; verify `cargo build -p notebook-app` succeeds

## 2. Themes and preferences (`appearance`)

- [ ] 2.1 Add `web/src/lib/preferences.ts`: a reactive store for `theme` (`system|light|dark`), `editor.sourceView`, and `editor.focusMode` on top of the device-local settings API, falling back to defaults when reads fail; verify Vitest tests cover defaults, persistence round-trip, and read failure
- [ ] 2.2 Add `web/src/theme/tokens.css` with light and dark token sets under `:root[data-theme=…]` plus `prefers-color-scheme` defaults for `:root:not([data-theme])`, and replace the ad-hoc colors in `app.css`; verify a Vitest contrast test computes every text/background token pair at ≥4.5:1 and every control, focus-ring, syntax-marker, and dim pair at ≥3:1 in both themes
- [ ] 2.3 Add `web/src/theme/theme.ts` to resolve the preference against `matchMedia`, set `data-theme` on the root element, and follow OS changes live under System; apply it in `main.ts` before mounting the app; verify Vitest tests (mocked `matchMedia`) for each preference value and a live OS switch
- [ ] 2.4 Add a System / Light / Dark control to the Settings view with an accessible label; verify in the running app that the whole UI switches immediately, survives a restart, and that a Dark preference never shows a light first frame on startup

## 3. Live-preview core rendering (`note-editor`)

- [ ] 3.1 Implement the decoration ViewPlugin over visible ranges with active-line computation from all selection ranges, and a `livePreview` compartment enabled by default; verify Vitest tests on an `EditorState`/headless view that the active line set matches single cursor, multi-cursor, and multi-line selection cases
- [ ] 3.2 Hide and dim markers for emphasis, strong, strikethrough, inline code, ATX headings, blockquotes, links, and autolinks, with content marks kept on active lines; verify tests asserting decorations for each construct on active and inactive lines, and that the document text is unchanged
- [ ] 3.3 Render heading sizes (ATX and setext, setext underline hidden off the active line), blockquote bar, bullet widgets for unordered list markers, and horizontal-rule widgets; verify tests for each scenario in the rendered-elements requirement
- [ ] 3.4 Ensure tables, images, reference links, footnotes, and HTML are left as source and nothing is ever rendered as HTML or fetched; verify tests that an HTML snippet and an image reference produce no element or decoration beyond source styling, and a manual check in the browser network panel that opening a note with a remote image makes no request
- [ ] 3.5 Add a test that copying a rendered selection yields markdown source; verify it passes

## 4. Interactive elements

- [ ] 4.1 Implement the checkbox widget for task markers with pointer-event toggling that dispatches a local transaction without moving the selection; verify tests that a toggle changes only the marker character, keeps the selection, is undone by one undo, and produces a Yjs update
- [ ] 4.2 Implement the toggle-task command on `Mod-Enter` at precedence above the vim keymap, covering checked, unchecked, and plain list items across a multi-line selection; verify tests with vim off and with vim on in normal and insert mode
- [ ] 4.3 Add `openExternal(url)` to the platform adapter (opener plugin on Tauri, `window.open` with `noopener,noreferrer` in the browser) and the scheme allowlist; verify unit tests that `http`, `https`, and `mailto` pass and `javascript:`, `file:`, `data:`, and unparsable URLs are rejected without calling the platform
- [ ] 4.4 Implement link activation: plain click edits, Mod-click opens, touch tap on an inactive line opens; verify tests on the event handlers and a manual check in the desktop app and browser
- [ ] 4.5 Add tag pill marks reusing the existing tag scanner ranges; verify tests with the `tags` spec examples (single, nested, multi-word, `#42`, code span, URL fragment, heading marker) and an edit inside a pill

## 5. Code blocks

- [ ] 5.1 Configure markdown with `@codemirror/language-data` code languages and a theme-variable `HighlightStyle`; hide fence lines off the active line and style block background; verify tests that known, unknown, and missing info strings render without error and that markdown and tags inside code get no formatting, and that `npm run build` emits language grammars as separate chunks

## 6. Typography, focus mode, polish

- [ ] 6.1 Import the self-hosted fonts and add editor typography (72ch measure, centered, ≥16px padding, line height, heading scale); verify `npm run build` output contains the `woff2` files as same-origin assets and a manual check at 360px and wide widths in both themes
- [ ] 6.2 Implement the `focusMode` compartment that dims all top-level blocks except the one holding the main cursor; verify tests for the dim-other-paragraphs and follow-the-cursor scenarios
- [ ] 6.3 Add smooth cursor transitions disabled under `prefers-reduced-motion`, and the active-line background; verify manually with the OS reduced-motion setting on and off
- [ ] 6.4 Implement source view by reconfiguring the `livePreview` compartment with scroll preserved, plus keyboard commands and editor view-options controls for source view and focus mode bound to the preferences store; verify tests that switching keeps selection, undo history, and vim mode, and that the preference applies to the next opened note

## 7. Integration checks

- [ ] 7.1 Verify vim with live preview end to end: motions over hidden markers match source view, `V`+`j` reveals both lines, and `:w` still works; covered by Vitest tests plus a manual pass in the desktop app
- [ ] 7.2 Verify remote edits: with two clients on one note, edits on another device re-render the affected lines without moving the local cursor or changing the revealed line; manual check desktop plus browser, plus a test applying a Yjs update to a bound editor
- [ ] 7.3 Run a performance check on a 5,000-line note (decorations limited to the viewport, no noticeable typing lag) and a CJK IME composition check on a line with markup; verify by reporting the measured results in the apply summary
- [ ] 7.4 Run `npm run lint`, `npm run format:check`, `npm run check`, `npm test`, and `npm run build` in `web/`, then `scripts/check.sh`; verify all exit 0
- [ ] 7.5 Run `openspec validate editor-live-preview --strict` and verify it passes

## Workflow follow-up

- Review the change (code-reviewer, plus security-analyst for the opener capability and link handling), then archive it with `/opsx:archive` so `note-editor` gains the new requirements and `appearance` lands in `openspec/specs/`.
- Update `TODO.md`: mark `editor-live-preview` as `[x]` once archived.
