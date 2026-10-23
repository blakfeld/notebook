<script lang="ts">
  import type { CoreApi, Phase } from '../../platform'
  import { errorMessage } from '../errors'

  interface Props {
    core: CoreApi
    /** The link that offers starting over. */
    label: string
    onPhase: (phase: Phase) => void
  }

  let { core, label, onPhase }: Props = $props()

  let confirming = $state(false)
  let error = $state('')
  let busy = $state(false)

  async function startOver() {
    if (busy) return
    busy = true
    error = ''
    try {
      onPhase(await core.startOver())
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

{#if confirming}
  <div class="confirm" role="group" aria-labelledby="start-over-title">
    <h2 id="start-over-title">Start over?</h2>
    <p>
      This creates a new, empty notebook. The current one is kept in the app’s
      data folder under another name, not deleted, but Notebook won’t open it.
    </p>
    {#if error}
      <p class="form-error" role="alert">{error}</p>
    {/if}
    <div class="buttons">
      <button type="button" onclick={() => (confirming = false)}>Cancel</button>
      <button type="button" class="danger" disabled={busy} onclick={startOver}>
        Start over
      </button>
    </div>
  </div>
{:else}
  <div class="setup-links">
    <button type="button" class="link" onclick={() => (confirming = true)}>
      {label}
    </button>
  </div>
{/if}

<style>
  .confirm {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  p {
    margin: 0;
  }

  .buttons {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }
</style>
