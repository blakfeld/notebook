import {
  NoteClosedError,
  type NoteDocBridge,
  type OpenNoteDoc,
} from '../editor/bridge'
import { fromBase64, toBase64 } from './base64'
import type { CoreApi } from './core'
import { errorCode, type EditorRemoteUpdate, type ErrorCode } from './types'

/**
 * Push failures the core will never get over for this session: the
 * notebook isn't open (the session went with it), the note or session is
 * gone, or the note is trashed.
 */
const FINAL = new Set<ErrorCode>(['not_open', 'not_found', 'trashed'])

/**
 * The {@link NoteDocBridge} over the core's editor host commands:
 * `editorOpen`, `editorPush`, `editorClose`, and the `editor-remote-update`
 * event, converting updates between bytes and base64.
 */
export function coreBridge(core: CoreApi): NoteDocBridge {
  return {
    async open(noteId): Promise<OpenNoteDoc> {
      // Subscribe before attaching, and wait until the subscription is live,
      // so nothing sent after the attach is lost. Until the session is
      // known, and until the editor subscribes, updates wait here.
      let session: number | null = null
      let waiting: EditorRemoteUpdate[] = []
      const listeners = new Set<(update: Uint8Array) => void>()
      const deliver = (event: EditorRemoteUpdate) => {
        if (session === null || listeners.size === 0) {
          waiting.push(event)
          return
        }
        if (event.session !== session) return
        const update = fromBase64(event.update)
        for (const listener of listeners) listener(update)
      }
      const off = core.on('editor-remote-update', deliver)
      const drain = () => {
        const queued = waiting
        waiting = []
        for (const event of queued) deliver(event)
      }

      let opened
      try {
        await off.ready
        opened = await core.editorOpen(noteId)
      } catch (error) {
        off()
        throw error
      }
      session = opened.session
      // Other sessions' updates can be dropped now; ours wait for a listener.
      waiting = waiting.filter((event) => event.session === session)
      let open = true

      return {
        initialState: fromBase64(opened.state),
        async push(update) {
          if (!open) throw new NoteClosedError('editor detached')
          try {
            await core.editorPush(opened.session, toBase64(update))
          } catch (error) {
            if (FINAL.has(errorCode(error))) {
              throw new NoteClosedError('the core refused the edit', error)
            }
            throw error
          }
        },
        onRemote(cb) {
          listeners.add(cb)
          drain()
          return () => listeners.delete(cb)
        },
        async close() {
          if (!open) return
          open = false
          off()
          listeners.clear()
          waiting = []
          try {
            await core.editorClose(opened.session)
          } catch (error) {
            // Locked or gone: the host already dropped the session.
            if (!FINAL.has(errorCode(error))) throw error
          }
        },
      }
    },
  }
}
