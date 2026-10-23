<script lang="ts">
  import type { CoreApi, Phase, PickedBackup } from '../../platform'
  import { errorMessage, restoreErrorMessage } from '../errors'

  interface Props {
    core: CoreApi
    onPhase: (phase: Phase) => void
  }

  let { core, onPhase }: Props = $props()

  /** The backup picked, while its recovery key is asked for. */
  let picked = $state<PickedBackup | null>(null)
  let recoveryKey = $state('')
  let error = $state('')
  let busy = $state(false)
  let restoring = $state(false)

  async function pick() {
    if (busy) return
    busy = true
    error = ''
    let backup: PickedBackup | null = null
    try {
      backup = await core.pickBackup()
    } catch (err) {
      error = errorMessage(err)
    } finally {
      busy = false
    }
    // `null`: the picker was dismissed, and nothing changes. A backup of
    // the open notebook needs no key (there's none open here, normally).
    if (backup?.same_vault) await restore(null)
    else picked = backup
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault()
    // The core ignores case, spaces, and hyphens, as when opening.
    await restore(recoveryKey.trim())
  }

  async function restore(key: string | null) {
    if (busy) return
    busy = restoring = true
    error = ''
    try {
      const next = await core.restoreBackup({
        recoveryKey: key,
        skipSafetyBackup: false,
      })
      recoveryKey = ''
      onPhase(next)
    } catch (err) {
      error = restoreErrorMessage(err)
    } finally {
      busy = restoring = false
    }
  }

  function cancel() {
    void core.cancelRestore()
    picked = null
    recoveryKey = ''
    error = ''
  }
</script>

{#if picked}
  <form class="restore" onsubmit={submit} aria-labelledby="restore-title">
    <h2 id="restore-title">Restore from backup</h2>
    <p class="hint">
      Enter the recovery key of the notebook in
      <span class="file">{picked.file_name}</span>.
    </p>
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
    <div class="buttons">
      <button type="button" disabled={busy} onclick={cancel}>Cancel</button>
      <button type="submit" class="primary" disabled={busy}>
        {restoring ? 'Restoring…' : 'Restore'}
      </button>
    </div>
  </form>
{:else}
  {#if error}
    <p class="form-error" role="alert">{error}</p>
  {/if}
  <div class="setup-links">
    <button type="button" class="link" disabled={busy} onclick={pick}>
      {restoring ? 'Restoring…' : 'Restore from backup'}
    </button>
  </div>
{/if}

<style>
  .restore {
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

  .file {
    overflow-wrap: anywhere;
    font-family: ui-monospace, monospace;
  }

  .buttons {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }
</style>
