<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import type { CoreApi, Phase } from '../../platform'
  import { errorMessage } from '../errors'
  import StartOver from './StartOver.svelte'

  interface Props {
    core: CoreApi
    phase: Phase
    /**
     * The key `createNotebook` returned, if this screen follows it. Without
     * one (an upgrade generates the key at launch) it is fetched.
     */
    recoveryKey: string | null
    onPhase: (phase: Phase) => void
  }

  let { core, phase, recoveryKey, onPhase }: Props = $props()

  /** Shown only on this screen and dropped once acknowledged. */
  let key = $state(untrack(() => recoveryKey) ?? '')
  let keyError = $state('')
  let saved = $state(false)
  let error = $state('')
  let busy = $state(false)

  async function load() {
    keyError = ''
    try {
      key = await core.pendingRecoveryKey()
    } catch (err) {
      keyError = errorMessage(err)
    }
  }

  onMount(() => {
    if (!key) void load()
  })

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy || !saved || !key) return
    busy = true
    error = ''
    try {
      const next = await core.acknowledgeRecoveryKey()
      key = ''
      onPhase(next)
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

<form class="setup-form" onsubmit={submit} aria-labelledby="recovery-title">
  <h1 id="recovery-title">Save your recovery key</h1>
  {#if phase.upgrade}
    <p>
      Notebook no longer uses a sync server: your notes now stay on this
      computer. This is a new recovery key. The one you saved before no longer
      matters.
    </p>
  {/if}
  <p class="hint">
    You’ll need this key to open a backup on another computer, or if this
    computer’s secure credential store loses the notebook’s key. Write it down
    or store it in a password manager. It won’t be shown again.
  </p>
  {#if !phase.secure_store}
    <p class="hint">
      This computer has no secure credential store, so you’ll also enter this
      key each time you open Notebook.
    </p>
  {/if}

  <section class="recovery" aria-label="Recovery key">
    {#if key}
      <code class="recovery-key" data-testid="recovery-key">{key}</code>
    {:else if keyError}
      <p class="form-error" role="alert">{keyError}</p>
      <button type="button" onclick={load}>Try again</button>
    {:else}
      <p class="hint">Loading…</p>
    {/if}
    <label class="ack">
      <input type="checkbox" bind:checked={saved} disabled={!key} />
      I’ve saved my recovery key somewhere safe
    </label>
  </section>

  {#if error}
    <p class="form-error" role="alert">{error}</p>
  {/if}
  <button type="submit" class="primary" disabled={busy || !saved || !key}>
    {busy ? 'Opening…' : 'Open notebook'}
  </button>
</form>
{#if phase.upgrade && error}
  <!-- An upgrade whose old notebook won't open would otherwise stop here
       at every launch. -->
  <StartOver {core} {onPhase} label="The old notebook won’t open?" />
{/if}

<style>
  p {
    margin: 0;
  }

  .recovery {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
  }

  .recovery-key {
    display: block;
    padding: 0.5rem;
    border-radius: 6px;
    background: var(--bg);
    font-size: 1rem;
    letter-spacing: 0.05em;
    overflow-wrap: anywhere;
    user-select: all;
  }

  .ack {
    display: flex;
    gap: 0.5rem;
    align-items: flex-start;
  }
</style>
