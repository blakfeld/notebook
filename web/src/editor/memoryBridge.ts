import * as Y from 'yjs'
import type { NoteDocBridge, OpenNoteDoc } from './bridge'

/**
 * A {@link NoteDocBridge} with a plain `Y.Doc` per note standing in for the
 * core. For tests and the placeholder page; it is not a platform, and it
 * never encrypts or stores anything.
 */
export interface MemoryBridge extends NoteDocBridge {
  /** The note's body as the stand-in core has it. */
  body(noteId: string): string
  /** Simulates an edit to the note made on another device. */
  remoteEdit(noteId: string, edit: (body: Y.Text) => void): void
  /** While true, every push rejects. */
  failPushes: boolean
  /** Pushes accepted so far, across all notes. */
  readonly pushes: number
  /** Editors currently attached to the note. */
  attached(noteId: string): number
  /** The stand-in core's document for the note. */
  doc(noteId: string): Y.Doc
}

/** The origin of {@link MemoryBridge.remoteEdit} transactions. */
const OTHER_DEVICE = Symbol('other device')

/** Creates a memory bridge holding `notes`, keyed by note id. */
export function memoryBridge(notes: Record<string, string> = {}): MemoryBridge {
  const docs = new Map<string, Y.Doc>()
  const sessions = new Map<string, number>()
  let pushes = 0

  const docFor = (noteId: string): Y.Doc => {
    let doc = docs.get(noteId)
    if (!doc) {
      if (!(noteId in notes)) {
        throw new Error(`note ${noteId} not found`)
      }
      doc = new Y.Doc()
      doc.getText('body').insert(0, notes[noteId])
      docs.set(noteId, doc)
    }
    return doc
  }

  const bridge: MemoryBridge = {
    failPushes: false,
    get pushes() {
      return pushes
    },

    async open(noteId): Promise<OpenNoteDoc> {
      const doc = docFor(noteId)
      // Each attached editor is its own origin, so its pushes aren't
      // echoed back to it.
      const self = Symbol('editor')
      const listeners = new Set<(update: Uint8Array) => void>()
      const onUpdate = (update: Uint8Array, origin: unknown) => {
        if (origin !== self) {
          for (const listener of listeners) listener(update)
        }
      }
      doc.on('update', onUpdate)
      sessions.set(noteId, (sessions.get(noteId) ?? 0) + 1)
      let open = true
      return {
        initialState: Y.encodeStateAsUpdate(doc),
        async push(update) {
          if (!open) throw new Error('editor detached')
          if (bridge.failPushes) throw new Error('push failed')
          Y.applyUpdate(doc, update, self)
          pushes++
        },
        onRemote(cb) {
          listeners.add(cb)
          return () => listeners.delete(cb)
        },
        async close() {
          if (!open) return
          open = false
          listeners.clear()
          doc.off('update', onUpdate)
          sessions.set(noteId, (sessions.get(noteId) ?? 1) - 1)
        },
      }
    },

    body: (noteId) => docFor(noteId).getText('body').toString(),
    remoteEdit(noteId, edit) {
      const doc = docFor(noteId)
      doc.transact(() => edit(doc.getText('body')), OTHER_DEVICE)
    },
    attached: (noteId) => sessions.get(noteId) ?? 0,
    doc: docFor,
  }
  return bridge
}
