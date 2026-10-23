import type { CoreApi, ListEntry, TagNode } from '../../platform'
import type { SelectionState } from './selection.svelte'

/** How long typing in the search field waits before querying. */
export const SEARCH_DEBOUNCE_MS = 150

/**
 * The note list and tag tree for the current selection and search. Both are
 * re-queried whenever the core reports a vault change, local or remote, so
 * there is one path for live updates.
 */
export class NotesState {
  entries = $state<ListEntry[]>([])
  tree = $state<TagNode[]>([])
  /** The search field's text. Queries follow it after a short pause. */
  query = $state('')
  /** The last failed query, cleared by the next success. */
  error = $state<unknown>(null)
  /** The open note's list entry, or its last one if a search hides it. */
  readonly open = $derived.by(() => {
    const id = this.#selection.noteId
    if (id === null) return null
    return this.entries.find((e) => e.id === id) ?? this.#lastOpen(id)
  })

  readonly #core: CoreApi
  readonly #selection: SelectionState
  #applied = ''
  #timer: ReturnType<typeof setTimeout> | undefined
  #listGeneration = 0
  #treeGeneration = 0
  #seen = $state<ListEntry | null>(null)
  #offs: (() => void)[] = []

  constructor(core: CoreApi, selection: SelectionState) {
    this.#core = core
    this.#selection = selection
  }

  /** Subscribes to changes and loads the list and tree. */
  start(): Promise<void> {
    if (this.#offs.length === 0) {
      this.#offs.push(
        this.#core.on('vault-changed', ({ changed }) => {
          void this.#onChanged(changed)
        }),
        this.#selection.onChange(() => void this.refreshList()),
      )
    }
    return this.refresh()
  }

  stop(): void {
    for (const off of this.#offs) off()
    this.#offs = []
    clearTimeout(this.#timer)
  }

  /**
   * Re-queries after a vault change. When the open note changed and has left
   * the list, it may have been deleted permanently (here or on another
   * device): if no view holds it any more, the editor closes.
   */
  async #onChanged(changed: string[]): Promise<void> {
    await this.refresh()
    const id = this.#selection.noteId
    if (id === null || !changed.includes(id)) return
    if (this.entries.some((e) => e.id === id)) return
    const found = await this.#find(id)
    // Unless the user has moved on meanwhile.
    if (this.#selection.noteId !== id || found === undefined) return
    if (found === null) {
      this.#selection.openNote(null)
    } else {
      // Moved out of the list (e.g. trashed elsewhere): keep its flags
      // current, so the editor turns read-only and offers the right actions.
      this.#seen = found
    }
  }

  /**
   * The note's entry from whichever view holds it (together they cover
   * every note), `null` if none does, or `undefined` if the core couldn't
   * say (e.g. the call failed).
   */
  async #find(id: string): Promise<ListEntry | null | undefined> {
    try {
      const views = await Promise.all(
        (['notes', 'archive', 'trash'] as const).map((view) =>
          this.#core.listNotes({ view }, ''),
        ),
      )
      return views.flat().find((e) => e.id === id) ?? null
    } catch {
      return undefined
    }
  }

  /** Updates the search text; the list follows after a pause. */
  setQuery(query: string): void {
    this.query = query
    clearTimeout(this.#timer)
    this.#timer = setTimeout(() => {
      this.#applied = query
      void this.refreshList()
    }, SEARCH_DEBOUNCE_MS)
  }

  refresh(): Promise<void> {
    return Promise.all([this.refreshList(), this.refreshTree()]).then(() => {})
  }

  async refreshList(): Promise<void> {
    const generation = ++this.#listGeneration
    try {
      const entries = await this.#core.listNotes(
        this.#selection.current,
        this.#applied,
      )
      // A newer query may have finished first.
      if (generation !== this.#listGeneration) return
      this.entries = entries
      this.error = null
      const open = entries.find((e) => e.id === this.#selection.noteId)
      if (open) this.#seen = open
    } catch (error) {
      if (generation === this.#listGeneration) this.error = error
    }
  }

  async refreshTree(): Promise<void> {
    const generation = ++this.#treeGeneration
    try {
      const tree = await this.#core.tagTree()
      if (generation === this.#treeGeneration) this.tree = tree
    } catch (error) {
      if (generation === this.#treeGeneration) this.error = error
    }
  }

  /** Opens a note from the list. */
  openNote(id: string | null): void {
    const entry = this.entries.find((e) => e.id === id)
    if (entry) this.#seen = entry
    this.#selection.openNote(id)
  }

  /** Records the flags of the open note after an action changed them. */
  noteChanged(entry: ListEntry): void {
    if (entry.id === this.#selection.noteId) this.#seen = entry
  }

  #lastOpen(id: string): ListEntry | null {
    return this.#seen?.id === id ? this.#seen : null
  }
}
