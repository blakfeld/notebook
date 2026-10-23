<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { coreBridge, type CoreApi } from '../../platform'
  import ReauthPrompt from '../account/ReauthPrompt.svelte'
  import SecureStoreNotice from '../account/SecureStoreNotice.svelte'
  import { errorMessage } from '../errors'
  import type { PreferencesStore } from '../preferences'
  import { displayTitle, shortcutOf } from '../format'
  import {
    createVault,
    lockVault,
    setVault,
    startVault,
    stopVault,
    type PhaseState,
  } from '../state'
  import { NoteActions } from './actions.svelte'
  import ConfirmDialog from './ConfirmDialog.svelte'
  import EditorPane from './EditorPane.svelte'
  import NoteList from './NoteList.svelte'
  import Settings from './Settings.svelte'
  import Sidebar from './Sidebar.svelte'

  interface Props {
    core: CoreApi
    phase: PhaseState
    preferences: PreferencesStore
  }

  let { core, phase, preferences }: Props = $props()

  // One vault per unlock: the shell is unmounted on lock, which drops it.
  // The core, phase, and preferences never change while it's mounted.
  const vault = untrack(() => createVault(core, phase, preferences))
  setVault(vault)
  const actions = new NoteActions(vault)
  const bridge = untrack(() => coreBridge(core))

  let settingsOpen = $state(false)
  let noteList = $state<ReturnType<typeof NoteList>>()
  let shellError = $state('')

  onMount(() => {
    void startVault(vault)
    return () => stopVault(vault)
  })

  async function lock() {
    try {
      await lockVault(vault)
    } catch (error) {
      shellError = errorMessage(error)
    }
  }

  function onkeydown(event: KeyboardEvent) {
    // Dialogs keep the keyboard to themselves.
    if (settingsOpen || actions.confirming) return
    const shortcut = shortcutOf(event)
    if (!shortcut) return
    event.preventDefault()
    if (shortcut === 'new-note') void actions.newNote()
    else if (shortcut === 'search') noteList?.focusSearch()
    else void lock()
  }

  const preferenceError = untrack(() => preferences.saveError)

  let message = $derived(
    actions.error ||
      shellError ||
      ($preferenceError
        ? `Couldn’t save that setting: ${errorMessage($preferenceError)}`
        : ''),
  )
</script>

<svelte:window {onkeydown} />

<div class="shell">
  <SecureStoreNotice
    remembered={phase.current?.credentials_remembered ?? true}
  />
  <ReauthPrompt {core} sync={vault.sync} />
  {#if message}
    <div class="message" role="alert">
      <span>{message}</span>
      <button
        type="button"
        class="plain"
        aria-label="Dismiss"
        onclick={() => {
          actions.error = ''
          shellError = ''
          preferences.clearSaveError()
        }}
      >
        <span aria-hidden="true">✕</span>
      </button>
    </div>
  {/if}
  <div class="panes">
    <Sidebar onOpenSettings={() => (settingsOpen = true)} />
    <NoteList {actions} bind:this={noteList} />
    <EditorPane {bridge} {actions} />
  </div>
</div>

{#if settingsOpen}
  <Settings onClose={() => (settingsOpen = false)} />
{/if}

{#if actions.confirming}
  <ConfirmDialog
    title="Delete permanently?"
    message="“{displayTitle(
      actions.confirming.title,
    )}” will be deleted from all your devices. This can’t be undone."
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
