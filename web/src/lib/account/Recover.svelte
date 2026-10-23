<script lang="ts">
  import type { CoreApi, Phase } from '../../platform'
  import { checkNewPassphrase, errorMessage } from '../errors'

  interface Props {
    core: CoreApi
    server: string
    onPhase: (phase: Phase) => void
    onSignIn: () => void
  }

  let { core, server = $bindable(), onPhase, onSignIn }: Props = $props()

  let username = $state('')
  let recoveryKey = $state('')
  let passphrase = $state('')
  let confirm = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy) return
    error = ''
    const problem = checkNewPassphrase(passphrase, confirm)
    if (problem) {
      error = problem
      return
    }
    busy = true
    try {
      const phase = await core.recover({
        server: server.trim(),
        username: username.trim(),
        recoveryKey: recoveryKey.trim(),
        passphrase,
        confirm,
      })
      recoveryKey = ''
      passphrase = ''
      confirm = ''
      onPhase(phase)
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

<form class="account-form" onsubmit={submit} aria-labelledby="recover-title">
  <h1 id="recover-title">Recover your account</h1>
  <p class="hint">
    Enter your recovery key and choose a new passphrase. Your notes stay as they
    are.
  </p>
  <label class="field">
    Server
    <input
      type="text"
      inputmode="url"
      autocapitalize="off"
      spellcheck="false"
      placeholder="https://notes.example.com"
      required
      bind:value={server}
    />
  </label>
  <label class="field">
    Username
    <input
      type="text"
      autocomplete="username"
      autocapitalize="off"
      spellcheck="false"
      required
      bind:value={username}
    />
  </label>
  <label class="field">
    Recovery key
    <input
      type="text"
      autocomplete="off"
      autocapitalize="characters"
      spellcheck="false"
      required
      bind:value={recoveryKey}
    />
  </label>
  <label class="field">
    New passphrase
    <input
      type="password"
      autocomplete="new-password"
      required
      bind:value={passphrase}
    />
    <span class="hint">At least 12 characters.</span>
  </label>
  <label class="field">
    New passphrase again
    <input
      type="password"
      autocomplete="new-password"
      required
      bind:value={confirm}
    />
  </label>
  {#if error}
    <p class="form-error" role="alert">{error}</p>
  {/if}
  <button type="submit" class="primary" disabled={busy}>
    {busy ? 'Recovering…' : 'Recover account'}
  </button>
  <div class="account-links">
    <button type="button" class="link" onclick={onSignIn}>
      Back to sign in
    </button>
  </div>
</form>
