<script lang="ts">
  import type { CoreApi, Phase } from '../../platform'
  import { errorMessage } from '../errors'
  import StartOver from './StartOver.svelte'

  interface Props {
    core: CoreApi
    phase: Phase
    onPhase: (phase: Phase) => void
  }

  let { core, phase, onPhase }: Props = $props()

  let recoveryKey = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy) return
    busy = true
    error = ''
    try {
      // The core ignores case, spaces, and hyphens, and reads I, L, and O
      // as digits, so the key can be typed however it was written down.
      const next = await core.openWithRecoveryKey(recoveryKey.trim())
      recoveryKey = ''
      onPhase(next)
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

<div class="setup-form">
  <form class="setup-form" onsubmit={submit} aria-labelledby="prompt-title">
    <h1 id="prompt-title">Open your notebook</h1>
    {#if phase.secure_store}
      <p class="hint">
        The key that opens this notebook isn’t in this computer’s secure
        credential store. Enter your recovery key to open it.
      </p>
    {:else}
      <p class="hint" data-testid="no-secure-store">
        Notebook couldn’t use this computer’s secure credential store: there may
        be none, or access was denied. If you denied a prompt, quit and reopen
        Notebook to try again. Otherwise, enter your recovery key to open your
        notebook. Without a secure store, you’ll enter it each time.
      </p>
    {/if}
    <label class="field">
      Recovery key
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="text"
        autocomplete="off"
        autocapitalize="characters"
        spellcheck="false"
        placeholder="XXXX-XXXX-XXXX-…"
        required
        autofocus
        aria-invalid={error ? 'true' : undefined}
        bind:value={recoveryKey}
      />
    </label>
    {#if error}
      <p class="form-error" role="alert">{error}</p>
    {/if}
    <button type="submit" class="primary" disabled={busy}>
      {busy ? 'Opening…' : 'Open notebook'}
    </button>
  </form>
  <StartOver
    {core}
    {onPhase}
    label="Lost the key, or the notebook won’t open?"
  />
</div>
