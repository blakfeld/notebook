<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { coreBridge, type CoreApi } from '../../platform'
  import { errorMessage } from '../errors'
  import type { PreferencesStore } from '../preferences'
  import { displayTitle, shortcutOf } from '../format'
  import {
    createVault,
    setVault,
    startVault,
    stopVault,
    type PhaseState,
  } from '../state'
  import { NoteActions } from './actions.svelte'
  import ConfirmDialog from './ConfirmDialog.svelte'
  import EditorPane from './EditorPane.svelte'
  import NoteList from './NoteList.svelte'
  import Settings, { type SettingsSection } from './Settings.svelte'
  import Sidebar from './Sidebar.svelte'

  interface Props {
    core: CoreApi
    phase: PhaseState
    preferences: PreferencesStore
  }

  let { core, phase, preferences }: Props = $props()

  // One vault while the notebook is open. The core, phase, and preferences
  // never change while the shell is mounted.
  const vault = untrack(() => createVault(core, phase, preferences))
  setVault(vault)
  const actions = new NoteActions(vault)
  const bridge = untrack(() => coreBridge(core))

  /** The section Settings opened at, while it is open. */
  let settingsSection = $state<SettingsSection | null>(null)
  let settingsOpen = $derived(settingsSection !== null)
  let noteList = $state<ReturnType<typeof NoteList>>()

  onMount(() => {
    void startVault(vault)
    return () => stopVault(vault)
  })

  function onkeydown(event: KeyboardEvent) {
    // Dialogs keep the keyboard to themselves.
    if (settingsOpen || actions.confirming) return
    const shortcut = shortcutOf(event)
    if (!shortcut) return
    event.preventDefault()
    if (shortcut === 'new-note') void actions.newNote()
    else noteList?.focusSearch()
  }

  const preferenceError = untrack(() => preferences.saveError)

  let message = $derived(
    actions.error ||
      ($preferenceError
        ? `Couldn’t save that setting: ${errorMessage($preferenceError)}`
        : ''),
  )
</script>

<svelte:window {onkeydown} />

<div class="shell">
  {#if message}
    <div class="message" role="alert">
      <span>{message}</span>
      <button
        type="button"
        class="plain"
        aria-label="Dismiss"
        onclick={() => {
          actions.error = ''
          preferences.clearSaveError()
        }}
      >
        <span aria-hidden="true">✕</span>
      </button>
    </div>
  {/if}
  <div class="panes">
    <Sidebar
      onOpenSettings={(section = 'editor') => (settingsSection = section)}
    />
    <NoteList {actions} bind:this={noteList} />
    <EditorPane {bridge} {actions} />
  </div>
</div>

{#if settingsSection}
  <Settings
    initialSection={settingsSection}
    onClose={() => (settingsSection = null)}
  />
{/if}

{#if actions.confirming}
  <ConfirmDialog
    title="Delete permanently?"
    message="“{displayTitle(
      actions.confirming.title,
    )}” will be deleted from this device. This can’t be undone."
    confirmLabel="Delete permanently"
    onConfirm={() => actions.confirmDelete()}
    onCancel={() => actions.cancelDelete()}
  />
{/if}

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  .panes {
    display: grid;
    flex: 1;
    grid-template-columns: minmax(10rem, 14rem) minmax(14rem, 20rem) 1fr;
    min-height: 0;
  }

  .message {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    justify-content: space-between;
    padding: 0.4rem 1rem;
    border-bottom: 1px solid var(--border);
    color: var(--danger);
  }
</style>
