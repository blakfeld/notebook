<script lang="ts">
  import type { Snippet } from 'svelte'
  import type { CoreApi } from '../../platform'
  import { errorMessage } from '../errors'

  interface Props {
    core: CoreApi
    /** Called with the new notebook's recovery key, to show it once. */
    onCreated: (recoveryKey: string) => void
    /**
     * The restore slot: `vault-backup` passes its "Restore from backup"
     * button here, shown under "Create a notebook".
     */
    restore?: Snippet
  }

  let { core, onCreated, restore }: Props = $props()

  let error = $state('')
  let busy = $state(false)

  async function create() {
    if (busy) return
    busy = true
    error = ''
    try {
      onCreated(await core.createNotebook())
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

<section class="setup-form" aria-labelledby="welcome-title">
  <h1 id="welcome-title">Welcome to Notebook</h1>
  <p class="hint">
    Your notes are kept on this computer, encrypted. There’s no account to
    create and nothing to sign in to.
  </p>
  {#if error}
    <p class="form-error" role="alert">{error}</p>
  {/if}
  <button type="button" class="primary" disabled={busy} onclick={create}>
    {busy ? 'Creating…' : 'Create a notebook'}
  </button>
  {@render restore?.()}
</section>
