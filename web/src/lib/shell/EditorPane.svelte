<script lang="ts">
  import { untrack } from 'svelte'
  import {
    NoteEditor,
    type NoteDocBridge,
    type NoteEditorHandle,
  } from '../../editor'
  import { errorMessage } from '../errors'
  import { shortcutLabel } from '../format'
  import { useVault } from '../state'
  import { ACTION_LABELS, actionsFor, type NoteActions } from './actions.svelte'

  interface Props {
    bridge: NoteDocBridge
    actions: NoteActions
  }

  let { bridge, actions }: Props = $props()

  const vault = useVault()
  const { selection, notes, settings } = vault

  let handle = $state<NoteEditorHandle | null>(null)
  let editorError = $state('')

  // Lets lock, sign-out, and trash deliver pending edits first.
  $effect(() => {
    const current = handle
    vault.editor = current
    return () => {
      if (vault.editor === current) vault.editor = null
    }
  })

  let open = $derived(notes.open)
  let readOnly = $derived(!!open?.trashed)

  // A new note gets the cursor.
  $effect(() => {
    if (actions.focusRequest === 0 || !handle) return
    untrack(() => handle?.focus())
  })

  $effect(() => {
    void selection.noteId
    editorError = ''
  })
</script>

<section class="editor-pane" aria-label="Editor">
  {#if selection.noteId && open}
    <div class="toolbar" role="toolbar" aria-label="Note actions">
      {#each actionsFor(open, actions.inTrash) as action (action)}
        <button
          type="button"
          class="plain"
          class:danger={action === 'delete'}
          onclick={() => actions.run(action, open)}
        >
          {ACTION_LABELS[action]}
        </button>
      {/each}
    </div>
    {#if readOnly}
      <div class="trash-banner" role="status">
        <span>This note is in Trash and can’t be edited.</span>
        <button type="button" onclick={() => actions.run('restore', open)}>
          Restore
        </button>
      </div>
    {/if}
  {/if}
  {#if editorError}
    <p class="form-error editor-error" role="alert">{editorError}</p>
  {/if}

  <!-- One editor for the pane's lifetime; it switches notes in place. -->
  <div class="editor" hidden={!selection.noteId}>
    <NoteEditor
      {bridge}
      settings={settings.device}
      noteId={selection.noteId}
      vimMode={settings.vimMode}
      {readOnly}
      bind:handle
      onError={(error) => (editorError = errorMessage(error))}
    />
  </div>

  {#if !selection.noteId}
    <div class="empty">
      <p>
        {notes.entries.length === 0 ? 'No notes here yet' : 'No note selected'}
      </p>
      {#if !actions.inTrash}
        <button type="button" onclick={() => actions.newNote()}>
          New note <span class="hint">{shortcutLabel('N')}</span>
        </button>
      {/if}
    </div>
  {/if}
</section>

<style>
  .editor-pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }

  .toolbar {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    justify-content: flex-end;
    padding: 0.4rem 0.75rem;
    border-bottom: 1px solid var(--border);
  }

  .toolbar button {
    padding: 0.25rem 0.6rem;
    font-size: 0.9rem;
  }

  .danger {
    color: var(--danger);
  }

  .trash-banner {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    padding: 0.5rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .editor-error {
    padding: 0.3rem 1rem;
  }

  .editor {
    flex: 1;
    min-height: 0;
  }

  .editor[hidden] {
    display: none;
  }

  .empty {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 0.75rem;
    align-items: center;
    justify-content: center;
    color: var(--muted);
  }

  .empty p {
    margin: 0;
  }
</style>
