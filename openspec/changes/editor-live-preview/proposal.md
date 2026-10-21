# Proposal

## Why

The note editor must be beautiful. The functional editor from `note-editor-component` shows raw markdown syntax. Writers expect what Bear and Obsidian's Live Preview offer: markdown stays the source of truth, but the page reads like a formatted document. This change makes the editor feel polished without giving up plain-text markdown or vim mode.

## What Changes

- **Live preview rendering** via CodeMirror decorations:
  - Hide markdown syntax (`**`, `_`, `#` heading markers, link URLs, `` ` ``) on lines the cursor isn't on, and reveal it on the active line or selection.
  - Render headings with scaled typography, emphasis and strong text, inline code, blockquotes, horizontal rules, and links.
  - Render task list checkboxes as interactive toggles that edit the underlying `- [ ]` text.
  - Render tags as styled pills, still editable as text.
  - Render fenced code blocks with language-aware highlighting.
- **Typography and layout**: a readable measure (max line width), refined font choices (proportional prose plus monospace code), comfortable line height, and generous margins.
- **Themes**: light and dark themes following the OS preference, with a manual override.
- **Focus and polish**: optional focus mode that dims everything but the current paragraph, smooth cursor, and subtle active-line treatment.
- Live preview must coexist with vim mode: vim motions operate on the underlying text, and syntax reveal follows the vim cursor.
- Option to switch to a raw source view.

## Capabilities

### New Capabilities
<!-- None. -->

### Modified Capabilities
- `note-editor` (introduced by `note-editor-component`): adds live-preview rendering behavior, interactive task lists, themes, typography requirements, focus mode, and a raw-source toggle.

## Impact

- Frontend editor package: custom CM6 view plugins and decorations, theme definitions, and font assets (self-hosted to satisfy CSP).
- No core, server, or sync changes.
- Needs visual QA across desktop, browser, and (later) mobile widths.
