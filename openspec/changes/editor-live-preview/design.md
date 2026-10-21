# Design

## Context

See proposal.md for motivation. This change builds on the CodeMirror 6 editor delivered by `note-editor-component`. That editor already has `@codemirror/lang-markdown` (Lezer markdown parser), a Bear-style tag highlighter that mirrors the core grammar in `crates/notebook-core/src/tags/parse.rs`, a `y-codemirror.next` binding to the note's Yjs document, undo scoped to local edits, vim mode in a compartment, and device-local settings through `web/src/platform/`. Live preview layers presentation on top. It never changes how text reaches the CRDT.

Constraints that shape the approach:

- **CSP.** The Tauri CSP is `default-src 'self'` with `style-src 'self' 'unsafe-inline'`. The server CSP (from `server-hosting`) allows scripts, styles, and connections only from its own origin. Fonts, code-highlighting grammars, and theme CSS must ship as same-origin bundled assets. No remote fonts, no `eval`.
- **E2EE privacy.** Opening a note must not reveal anything to a third party. That rules out loading images or link previews referenced in note text.
- **One frontend** for desktop, browser, and mobile. UI code may not import `@tauri-apps/api` outside `web/src/platform/`.
- `app.css` already defines `--text`, `--muted`, `--bg` with a `prefers-color-scheme` media query. This change replaces that with a token set that also supports a manual override.

## Goals / Non-Goals

**Goals:**
- Live preview as an isolated set of CM6 extensions in one compartment, so source view is just "compartment empty".
- Rendering cost proportional to the visible viewport, not the note length.
- One theme implementation (CSS custom properties) that drives both the editor and the app chrome.

**Non-Goals:**
- Rendering tables, images, footnotes, reference-style links, math, or HTML (they stay as source; see the `note-editor` spec).
- Tag click-to-navigate, link previews, or link autocompletion.
- Remote collaborator cursors or presence.
- A separate reading-only mode. Live preview is always editable.
- Custom user themes or font selection. Two built-in themes only.
- The mobile keyboard toolbar (owned by `mobile-app`).

## Decisions

### 1. Decorations from the Lezer syntax tree in a ViewPlugin, limited to visible ranges

A `ViewPlugin` walks `syntaxTree(state)` over `view.visibleRanges` and builds a `DecorationSet` of `Decoration.mark` (styling), `Decoration.replace` (hiding markers), and `Decoration.widget`/replace-with-widget (checkboxes, bullets, horizontal rules). It rebuilds on `docChanged`, `selectionSet`, `viewportChanged`, or when the syntax tree advances (`syntaxTree(update.state) !== syntaxTree(update.startState)`).

All hidden ranges are within a single line (inline markers, ATX heading markers, blockquote `>`, the whole text of a fence line or setext underline, leaving an empty styled line). Because no replace spans a line break, a ViewPlugin is allowed to provide them. Line-level styling (heading size, blockquote bar, code block background, focus dimming) uses `Decoration.line`.

*Alternatives:* a `StateField` would let decorations span lines, which is needed to collapse fence lines entirely, but it costs a full-document walk on every change and loses the viewport limit. Collapsing to an empty styled line is visually close enough. A markdown-to-HTML renderer (preview pane) was rejected because it is a second view, not live preview, and would need HTML sanitizing.

### 2. Reveal by line, from every selection range

A line is "active" when any `state.selection.ranges` range touches it (from `lineAt(range.from)` through `lineAt(range.to)`). On active lines, marker `replace` decorations are swapped for a `cm-md-syntax` mark that dims the markers. Content marks such as strong and heading size stay. Checkbox, bullet, and rule widgets are not drawn on active lines.

*Alternatives:* Obsidian-style per-node reveal (only the construct under the cursor) is finer but causes more horizontal jitter while moving within a line and is harder to keep predictable with vim motions. Per-line reveal is simple to specify and test. Reveal follows only the local selection. `y-codemirror.next` maps the selection through remote changes, so the active line set follows the text automatically.

### 3. Hidden ranges are not atomic

Hidden markers are not registered with `EditorView.atomicRanges`. A cursor can only enter a line by making it active, and then the markers are visible. So no motion ever lands "inside" an invisible range, and vim motions (which operate on document positions via `@replit/codemirror-vim`) count characters exactly as in source view. Vertical motion (`j`/`k`, arrow keys) uses CM's pixel goal column. The target line reveals after the move and can shift horizontally. This is the accepted live-preview trade-off.

### 4. Checkboxes as widgets that dispatch ordinary transactions

`[ ]`/`[x]` (the GFM `TaskMarker` node) is replaced by a `CheckboxWidget` (`<input type="checkbox">` with `aria-label`). It handles `pointerdown` (so mouse, pen, and touch all work), calls `preventDefault()` so the selection doesn't move, and dispatches `{changes: {from: markerPos + 1, to: markerPos + 2, insert: checked ? ' ' : 'x'}, userEvent: 'input.toggle-task'}`. This is a plain local transaction, so it flows through the Yjs binding, is captured by the local `Y.UndoManager`, and syncs like typing. `ignoreEvent()` returns false only for the handled events.

The toggle-task command is bound to `Mod-Enter` in the live-preview keymap (CM's default `insertBlankLine` on `Mod-Enter` is low value in prose). It is registered at higher precedence than the vim keymap so it works in vim normal and insert modes. It edits the same character, or inserts `[ ] ` after a bullet or ordered list marker.

### 5. Links: Mod-click and touch tap via the platform adapter

Link text gets a `cm-md-link` mark carrying the URL in a data attribute. A `domEventHandlers` `mousedown` handler opens the link when Mod is held. A `touchend` handler without movement opens a link on a non-active line. Opening goes through a new platform adapter function `openExternal(url: string)`:

- **Validation happens in the frontend before the call:** `new URL(url)` must parse and `protocol` must be `http:`, `https:`, or `mailto:`. Anything else is a no-op.
- **Tauri:** `@tauri-apps/plugin-opener` `openUrl`, with the `opener:allow-open-url` permission scoped to `http://*`, `https://*`, and `mailto:*` in `crates/notebook-app/capabilities/default.json`. The scope enforces the same allowlist host-side.
- **Browser:** `window.open(url, '_blank', 'noopener,noreferrer')`.

*Alternative:* plain `<a href>` elements inside the editor were rejected. They would navigate the Tauri webview itself, and a click would fight with cursor placement.

### 6. Tags reuse the existing scanner

The tag highlighter from `note-editor-component` already produces tag ranges matching core rules (and skips code). Live preview reuses that scanner's ranges and adds a `cm-tag-pill` mark. It does not add a second grammar. Pills are marks only, so the text stays editable. `#` stays visible, as in Bear.

### 7. Code blocks via `@codemirror/language-data`

`markdown({ codeLanguages: languages })` from `@codemirror/language-data` gives nested parsing for fenced blocks. Grammars load lazily via dynamic `import()`, which Vite emits as same-origin hashed chunks (allowed by `script-src 'self'`). Unknown info strings fall back to no nested parser. Highlighting uses a `HighlightStyle` whose colors are CSS variables, so it follows the theme. Inline-format decorations skip nodes inside `FencedCode`/`CodeBlock`/`InlineCode`.

### 8. Theme as CSS custom properties keyed by `data-theme`

`web/src/theme/tokens.css` defines all color tokens (text, muted, bg, surface, accent, link, tag-pill bg/fg, code bg, selection, syntax-marker, dim, focus-ring, plus highlight colors) under `:root[data-theme='light']` and `:root[data-theme='dark']`. A small `theme.ts` resolves the preference (`system | light | dark`) against `matchMedia('(prefers-color-scheme: dark)')`, sets `document.documentElement.dataset.theme`, and listens for media changes. The CM theme (`EditorView.theme`) and `HighlightStyle` reference only `var(--…)`, so a theme switch needs no editor reconfiguration.

Preventing a flash of the wrong theme: an inline `<script>` in `index.html` is forbidden by CSP. Instead `main.ts` resolves and applies the theme synchronously before mounting the app. The pre-mount default comes from `@media (prefers-color-scheme)` rules on `:root:not([data-theme])`, so System users are correct from first paint. On Tauri, the preference is read through the adapter before mount (the read is async, so mount waits on it). Explicit Light/Dark users therefore never see the other theme.

*Alternative:* two separate CM themes swapped through a compartment. Rejected because the app chrome needs the same tokens anyway, and CSS variables switch without touching editor state.

### 9. Preferences through the existing device-local settings

`theme`, `editor.sourceView`, and `editor.focusMode` are stored through the same device-local settings API that `note-editor-component` introduced for vim mode. On native that is app-local settings. In the browser it is `localStorage`, which `browser-web-client` permits for non-sensitive UI preferences. A small Svelte store (`web/src/lib/preferences.ts`) exposes them reactively. Read failures fall back to defaults.

### 10. Compartments for source view and focus mode

The live-preview extensions (decoration plugin, checkbox and link handlers, toggle-task keymap, preview theme rules) live in a `livePreview` compartment. Source view reconfigures it to `[]`. Focus mode is a separate `focusMode` compartment holding a ViewPlugin that adds `cm-dimmed` line decorations to lines outside the top-level block (direct child of `Document` in the syntax tree) that holds the main cursor. Reconfiguring a compartment keeps doc, selection, history, and vim state, which the source-view requirement needs. Scroll position is preserved by capturing `view.scrollSnapshot()` before reconfigure and dispatching it after, since line heights change.

The toggle-task command stays available in source view, and focus mode works in either view.

### 11. Typography and fonts

Self-hosted variable fonts via `@fontsource-variable/inter` (prose) and `@fontsource-variable/jetbrains-mono` (code), both SIL OFL, imported as CSS so Vite bundles the `woff2` files as same-origin hashed assets with `font-display: swap`. Fallback stacks end in `system-ui` and `ui-monospace` for scripts the fonts don't cover (e.g. CJK). The editor content uses `max-width: 72ch; margin-inline: auto; padding-inline: max(16px, …)` with `line-height: 1.6`. Heading scale is a fixed ratio (H1 1.75em down to H6 1em with weight and color distinguishing H5 and H6).

### 12. Cursor and active line

`drawSelection()` (already present for vim) draws the cursor. A CSS `transition` on `.cm-cursor` `left`/`top` (~80ms) gives smooth movement and is disabled under `@media (prefers-reduced-motion: reduce)`. `highlightActiveLine()` with a low-alpha `--active-line` token gives the active-line treatment.

## Risks / Trade-offs

- [Horizontal jitter when the cursor enters a line and markers appear] → Accepted. Markers are dimmed so the shift reads as intentional. Per-line reveal keeps it to vertical moves only.
- [Decoration rebuild cost on every selection change in long notes] → Viewport-limited walk. Add a Vitest benchmark-style test that builds decorations for a 5,000-line note and asserts it touches only visible ranges. Manually check typing latency on a large note.
- [Lezer parse lags on very large notes, so far-away text renders as source until parsed] → Acceptable. Decorations update when the tree advances (Decision 1).
- [IME composition inside a line whose decorations change mid-composition can break input] → The reveal set only changes on selection moves to another line, not during composition on the same line. Include a manual QA step with a CJK IME.
- [CodeMirror's injected styles vs. a strict server CSP without `'unsafe-inline'` styles] → Live preview adds only class-based rules through `EditorView.theme`, the same mechanism the base editor already uses. Whatever `note-editor-component`/`browser-web-client` settle for CM style injection (`EditorView.cspNonce` or constructable stylesheets) covers this change too. Fonts need `font-src` to permit `'self'`, which `default-src 'self'` does.
- [`language-data` grammars add bundle weight] → They are lazy chunks, loaded only when a note contains that language.
- [Opener plugin widens the Tauri capability surface] → Scoped to `http`, `https`, and `mailto` host-side, validated again in the frontend, and invoked only on explicit user gesture.
- [Vim plugin's own `Mod-Enter` or visual-mode handling may conflict] → Keymap precedence set explicitly. Covered by a test that runs the command with vim enabled.

## Migration Plan

Frontend-only plus one Tauri plugin. Live preview becomes the default view for existing users on upgrade. No data migration, since note text is untouched. Rollback is reverting the frontend build. Stored preference keys are simply ignored by older builds.

## Open Questions

- Exact heading scale, accent colors, and pill styling will be tuned during visual QA (with the `ui-designer` agent). These only change token values, not structure.
