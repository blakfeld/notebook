<script lang="ts">
  import type { Account, CoreApi, Phase } from '../../platform'
  import { errorMessage } from '../errors'

  interface Props {
    core: CoreApi
    account: Account | null
    onPhase: (phase: Phase) => void
  }

  let { core, account, onPhase }: Props = $props()

  let passphrase = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy) return
    busy = true
    error = ''
    try {
      const phase = await core.unlock(passphrase)
      passphrase = ''
      onPhase(phase)
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

<div class="account-page">
  <form class="account-form" onsubmit={submit} aria-labelledby="unlock-title">
    <h1 id="unlock-title">Notebook is locked</h1>
    {#if account}
      <p class="hint">{account.username} on {account.server}</p>
    {/if}
    <label class="field">
      Passphrase
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="password"
        autocomplete="current-password"
        required
        autofocus
        bind:value={passphrase}
      />
    </label>
    {#if error}
      <p class="form-error" role="alert">{error}</p>
    {/if}
    <button type="submit" class="primary" disabled={busy}>
      {busy ? 'Unlocking…' : 'Unlock'}
    </button>
  </form>
</div>
