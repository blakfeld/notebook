/**
 * The note document bridge: how the editor reaches a note's CRDT document
 * without knowing whether the core runs behind Tauri IPC or in WASM.
 *
 * The core owns the authoritative document, its key, and all encryption.
 * The editor holds a Yjs replica of the open note and exchanges plaintext
 * updates with the core through this bridge. Every update is a Yjs update
 * in v1 encoding (`Y.encodeStateAsUpdate`, `Y.mergeUpdates`), with no
 * format byte in front: these bytes are never stored, and the core and the
 * editor ship together.
 *
 * A platform implements it over the core's replica:
 *
 * - `open` with `Replica::editor_attach`,
 * - `push` with `Replica::apply_editor_update`,
 * - `onRemote` by calling `Replica::editor_pending` whenever the core
 *   reports that the note changed, and forwarding non-empty results,
 * - `close` with `Replica::editor_detach`.
 *
 * It never calls `NoteDocument` directly, so every edit gets the replica's
 * single storage transaction and outbox entry.
 *
 * Both sides are CRDT replicas, so every exchange is idempotent: an update
 * may be delivered twice, or race with one going the other way, and both
 * sides still converge.
 *
 * The editor delivers local edits within one second of the last keystroke
 * (merged, after 300 ms of idle), and immediately when the note is closed or
 * switched, on vim `:w`, and when the page is hidden or unloaded. A push
 * that fails is kept and retried, so it may later arrive merged with newer
 * edits.
 *
 * Preferences such as vim mode (key `editor.vimMode`, values `"true"` and
 * `"false"`) don't go through the bridge; they live in the device-local
 * {@link DeviceSettings} store.
 *
 * @see DeviceSettings in `src/platform/settings.ts`
 */
export interface NoteDocBridge {
  /** Attaches an editor to a note. Resolves with the note's full state. */
  open(noteId: string): Promise<OpenNoteDoc>
}

/** One editor attached to one note, from {@link NoteDocBridge.open}. */
export interface OpenNoteDoc {
  /** The note's full state at attach time, as a Yjs v1 update. */
  readonly initialState: Uint8Array
  /**
   * Delivers local edits (a Yjs v1 update) to the core. Resolves once the
   * core has stored them. Rejects if it couldn't; the editor keeps the
   * update and sends it again, unless the rejection is a
   * {@link NoteClosedError}.
   */
  push(update: Uint8Array): Promise<void>
  /**
   * Subscribes to changes the editor lacks: edits from other devices,
   * core-side edits such as a tag rename, and the core's own updated-time
   * writes. Returns an unsubscribe function.
   */
  onRemote(cb: (update: Uint8Array) => void): () => void
  /** Detaches the editor. Call it after the last push. */
  close(): Promise<void>
}

/**
 * A push the core will never accept: the note was deleted or trashed, or
 * the vault locked. The editor drops the update instead of retrying, and
 * detaches.
 */
export class NoteClosedError extends Error {
  /** `cause` is what the core reported, for the host to show. */
  constructor(message: string, cause?: unknown) {
    super(message, { cause })
    this.name = 'NoteClosedError'
  }
}
