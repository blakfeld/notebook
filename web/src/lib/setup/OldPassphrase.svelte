<script lang="ts">
  import type { CoreApi, Phase } from '../../platform'
  import { errorMessage } from '../errors'
  import StartOver from './StartOver.svelte'

  interface Props {
    core: CoreApi
    onPhase: (phase: Phase) => void
  }

  let { core, onPhase }: Props = $props()

  let passphrase = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy) return
    busy = true
    error = ''
    try {
      const next = await core.upgradeWithPassphrase(passphrase)
      passphrase = ''
      onPhase(next)
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

<div class="setup-form">
  <form class="setup-form" onsubmit={submit} aria-labelledby="upgrade-title">
    <h1 id="upgrade-title">Notebook no longer uses a server</h1>
    <p class="hint">
      Your notes now stay on this computer. To keep the ones already here, enter
      the passphrase you used to sign in. Notebook asks for it this once.
    </p>
    <label class="field">
      Passphrase
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="password"
        autocomplete="current-password"
        required
        autofocus
        aria-invalid={error ? 'true' : undefined}
        bind:value={passphrase}
      />
    </label>
    {#if error}
      <p class="form-error" role="alert">{error}</p>
    {/if}
    <button type="submit" class="primary" disabled={busy}>
      {busy ? 'Checking…' : 'Continue'}
    </button>
  </form>
  <StartOver {core} {onPhase} label="I can’t remember it" />
</div>
