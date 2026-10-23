import type { ListEntry } from '../../platform'
import { errorMessage } from '../errors'
import { flushEditor, type Vault } from '../state'

export type NoteAction =
  'pin' | 'unpin' | 'archive' | 'unarchive' | 'trash' | 'restore' | 'delete'

export const ACTION_LABELS: Record<NoteAction, string> = {
  pin: 'Pin',
  unpin: 'Unpin',
  archive: 'Archive',
  unarchive: 'Unarchive',
  trash: 'Move to Trash',
  restore: 'Restore',
  delete: 'Delete permanently…',
}

/**
 * The actions offered for a note. A trashed note can only be restored or,
 * from Trash, deleted permanently.
 */
export function actionsFor(entry: ListEntry, inTrash: boolean): NoteAction[] {
  if (entry.trashed) return inTrash ? ['restore', 'delete'] : ['restore']
  return [
    entry.pinned ? 'unpin' : 'pin',
    entry.archived ? 'unarchive' : 'archive',
    'trash',
  ]
}

/**
 * Note actions shared by the list's context menu, the editor toolbar, and
 * the shortcuts. Failures land in `error`; permanent deletion waits in
 * `confirming` until the user confirms or cancels.
 */
export class NoteActions {
  error = $state('')
  /** The note awaiting delete confirmation. */
  confirming = $state<ListEntry | null>(null)
  /** Bumped when the editor should take focus (after creating a note). */
  focusRequest = $state(0)
  readonly #vault: Vault

  constructor(vault: Vault) {
    this.#vault = vault
  }

  get inTrash(): boolean {
    const current = this.#vault.selection.current
    return 'view' in current && current.view === 'trash'
  }

  /** Creates a note for the selection, opens it, and focuses the editor. */
  async newNote(): Promise<void> {
    const { core, selection, notes } = this.#vault
    const current = selection.current
    // A new note is neither pinned, archived, nor trashed, so show it where
    // it lives.
    if (
      'view' in current &&
      (current.view === 'pinned' ||
        current.view === 'archive' ||
        current.view === 'trash')
    ) {
      selection.select({ view: 'notes' })
    }
    this.error = ''
    try {
      const id = await core.createNote(selection.current)
      await notes.refreshList()
      notes.openNote(id)
      this.focusRequest++
    } catch (error) {
      this.#fail(error)
    }
  }

  async run(action: NoteAction, entry: ListEntry): Promise<void> {
    if (action === 'delete') {
      if (this.inTrash && entry.trashed) this.confirming = entry
      return
    }
    const { core, notes } = this.#vault
    this.error = ''
    try {
      switch (action) {
        case 'pin':
        case 'unpin':
          await core.setPinned(entry.id, action === 'pin')
          notes.noteChanged({ ...entry, pinned: action === 'pin' })
          break
        case 'archive':
        case 'unarchive':
          await core.setArchived(entry.id, action === 'archive')
          notes.noteChanged({ ...entry, archived: action === 'archive' })
          break
        case 'trash':
        case 'restore':
          // A trashed note refuses edits, so deliver the open editor's first.
          if (action === 'trash' && entry.id === this.#vault.selection.noteId) {
            await flushEditor(this.#vault)
          }
          await core.setTrashed(entry.id, action === 'trash')
          notes.noteChanged({ ...entry, trashed: action === 'trash' })
          break
      }
      await notes.refreshList()
    } catch (error) {
      this.#fail(error)
    }
  }

  async confirmDelete(): Promise<void> {
    const entry = this.confirming
    this.confirming = null
    if (!entry) return
    const { core, notes, selection } = this.#vault
    this.error = ''
    try {
      await core.deletePermanently(entry.id)
      if (selection.noteId === entry.id) selection.openNote(null)
      await notes.refreshList()
    } catch (error) {
      this.#fail(error)
    }
  }

  cancelDelete(): void {
    this.confirming = null
  }

  #fail(error: unknown): void {
    this.error = errorMessage(error)
  }
}
