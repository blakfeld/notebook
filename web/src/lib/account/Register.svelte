<script lang="ts">
  import { onMount } from 'svelte'
  import { errorCode, type CoreApi, type Phase } from '../../platform'
  import { checkNewPassphrase, errorMessage } from '../errors'

  interface Props {
    core: CoreApi
    server: string
    onPhase: (phase: Phase) => void
    onSignIn: () => void
  }

  let { core, server = $bindable(), onPhase, onSignIn }: Props = $props()

  let username = $state('')
  let passphrase = $state('')
  let confirm = $state('')
  let inviteCode = $state('')
  /**
   * Generated on the device before the account exists; the core keeps it
   * for `register`. Shown only on this screen and dropped once registered.
   */
  let recoveryKey = $state('')
  let keyError = $state('')
  let saved = $state(false)
  let error = $state('')
  let usernameError = $state('')
  let busy = $state(false)
  let usernameInput: HTMLInputElement

  async function generate() {
    keyError = ''
    try {
      recoveryKey = await core.generateRecoveryKey()
    } catch (err) {
      keyError = errorMessage(err)
    }
  }

  onMount(() => {
    void generate()
  })

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy || !saved || !recoveryKey) return
    error = ''
    usernameError = ''
    const problem = checkNewPassphrase(passphrase, confirm)
    if (problem) {
      error = problem
      return
    }
    busy = true
    try {
      const phase = await core.register({
        server: server.trim(),
        username: username.trim(),
        passphrase,
        confirm,
        inviteCode: inviteCode.trim(),
      })
      recoveryKey = ''
      passphrase = ''
      confirm = ''
      onPhase(phase)
    } catch (err) {
      const code = errorCode(err)
      if (code === 'username_taken' || code === 'invalid_username') {
        usernameError = errorMessage(err)
        usernameInput.focus()
      } else {
        error = errorMessage(err)
      }
    } finally {
      busy = false
    }
  }
</script>

<form class="account-form" onsubmit={submit} aria-labelledby="register-title">
  <h1 id="register-title">Create an account</h1>
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
      aria-invalid={usernameError ? 'true' : undefined}
      aria-describedby={usernameError ? 'register-username-error' : undefined}
      bind:this={usernameInput}
      bind:value={username}
    />
    {#if usernameError}
      <span class="field-error" id="register-username-error" role="alert">
        {usernameError}
      </span>
    {/if}
  </label>
  <label class="field">
    Passphrase
    <input
      type="password"
      autocomplete="new-password"
      required
      aria-describedby="register-passphrase-hint"
      bind:value={passphrase}
    />
    <span class="hint" id="register-passphrase-hint">
      At least 12 characters. It can’t be reset without your recovery key.
    </span>
  </label>
  <label class="field">
    Passphrase again
    <input
      type="password"
      autocomplete="new-password"
      required
      bind:value={confirm}
    />
  </label>
  <label class="field">
    Invite code
    <input
      type="text"
      autocapitalize="off"
      spellcheck="false"
      required
      bind:value={inviteCode}
    />
  </label>

  <section class="recovery" aria-labelledby="recovery-title">
    <h2 id="recovery-title">Your recovery key</h2>
    <p class="hint">
      If you forget your passphrase, this key is the only way back into your
      notes. Write it down or store it in a password manager. It won’t be shown
      again.
    </p>
    {#if recoveryKey}
      <code class="recovery-key" data-testid="recovery-key">{recoveryKey}</code>
    {:else if keyError}
      <p class="form-error" role="alert">{keyError}</p>
      <button type="button" onclick={generate}>Try again</button>
    {:else}
      <p class="hint">Generating…</p>
    {/if}
    <label class="ack">
      <input type="checkbox" bind:checked={saved} disabled={!recoveryKey} />
      I’ve saved my recovery key somewhere safe
    </label>
  </section>

  {#if error}
    <p class="form-error" role="alert">{error}</p>
  {/if}
  <button
    type="submit"
    class="primary"
    disabled={busy || !saved || !recoveryKey}
  >
    {busy ? 'Creating account…' : 'Create account'}
  </button>
  <div class="account-links">
    <button type="button" class="link" onclick={onSignIn}>
      I already have an account
    </button>
  </div>
</form>

<style>
  .recovery {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.75rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
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
