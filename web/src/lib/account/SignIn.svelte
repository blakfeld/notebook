<script lang="ts">
  import type { CoreApi, Phase } from '../../platform'
  import { errorMessage } from '../errors'

  interface Props {
    core: CoreApi
    /** Shared with the other account screens, so it's typed once. */
    server: string
    onPhase: (phase: Phase) => void
    onRegister: () => void
    onRecover: () => void
  }

  let {
    core,
    server = $bindable(),
    onPhase,
    onRegister,
    onRecover,
  }: Props = $props()

  let username = $state('')
  let passphrase = $state('')
  let error = $state('')
  let busy = $state(false)

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy) return
    busy = true
    error = ''
    try {
      const phase = await core.signIn(
        server.trim(),
        username.trim(),
        passphrase,
      )
      passphrase = ''
      onPhase(phase)
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

<form class="account-form" onsubmit={submit} aria-labelledby="sign-in-title">
  <h1 id="sign-in-title">Sign in</h1>
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
    Passphrase
    <input
      type="password"
      autocomplete="current-password"
      required
      bind:value={passphrase}
    />
  </label>
  {#if error}
    <p class="form-error" role="alert">{error}</p>
  {/if}
  <button type="submit" class="primary" disabled={busy}>
    {busy ? 'Signing in…' : 'Sign in'}
  </button>
  <div class="account-links">
    <button type="button" class="link" onclick={onRegister}>
      Create an account
    </button>
    <button type="button" class="link" onclick={onRecover}>
      Forgot passphrase?
    </button>
  </div>
</form>
