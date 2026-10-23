<script lang="ts">
  import type { ListEntry, View } from '../../platform'
  import { displayTitle, relativeTime, shortcutLabel } from '../format'
  import { useVault } from '../state'
  import {
    actionsFor,
    type NoteAction,
    type NoteActions,
  } from './actions.svelte'
  import ContextMenu from './ContextMenu.svelte'

  interface Props {
    actions: NoteActions
  }

  let { actions }: Props = $props()

  const { selection, notes } = useVault()

  let search: HTMLInputElement
  let list = $state<HTMLUListElement>()
  let menu = $state<{ entry: ListEntry; x: number; y: number } | null>(null)
  /** Refreshed each minute so relative times stay right. */
  let now = $state(Date.now())

  $effect(() => {
    const timer = setInterval(() => (now = Date.now()), 60_000)
    return () => clearInterval(timer)
  })

  const VIEW_TITLES: Record<View, string> = {
    notes: 'Notes',
    untagged: 'Untagged',
    pinned: 'Pinned',
    archive: 'Archive',
    trash: 'Trash',
  }

  let heading = $derived(
    'view' in selection.current
      ? VIEW_TITLES[selection.current.view]
      : `#${selection.current.tag}`,
  )

  /** Focuses the search field (`Mod+Shift+F`). */
  export function focusSearch() {
    search.focus()
    search.select()
  }

  function open(entry: ListEntry) {
    notes.openNote(entry.id)
  }

  function showMenu(entry: ListEntry, x: number, y: number) {
    notes.openNote(entry.id)
    menu = { entry, x, y }
  }

  function closeMenu() {
    const id = menu?.entry.id
    menu = null
    // Back to the entry the menu was for, if it's still listed.
    list?.querySelector<HTMLElement>(`[data-id="${id}"]`)?.focus()
  }

  function act(action: NoteAction) {
    const entry = menu!.entry
    menu = null
    void actions.run(action, entry)
  }

  function onkeydown(event: KeyboardEvent, index: number) {
    const entry = notes.entries[index]
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      const next = notes.entries[index + (event.key === 'ArrowDown' ? 1 : -1)]
      if (next) {
        open(next)
        list?.querySelector<HTMLElement>(`[data-id="${next.id}"]`)?.focus()
      }
    } else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault()
      open(entry)
    } else if (
      event.key === 'ContextMenu' ||
      (event.shiftKey && event.key === 'F10')
    ) {
      event.preventDefault()
      const rect = (event.currentTarget as HTMLElement).getBoundingClientRect()
      showMenu(entry, rect.left + 16, rect.top + rect.height / 2)
    }
  }

  // The selected entry is the one Tab lands on; the others are reached
  // with the arrow keys.
  function tabIndexOf(entry: ListEntry, index: number): number {
    const current = selection.noteId
    const listed = notes.entries.some((e) => e.id === current)
    return entry.id === current || (!listed && index === 0) ? 0 : -1
  }
</script>

<section class="note-list" aria-labelledby="note-list-heading">
  <header>
    <div class="title-row">
      <h2 id="note-list-heading">{heading}</h2>
      <button
        type="button"
        class="plain new"
        aria-label="New note"
        title="New note ({shortcutLabel('N')})"
        onclick={() => actions.newNote()}
      >
        <span aria-hidden="true">＋</span>
      </button>
    </div>
    <input
      type="search"
      placeholder="Search"
      aria-label="Search notes"
      title="Search ({shortcutLabel('F', true)})"
      value={notes.query}
      oninput={(event) => notes.setQuery(event.currentTarget.value)}
      onkeydown={(event) => {
        if (event.key === 'Escape' && notes.query) {
          event.preventDefault()
          notes.setQuery('')
        }
      }}
      bind:this={search}
    />
  </header>

  {#if notes.entries.length === 0}
    <p class="empty">
      {notes.query.trim() ? 'No matching notes' : 'No notes'}
    </p>
  {:else}
    <ul
      class="entries"
      role="listbox"
      aria-labelledby="note-list-heading"
      bind:this={list}
    >
      {#each notes.entries as entry, index (entry.id)}
        <li
          role="option"
          data-id={entry.id}
          aria-selected={entry.id === selection.noteId}
          tabindex={tabIndexOf(entry, index)}
          onclick={() => open(entry)}
          onkeydown={(event) => onkeydown(event, index)}
          oncontextmenu={(event) => {
            event.preventDefault()
            showMenu(entry, event.clientX, event.clientY)
          }}
        >
          <div class="top">
            <span class="entry-title" class:untitled={!entry.title.trim()}>
              {displayTitle(entry.title)}
            </span>
            {#if entry.pinned && !entry.trashed}
              <span class="pin" title="Pinned">
                <span aria-hidden="true">•</span>
                <span class="visually-hidden">Pinned</span>
              </span>
            {/if}
          </div>
          {#if entry.preview}
            <p class="preview">{entry.preview}</p>
          {/if}
          <time datetime={new Date(entry.updated).toISOString()}>
            {relativeTime(entry.updated, now)}
          </time>
        </li>
      {/each}
    </ul>
  {/if}
</section>

{#if menu}
  <ContextMenu
    actions={actionsFor(menu.entry, actions.inTrash)}
    label="Actions for {displayTitle(menu.entry.title)}"
    x={menu.x}
    y={menu.y}
    onAction={act}
    onClose={closeMenu}
  />
{/if}

<style>
  .note-list {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border-right: 1px solid var(--border);
  }

  header {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem;
    border-bottom: 1px solid var(--border);
  }

  .title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  h2 {
    margin: 0;
    overflow: hidden;
    font-size: 1rem;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .new {
    padding: 0.1rem 0.45rem;
    font-size: 1.1rem;
  }

  .empty {
    margin: 2rem 1rem;
    color: var(--muted);
    text-align: center;
  }

  .entries {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    list-style: none;
  }

  li {
    padding: 0.6rem 0.75rem;
    border-bottom: 1px solid var(--border);
    cursor: default;
  }

  li:hover {
    background: var(--hover);
  }

  li[aria-selected='true'] {
    background: var(--selected);
  }

  li:focus-visible {
    outline-offset: -2px;
  }

  .top {
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    justify-content: space-between;
  }

  .entry-title {
    overflow: hidden;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .untitled {
    color: var(--muted);
    font-weight: 500;
  }

  .pin {
    color: var(--accent);
  }

  .preview {
    display: -webkit-box;
    margin: 0.2rem 0;
    overflow: hidden;
    color: var(--muted);
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  time {
    color: var(--muted);
    font-size: 0.8rem;
  }
</style>
