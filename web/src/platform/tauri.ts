import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { CoreApi } from './core'
import type { CoreError } from './types'
import { isCoreError } from './types'

/**
 * Calls a host command. Tauri expects camelCase argument keys; the host
 * rejects with `{code, message}`, and anything else (a missing command, a
 * bad argument) becomes an `internal` error.
 */
async function call<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(command, args)
  } catch (error) {
    if (isCoreError(error)) throw error
    throw { code: 'internal', message: String(error) } satisfies CoreError
  }
}

/** The {@link CoreApi} of the Tauri shell, over `invoke` and `listen`. */
export function tauriCore(): CoreApi {
  return {
    appState: () => call('app_state'),
    coreVersion: () => call('core_version'),
    createNotebook: () => call('create_notebook'),
    pendingRecoveryKey: () => call('pending_recovery_key'),
    acknowledgeRecoveryKey: () => call('acknowledge_recovery_key'),
    openWithRecoveryKey: (recoveryKey) =>
      call('open_with_recovery_key', { recoveryKey }),
    upgradeWithPassphrase: (passphrase) =>
      call('upgrade_with_passphrase', { passphrase }),
    startOver: () => call('start_over'),

    tagTree: () => call('tag_tree'),
    listNotes: (selection, query) => call('list_notes', { selection, query }),
    createNote: (selection) => call('create_note', { selection }),
    setPinned: (id, pinned) => call('set_pinned', { id, pinned }),
    setArchived: (id, archived) => call('set_archived', { id, archived }),
    setTrashed: (id, trashed) => call('set_trashed', { id, trashed }),
    deletePermanently: (id) => call('delete_permanently', { id }),
    renameTag: (old, renamed) => call('rename_tag', { old, new: renamed }),
    exportNotes: (args) => call('export_notes', args),

    editorOpen: (note) => call('editor_open', { note }),
    editorPush: (session, update) => call('editor_push', { session, update }),
    editorClose: (session) => call('editor_close', { session }),

    getSettings: () => call('get_settings'),
    setVimMode: (on) => call('set_vim_mode', { on }),
    setPreference: (key, value) => call('set_preference', { key, value }),
    setTagExpanded: (path, expanded) =>
      call('set_tag_expanded', { path, expanded }),

    on(event, handler) {
      // `listen` resolves asynchronously; an unsubscribe that comes first
      // still detaches once it does.
      let unlisten: (() => void) | null = null
      let cancelled = false
      const ready = listen<Parameters<typeof handler>[0]>(event, (e) =>
        handler(e.payload),
      ).then((fn) => {
        if (cancelled) fn()
        else unlisten = fn
      })
      // Callers that don't wait for `ready` shouldn't see a stray rejection.
      ready.catch(() => {})
      const unsubscribe = () => {
        cancelled = true
        unlisten?.()
      }
      return Object.assign(unsubscribe, { ready })
    },
  }
}
