# Proposal

## Why

The note editor must be beautiful. The functional editor from `note-editor-component` shows raw markdown syntax. Writers expect what Bear and Obsidian's Live Preview offer: markdown stays the source of truth, but the page reads like a formatted document. This change makes the editor feel polished without giving up plain-text markdown or vim mode.

## What Changes

- **Live preview rendering** via CodeMirror decorations:
  - Hide markdown syntax (`**`, `_`, `~~`, `#` heading markers, link URLs, `` ` ``) on lines the selection doesn't touch, and reveal it on every line the cursor or a selection range touches.
  - Render headings with scaled typography, emphasis, strong and struck-through text, inline code, blockquotes, bullet lists, horizontal rules, and links.
  - Render task list checkboxes as interactive toggles that edit the underlying `- [ ]` text, plus a keyboard command to toggle the task on the current line.
  - Render tags as styled pills, still editable as text, using the same tag grammar as the existing highlighter.
  - Render fenced code blocks in monospace with language-aware highlighting.
  - Links open in the system browser on Mod-click (or tap on touch), and only for `http`, `https`, and `mailto` URLs. Live preview never fetches remote content: images and raw HTML stay as source text.
- **Typography and layout**: a readable measure (max line width), self-hosted fonts (proportional prose plus monospace code), comfortable line height, and generous margins that shrink on narrow screens.
- **Themes**: light and dark themes for the whole app that follow the OS preference, with a per-device manual override (System / Light / Dark) in Settings.
- **Focus and polish**: optional focus mode that dims every block except the one holding the cursor, smooth cursor movement (off under reduced motion), and a subtle active-line treatment.
- Live preview must coexist with vim mode: vim motions operate on the underlying text, and syntax reveal follows the vim cursor and visual selection.
- A raw source view toggle that turns off all live-preview rendering without reloading the editor. Source view and focus mode are per-device preferences.

## Capabilities

### New Capabilities
- `appearance`: App-wide light and dark themes, following the OS preference with a per-device manual override, applied to the editor and the surrounding UI with readable contrast.

### Modified Capabilities
- `note-editor` (introduced by `note-editor-component`): adds live-preview rendering, syntax reveal around the selection, interactive task lists, link opening rules, tag pills, code block highlighting, typography and measure, focus mode, and a raw source toggle. Added as new requirements only; existing `note-editor` requirements are unchanged.

## Impact

- Frontend (`web/`): CodeMirror 6 view plugins and decorations in the editor module, a theme module using CSS custom properties, preference storage through the existing device-local settings in the platform adapter, and a theme control in the Settings view from `desktop-app-shell`.
- New frontend dependencies: `@codemirror/language-data` (lazily loaded code-block languages) and self-hosted font packages bundled as same-origin assets, so the CSP needs no new origins.
- Platform adapter gains `openExternal(url)`. On Tauri this adds the opener plugin to `notebook-app` with a capability scoped to `http`, `https`, and `mailto` URLs. In the browser it opens a new tab with `noopener`.
- No `notebook-core`, server, storage, or sync changes.
- Depends on `note-editor-component` (must be archived first, since this change adds to `note-editor`) and on `desktop-app-shell` for the Settings view. `mobile-app` relies on live preview working with touch selection and the virtual keyboard.
- Needs visual QA across desktop, browser, and narrow (mobile) widths, in both themes.
