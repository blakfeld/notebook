# Design

## Context

See proposal.md (Why). The frontend in `web/` is a Vite + Svelte 5 + TypeScript SPA with no SvelteKit, a `src/lib/` and `src/platform/` split, Vitest, ESLint, Prettier, and `svelte-check`, all run by `scripts/check.sh` and CI. Today `src/platform/index.ts` only calls one Tauri command. The same bundle runs in the Tauri webview (CSP `style-src 'self' 'unsafe-inline'`) and is served by `notebook-server`, whose CSP is stricter: `script-src 'self' 'wasm-unsafe-eval'; style-src 'self'`.

`crdt-note-documents` (must land first) defines `NoteDocument` in `notebook-core`: one Yrs doc per note with root `body: Text` and `meta: Map`, UTF-16 offsets, Yjs update v1 encoding behind a format byte, an owned `NoteKey`, and mutators that only ever return `EncryptedUpdate`s. Its design explicitly leaves to this change how the editor's live document crosses the Tauri/WASM boundary, and says a plaintext path between core and editor gets added here, deliberately.

The tag rules live in `notebook-core::tags::parse` (pulldown-cmark for code exclusion, then a hand-written scanner) and are specified in `openspec/specs/tags/spec.md`. Requirements for this change are in `specs/note-editor/spec.md` and `specs/note-documents/spec.md`.

## Goals / Non-Goals

**Goals:**
- One editor component and one bridge contract that every client implements, with no platform checks inside the editor.
- Keys and encryption stay in the core; the editor only ever sees plaintext the user is already looking at.
- A boundary that `editor-live-preview` and `mobile-app` can build on without reworking the binding: they add extensions and commands, not a new data path.

**Non-Goals:**
- The Tauri commands/events and the WASM bindings that implement the bridge (`desktop-app-shell`, `browser-web-client`). This change fixes their contract and the core methods they call.
- How `Replica` loads, persists, and syncs documents (`sync-client-engine`, `local-sqlite-storage`), and the `Workspace` pass-throughs and Tauri commands/WASM bindings over the replica's editor entry points (`desktop-app-shell`, `browser-web-client`).
- Live preview, themes, typography, task checkbox toggling (`editor-live-preview`), the touch toolbar (`mobile-app`), and the settings screen that hosts the vim toggle (`desktop-app-shell`).
- Remote cursors/presence. It's one user across devices; awareness is passed as `null`.
- Editing metadata (pin/archive/trash) from the editor.

## Decisions

### The core owns the document; the editor holds a Yjs replica

While a note is open there are two replicas of its CRDT: the authoritative `NoteDocument` in the core (Rust/Yrs) and a `Y.Doc` in the webview bound to CodeMirror by `y-codemirror.next`. They sync by exchanging plaintext Yjs v1 updates over the bridge. Yrs and Yjs share that encoding, so no translation is needed.

- The core keeps the `NoteKey`, does all encryption, and is the only place updates go to storage and sync. The plaintext crossing the bridge is the note text the webview is already rendering, so the trust boundary doesn't move. With Tauri it crosses IPC into the webview; with WASM it never leaves the tab.
- Because both sides are CRDT replicas, ordering, duplicates, and races between a local push and a remote update don't matter: every exchange is idempotent and converges.

**Alternative considered: the core owns the only replica, the editor sends splices.** The editor would translate each CodeMirror change into `splice(offset, delete, insert)` calls and the core would push back text deltas. That means hand-writing what `y-codemirror.next` already does (position mapping across concurrent changes, local-only undo via `Y.UndoManager`), plus an offset contract in a public Rust API. Rejected.

**Alternative considered: the editor owns the document, and the core only encrypts.** The webview would hold the authoritative `Y.Doc` and ask the core to encrypt its updates. That splits the "only ciphertext leaves the document" rule across two languages, needs a separate path for core-side edits (tag rename, sync) to reach a closed note, and breaks for notes that aren't open. Rejected.

### Core editor exchange API (`notebook-core`, extends `NoteDocument`)

```rust
/// Plaintext Yjs v1 update for the in-device editor only. Redacted Debug,
/// no conversion to or from EncryptedUpdate/EncryptedSnapshot.
pub struct EditorUpdate(Vec<u8>);          // from_bytes / as_bytes / into_bytes
/// What the core believes the attached editor replica already has.
pub struct EditorSync { /* yrs StateVector */ }

impl NoteDocument {
    /// Full current state for a new editor replica, plus its sync tracker.
    pub fn editor_attach(&self) -> (EditorUpdate, EditorSync);
    /// Apply an update made in the editor. Returns the encrypted update for
    /// storage/sync (None if nothing changed). Sets meta.updated = now when
    /// the body changed.
    pub fn apply_editor_update(&mut self, sync: &mut EditorSync, update: &EditorUpdate, now: Timestamp)
        -> Result<Option<EncryptedUpdate>, DocumentError>;
    /// Everything the editor replica lacks (remote updates, core-side edits,
    /// the meta.updated write). None if nothing.
    pub fn editor_pending(&self, sync: &mut EditorSync) -> Option<EditorUpdate>;
}
```

- No format byte on `EditorUpdate`: it never persists, and the core and frontend ship together, so they can't disagree on the version. The format byte remains on everything encrypted.
- `apply_editor_update` decodes first (`Update::decode_v1`) and fails with `DocumentError::Malformed` before touching the doc, as `apply_update` does. It encodes the diff between the state vector before and after the transaction(s) and encrypts it as one `CrdtUpdate`, so the body change and the `updated` bump travel together.
- `EditorSync` tracks a state vector. `editor_pending` encodes the diff from it and advances it to the doc's current state vector. After `apply_editor_update` the tracker is merged with the update's own state vector, so the editor's edit isn't echoed back (an echo would be harmless anyway).
- Multiple editors on one note (two windows) each get their own `EditorSync`; nothing here assumes one.
- These `NoteDocument` methods are building blocks for `Replica` only. No host calls them directly (next section).

The `crdt-note-documents` spec says a document "SHALL never emit plaintext updates or snapshots." This change adds a typed, editor-only exception (see `specs/note-documents/spec.md`). The two should be reconciled at archive time by narrowing that sentence to "never hands plaintext to storage or sync"; see Open Questions.

### Host path: every editor edit goes through `Replica`

`sync-client-engine`'s `Replica` owns every `NoteDocument` and performs every edit, writing the encrypted update and its outbox entry in one storage transaction. The editor path follows the same rule. This change adds editor entry points to `Replica` (`notebook-core::sync::replica`), which keeps the `EditorSync` trackers itself so hosts never touch a `NoteDocument`:

```rust
pub struct EditorSessionId(u64);            // per attached editor, never reused in a process

impl Replica {
    pub fn editor_attach(&mut self, note: NoteId) -> Result<(EditorSessionId, EditorUpdate)>;
    /// NoteDocument::apply_editor_update, then in one storage transaction store
    /// the EncryptedUpdate and add an outbox entry, then wake the engine.
    /// Ok(false) when the update changed nothing.
    pub fn apply_editor_update(&mut self, session: EditorSessionId, update: &EditorUpdate, now: Timestamp) -> Result<bool>;
    /// What this editor lacks: remote updates, core-side edits, the `updated` bump.
    pub fn editor_pending(&mut self, session: EditorSessionId) -> Result<Option<EditorUpdate>>;
    pub fn editor_detach(&mut self, session: EditorSessionId);
}
```

- `apply_editor_update` reuses the replica's existing local-edit write path, so storage failure handling is the same as `set_body`'s: the in-memory document is reloaded from storage and the call fails. The editor keeps its pending edits and resends them (spec: "Failed delivery does not lose edits"); resending is idempotent.
- A session is bound to its note; an unknown or detached session, or a purged note, is an error. Detach also happens implicitly when the note is purged.
- Rules above the replica stay where `desktop-app-shell` puts them: `Workspace` rejects body changes to trashed notes before calling `Replica::apply_editor_update`, records the note id as locally changed, and updates the tag tree and search index, exactly as for its other edits.
- Remote changes reach the editor through the existing notifications. When `Workspace`'s change stream reports a note id (from `SyncEvent::NotesChanged` or from its own edits, such as `rename_tag`), the host calls `editor_pending` for every session attached to that note and forwards non-empty results as the editor's remote-update event. The editor's own push also produces a local-change notification, which is how the `updated` bump comes back.

**Alternative considered: hosts call `NoteDocument` editor methods directly.** That would bypass the replica's single-transaction write and outbox entry, which is the guarantee `sync-client-engine` relies on. Rejected.

### Bridge contract (TypeScript)

`web/src/editor/bridge.ts`:

```ts
export interface NoteDocBridge {
  /** Attach an editor to a note. Resolves with the full state. */
  open(noteId: string): Promise<OpenNoteDoc>
}
export interface OpenNoteDoc {
  readonly initialState: Uint8Array            // Yjs v1 update
  push(update: Uint8Array): Promise<void>      // local edits → core
  onRemote(cb: (update: Uint8Array) => void): () => void  // core → editor
  close(): Promise<void>
}

// web/src/platform/settings.ts (shared by every device-local preference)
export interface DeviceSettings {
  get(key: string): Promise<string | null>
  set(key: string, value: string): Promise<void>
}
```

- A platform implements `open` with `Replica::editor_attach` (via `Workspace`), `push` with `Replica::apply_editor_update`, `onRemote` by calling `Replica::editor_pending` on each change notification for that note, and `close` with `Replica::editor_detach`. It never calls `NoteDocument` directly.
- Bytes are `Uint8Array` end to end. For Tauri, `desktop-app-shell` should send them as raw IPC bodies (`tauri::ipc::Request` / `Response` with binary payloads) rather than JSON number arrays; that's its call, not this contract's.
- `DeviceSettings` is a string key-value store for non-sensitive, device-local preferences, defined in `web/src/platform/settings.ts` rather than in the editor so other preferences reuse it (`editor-live-preview` stores `theme`, `editor.sourceView`, and `editor.focusMode` there). Keys are namespaced strings; values are strings. A missing key or a failed read means "use the default"; a failed write is reported through `onError` and the in-memory value still applies for the session. The editor uses the key `editor.vimMode` with values `"true"`/`"false"`; absent means off. `web/src/platform/settings.ts` also exports `memorySettings()` for tests. Native shells back it with the `local-storage` device-local settings; the browser backs it with `localStorage`, which `browser-web-client` already allows for non-sensitive UI preferences.
- `web/src/editor/memoryBridge.ts` is the reference implementation: a pure-JS `Y.Doc` standing in for the core, with a `remoteEdit(noteId, fn)` test hook that simulates another device. It's used by unit tests and by the placeholder `App.svelte`, which mounts the editor on it with a sample note until `desktop-app-shell` replaces that page; it's not a platform.

**Alternative considered: put the bridge on the existing `src/platform/` module.** The platform adapter abstraction belongs to `browser-web-client`/`desktop-app-shell` and doesn't exist yet. Defining the bridge next to the editor and letting the platform layer implement it keeps this change from designing the adapter for them. `DeviceSettings` is the exception: it isn't editor-specific, so it lives in `src/platform/` where every preference consumer can find it.

### Delivery: coalesced pushes with explicit flushes

Each local `Y.Doc` transaction (origin is the y-codemirror binding, not `REMOTE`) appends its update to a pending list. Pending updates are merged with `Y.mergeUpdates` and pushed after 300 ms of idle, capped at 1 s since the first pending edit, so continuous typing still lands within a second. `flush()` pushes immediately and runs on note switch, `close`, `:w`, `visibilitychange` to hidden, and `pagehide`.

- One push per burst keeps IPC traffic and the number of encrypted updates (each one a sync op and outbox row) low, without hurting cross-device latency much since sync adds its own delay.
- A failed push keeps its updates at the head of the pending list (they merge with later ones), raises `onError`, and retries on the next timer or flush. Nothing is dropped.
- Remote updates are applied with `Y.applyUpdate(doc, u, REMOTE)`. That origin is excluded from the pending list and from the undo manager's tracked origins.

**Alternative considered: push every transaction immediately.** Simplest, but turns each keystroke into an IPC round trip, an encryption, and an outbox entry. Rejected; snapshots would compact it eventually, but there's no benefit.

### Editor composition

`web/src/editor/` holds a framework-free core (`createNoteEditor(parent, options) → NoteEditorHandle`) and a thin Svelte 5 wrapper (`NoteEditor.svelte`, props `bridge`, `noteId`, `vimMode`, `onError`). The handle exposes `view` (the `EditorView`), `undo()`, `redo()`, `flush()`, `focus()`, `setVimMode(on)`, `openNote(id)`, and `destroy()`. `mobile-app`'s toolbar and `editor-live-preview` use these instead of reaching into the binding.

Extensions, in order:
- `markdown({ base: markdownLanguage, codeLanguages: [] })` plus `syntaxHighlighting` with a minimal default highlight style. No code-language packages yet; `editor-live-preview` adds them.
- `yCollab(ydoc.getText('body'), null, { undoManager })` with a `Y.UndoManager` scoped to that text and tracking only the binding's origin. The editor binds only the `body` root and never writes `meta`; root names `body` (`Y.Text`) and `meta` (`Y.Map`) and UTF-16 offsets come from `crdt-note-documents` and match the JS string indices CodeMirror and Yjs use, so no offset translation exists anywhere on the path. CodeMirror's own `history()` is not installed (two undo stacks would fight). `yUndoManagerKeymap` supplies Mod-z / Mod-Shift-z / Mod-y.
- `search({ top: true })` with `searchKeymap` (find, replace, case, whole word, regex are built in).
- `EditorView.lineWrapping`.
- `keymap.of(markdownKeymap)` (`insertNewlineContinueMarkup`, `deleteMarkupBackward`), which handles bullets, ordered numbering, task items (continued as `- [ ] `), and ending the list on an empty item.
- The tag highlighter (below), `defaultKeymap`, `indentWithTab`.
- A `Compartment` holding `vim()` or `[]`. `setVimMode` reconfigures it, which keeps the doc, selection, and undo manager. `vim()` is placed first so its keymap wins.

Opening a note creates a fresh `Y.Doc`, applies `initialState` with origin `REMOTE`, creates a new undo manager, and replaces the view's state with `view.setState(...)` using the current vim setting. The previous note is flushed and closed first, so switching never mixes two docs.

### Read-only mode

`desktop-app-shell` opens trashed notes read-only. The handle gets `setReadOnly(on)` (and the Svelte prop `readOnly`), a `Compartment` holding `EditorState.readOnly.of(true)` plus `EditorView.editable.of(false)`. Remote updates still apply; local input, find-and-replace edits, and vim editing commands are blocked. The core check in `Workspace` remains the real guard.

### Vim ex commands

`Vim.defineEx('write', 'w', …)` calls `flush()` and shows "Saved" through the vim status/notification panel. `wq`, `x`, `quit`/`q` are redefined to flush and do nothing else; closing notes is the app shell's job. Default `@replit/codemirror-vim` behavior for these would otherwise be a no-op or an error message.

### Tag highlighting: TypeScript port checked against a shared fixture

A `ViewPlugin` scans visible line ranges with `findTagRanges(lineText, excluded)`, a TS port of `tags::parse::scan_tag` and its boundary and trailing-punctuation rules, and marks hits with `Decoration.mark({ class: 'cm-tag' })`. Code exclusion comes from the Lezer markdown syntax tree (`InlineCode`, `FencedCode`, `CodeBlock` nodes), which is the CommonMark structure pulldown-cmark also follows. Tags never span lines, so line-by-line scanning is exact.

Consistency with the Rust parser is enforced by a fixture: `crates/notebook-core/tests/fixtures/tag_cases.json`, a list of `{ "body": ..., "tags": ["#work", "#side project#", ...] }` where `tags` are the occurrence source texts in order. A Rust test asserts `parse(body)` yields those texts; a Vitest test asserts the editor's highlighted spans yield the same. Seed it with every scenario in `openspec/specs/tags/spec.md`. Comparing source texts avoids a UTF-8 vs UTF-16 offset mismatch.

**Alternative considered: ask the core for tag ranges.** Exact by construction, but async over IPC on every keystroke and only synchronous in the WASM build. Rejected; the fixture gives the same guarantee for the cases we know about.

**Alternative considered: a Lezer inline-parser extension for tags.** It would put tags in the syntax tree, which `editor-live-preview` might like, but Lezer inline parsers see emphasis-split content and make multi-word tags and the "preceded by whitespace" rule awkward. A decoration is enough here, and live preview can read the same `findTagRanges` output.

### CSP

CodeMirror injects its styles through `style-mod`. On a document root it adds `<style>` elements, which `style-src 'self'` blocks; it uses constructable stylesheets (`adoptedStyleSheets`), which CSP `style-src` doesn't govern, only when the root is a shadow root. So `createNoteEditor` mounts the view in an open shadow root on a `.note-editor-host` element it adds to `parent`, passing that root as the view's `root`. Page CSS can size the host but can't reach `.cm-*` elements inside it; editor styling goes through `EditorView.theme` and `baseTheme`, which `editor-live-preview` uses anyway. Inherited properties (font, color) and CSS custom properties still cross the boundary.

**Alternatives considered:** `EditorView.cspNonce` needs a per-request nonce, and the server serves the index page as a static file. Adding `'unsafe-inline'` to the server's `style-src` weakens the policy for the whole app. Both rejected.

Inline `style=""` attributes that CodeMirror sets through `setAttribute` (for example `tab-size` on the content element) are blocked under `style-src 'self'`; they're cosmetic. A task verifies the editor in a production build under the server's CSP and records any blocked attributes; if one matters, the editor sets it through a theme instead.

### Dependencies

`@codemirror/state`, `@codemirror/view`, `@codemirror/commands`, `@codemirror/language`, `@codemirror/lang-markdown`, `@codemirror/search`, `@replit/codemirror-vim`, `yjs`, `y-codemirror.next` as runtime dependencies; `jsdom` as a dev dependency with Vitest's `environment: 'jsdom'` for editor tests. Exact versions are pinned by `package-lock.json`; one copy of `@codemirror/state` and `yjs` must be in the tree (duplicates break `instanceof` checks), verified with `npm ls`.

## Risks / Trade-offs

- [Two replicas in memory per open note] → Notes are small text; cost is negligible. Only open notes have an editor replica.
- [Plaintext crosses Tauri IPC] → It's text the webview displays anyway; keys never cross. The `EditorUpdate` type keeps it out of storage/sync APIs at compile time.
- [TS tag scanner drifts from Rust] → Shared fixture runs in both test suites; any spec change to tags must add fixture cases. Drift on cases not in the fixture only affects highlighting, never stored data.
- [Up to 1 s of typed text lost on a hard crash] → Flushes on hide/pagehide/switch cover normal exits. Matches the spec's one-second bound.
- [`y-codemirror.next` and `@replit/codemirror-vim` both register key handlers] → Vim extension first in precedence; tests cover undo in vim (`u`, `Ctrl-r`) going through the Yjs undo manager by binding them to `undo()`/`redo()` via `Vim.defineAction`.
- [Vim's `u` uses CodeMirror history by default, which isn't installed] → Remap as above; verified by a test.
- [jsdom lacks layout, so some CodeMirror behavior (wrapping, scrolling) can't be unit tested] → Logic (binding, undo, vim, tags, delivery) is tested in jsdom; wrapping and CSP are checked manually on the placeholder page in dev and in a production build.
- [`EditorSync` state vector misses an edge case and re-sends data] → Re-sent updates are idempotent; correctness only depends on never under-sending, which tests check by converging replicas after random interleavings.

## Migration Plan

None. No editor exists yet. The core additions are new methods; the document format is unchanged.

## Open Questions

- Wording of the `crdt-note-documents` requirement "A document SHALL never emit plaintext updates or snapshots." It should be narrowed (to storage/sync) when either change archives, so the main `note-documents` spec isn't self-contradictory. This doesn't change what gets built here.
