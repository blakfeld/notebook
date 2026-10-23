<script lang="ts">
  import type { CoreApi, SyncStatus } from '../../platform'
  import { errorMessage } from '../errors'
  import type { SyncStatusState } from '../state'

  interface Props {
    core: CoreApi
    sync: SyncStatusState
  }

  let { core, sync }: Props = $props()

  let passphrase = $state('')
  let error = $state('')
  let busy = $state(false)
  /** The status that was current when sign-in succeeded. */
  let answered = $state<SyncStatus | null>(null)

  // Shown while the engine says so, except for the status already answered
  // (the engine reports again once it has retried), or when a command found
  // the session ended.
  let needed = $derived(
    sync.signInRequested ||
      (!!sync.status?.attention.includes('sign_in_required') &&
        sync.status !== answered),
  )

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    if (busy) return
    busy = true
    error = ''
    try {
      await core.reauthenticate(passphrase)
      passphrase = ''
      answered = sync.status
      sync.signInRequested = false
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
  }
</script>

{#if needed}
  <form class="reauth" onsubmit={submit} aria-labelledby="reauth-title">
    <div class="text">
      <strong id="reauth-title">Sign in to keep syncing</strong>
      <span class="hint">
        Your session ended. Edits are still saved on this device and sync once
        you sign in.
      </span>
    </div>
    <label class="visually-hidden" for="reauth-passphrase">Passphrase</label>
    <input
      id="reauth-passphrase"
      type="password"
      autocomplete="current-password"
      placeholder="Passphrase"
      required
      bind:value={passphrase}
    />
    <button type="submit" class="primary" disabled={busy}>Sign in</button>
    {#if error}
      <p class="form-error" role="alert">{error}</p>
    {/if}
  </form>
{/if}

<style>
  .reauth {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 0.75rem;
    align-items: center;
    padding: 0.6rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .text {
    display: flex;
    flex: 1 1 18rem;
    flex-direction: column;
  }

  input {
    flex: 0 1 14rem;
  }

  .form-error {
    flex-basis: 100%;
  }
</style>
