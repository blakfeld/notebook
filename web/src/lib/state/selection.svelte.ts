import type { Selection } from '../../platform'

/** The sidebar selection and the note open in the editor. */
export class SelectionState {
  /** Notes when the notebook opens. */
  current = $state<Selection>({ view: 'notes' })
  /** The open note, or `null` for the empty state. */
  noteId = $state<string | null>(null)
  // Callbacks, not state: nothing renders from this.
  // eslint-disable-next-line svelte/prefer-svelte-reactivity
  readonly #listeners = new Set<() => void>()

  /** Selects a view or tag and closes the open note. */
  select(selection: Selection): void {
    if (sameSelection(selection, this.current)) return
    this.current = selection
    this.noteId = null
    for (const listener of this.#listeners) listener()
  }

  openNote(id: string | null): void {
    this.noteId = id
  }

  /** Calls `cb` after each change of selection. Returns an unsubscribe. */
  onChange(cb: () => void): () => void {
    this.#listeners.add(cb)
    return () => this.#listeners.delete(cb)
  }
}

export function sameSelection(a: Selection, b: Selection): boolean {
  if ('view' in a) return 'view' in b && a.view === b.view
  return 'tag' in b && a.tag.toLowerCase() === b.tag.toLowerCase()
}

/** The tag path of a tag selection, or `null` for a view. */
export function selectedTag(selection: Selection): string | null {
  return 'tag' in selection ? selection.tag : null
}
