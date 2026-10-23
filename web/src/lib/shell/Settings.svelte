<script lang="ts">
  import { errorCode, type ExportResult, type Phase } from '../../platform'
  import { checkNewPassphrase, errorMessage } from '../errors'
  import { shortcutLabel, syncSummary } from '../format'
  import { flushEditor, useVault } from '../state'
  import Modal from './Modal.svelte'

  interface Props {
    onClose: () => void
  }

  let { onClose }: Props = $props()

  const vault = useVault()
  const { core, phase, settings, sync } = vault

  /**
   * The screen's sections, in order. Other capabilities add theirs here
   * (Appearance from `editor-live-preview`, Export from `markdown-export`).
   */
  const SECTIONS = [
    { id: 'account', label: 'Account' },
    { id: 'editor', label: 'Editor' },
    { id: 'export', label: 'Export' },
  ] as const
  type Section = (typeof SECTIONS)[number]['id']

  let section = $state<Section>('account')

  let account = $derived(phase.current?.account ?? null)
  let summary = $derived(syncSummary(sync.status))

  // ---- change passphrase ----
  let changing = $state(false)
  let current = $state('')
  let next = $state('')
  let confirm = $state('')
  let changeError = $state('')
  let changeDone = $state(false)
  let changeBusy = $state(false)

  async function changePassphrase(event: SubmitEvent) {
    event.preventDefault()
    if (changeBusy) return
    changeError = ''
    const problem = checkNewPassphrase(next, confirm)
    if (problem) {
      changeError = problem
      return
    }
    changeBusy = true
    try {
      await core.changePassphrase({ current, new: next, confirm })
      current = next = confirm = ''
      changing = false
      changeDone = true
    } catch (error) {
      const code = errorCode(error)
      sync.checkError(error)
      changeError =
        code === 'offline'
          ? 'Changing the passphrase needs a connection to the server. Nothing was changed.'
          : code === 'sign_in_required'
            ? 'Your session has ended. Sign in again from the prompt above the notes, then try again.'
            : errorMessage(error)
    } finally {
      changeBusy = false
    }
  }

  // ---- lock and sign out ----
  let accountError = $state('')
  /** Unsynced changes the user is being warned about, or `null`. */
  let losing = $state<number | null>(null)
  let busy = $state(false)

  /** Locks or signs out, after delivering the open editor's edits. */
  async function apply(action: () => Promise<Phase>) {
    if (busy) return
    busy = true
    accountError = ''
    try {
      await flushEditor(vault)
      phase.set(await action())
    } catch (error) {
      if (errorCode(error) === 'pending_changes') {
        losing = await core.pendingCount().catch(() => 1)
      } else {
        accountError = errorMessage(error)
      }
    } finally {
      busy = false
    }
  }

  // ---- export ----
  let includeTrashed = $state(false)
  let exporting = $state(false)
  let exported = $state<ExportResult | null>(null)
  let exportError = $state('')
  let unlocked = $derived(phase.current?.phase === 'unlocked')

  async function exportNotes() {
    if (exporting || !unlocked) return
    exporting = true
    exported = null
    exportError = ''
    try {
      // Edits still in the editor are part of the export.
      await flushEditor(vault)
      exported = await core.exportNotes({ includeTrashed })
    } catch (error) {
      exportError = errorMessage(error)
    } finally {
      exporting = false
    }
  }

  async function signOut() {
    if (busy) return
    accountError = ''
    let pending: number
    try {
      // Edits still in the editor count too.
      await flushEditor(vault)
      pending = await core.pendingCount()
    } catch (error) {
      accountError = errorMessage(error)
      return
    }
    if (pending > 0) {
      losing = pending
      return
    }
    await apply(() => core.signOut(false))
  }
</script>

<Modal labelledby="settings-title" {onClose}>
  <div class="settings">
    <header>
      <h2 id="settings-title">Settings</h2>
      <button
        type="button"
        class="plain"
        aria-label="Close settings"
        onclick={onClose}
      >
        <span aria-hidden="true">✕</span>
      </button>
    </header>
    <div class="body">
      <div class="tabs" role="tablist" aria-orientation="vertical">
        {#each SECTIONS as { id, label } (id)}
          <button
            type="button"
            role="tab"
            id="settings-tab-{id}"
            class="plain"
            aria-selected={section === id}
            aria-controls="settings-panel-{id}"
            onclick={() => (section = id)}
          >
            {label}
          </button>
        {/each}
      </div>

      <div
        class="panel"
        role="tabpanel"
        id="settings-panel-{section}"
        aria-labelledby="settings-tab-{section}"
      >
        {#if section === 'account'}
          <dl>
            <dt>Server</dt>
            <dd>{account?.server ?? '—'}</dd>
            <dt>Username</dt>
            <dd>{account?.username ?? '—'}</dd>
            <dt>Status</dt>
            <dd>
              Signed in · {summary.label}{summary.pending
                ? ` · ${summary.pending}`
                : ''}
              {#each summary.alerts as alert (alert)}
                <br /><span class="alert">{alert}</span>
              {/each}
            </dd>
            {#if phase.current && !phase.current.credentials_remembered}
              <dt>This device</dt>
              <dd>Sign-in isn’t remembered (no secure credential store)</dd>
            {/if}
          </dl>

          <div class="group">
            <h3>Passphrase</h3>
            {#if changing}
              <form class="change" onsubmit={changePassphrase}>
                <label class="field">
                  Current passphrase
                  <input
                    type="password"
                    autocomplete="current-password"
                    required
                    bind:value={current}
                  />
                </label>
                <label class="field">
                  New passphrase
                  <input
                    type="password"
                    autocomplete="new-password"
                    required
                    bind:value={next}
                  />
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
                {#if changeError}
                  <p class="form-error" role="alert">{changeError}</p>
                {/if}
                <div class="buttons">
                  <button
                    type="button"
                    onclick={() => {
                      changing = false
                      changeError = ''
                      current = next = confirm = ''
                    }}
                  >
                    Cancel
                  </button>
                  <button type="submit" class="primary" disabled={changeBusy}>
                    Change passphrase
                  </button>
                </div>
              </form>
            {:else}
              {#if changeDone}
                <p class="hint" role="status">Passphrase changed.</p>
              {/if}
              <button
                type="button"
                onclick={() => {
                  changing = true
                  changeDone = false
                }}
              >
                Change passphrase…
              </button>
            {/if}
          </div>

          <div class="group">
            <h3>Session</h3>
            {#if losing !== null}
              <div class="warning" role="alert">
                <p>
                  {losing === 1
                    ? '1 change hasn’t synced and will be lost.'
                    : `${losing} changes haven’t synced and will be lost.`}
                  Sign out anyway?
                </p>
                <div class="buttons">
                  <button type="button" onclick={() => (losing = null)}>
                    Cancel
                  </button>
                  <button
                    type="button"
                    class="danger"
                    disabled={busy}
                    onclick={() => apply(() => core.signOut(true))}
                  >
                    Sign out and lose changes
                  </button>
                </div>
              </div>
            {:else}
              <div class="buttons start">
                <button
                  type="button"
                  disabled={busy}
                  onclick={() => apply(() => core.lock())}
                >
                  Lock <span class="hint">{shortcutLabel('L')}</span>
                </button>
                <button
                  type="button"
                  class="danger"
                  disabled={busy}
                  onclick={signOut}
                >
                  Sign out
                </button>
              </div>
              <p class="hint">
                Signing out removes this device’s copy of your notes.
              </p>
            {/if}
            {#if accountError}
              <p class="form-error" role="alert">{accountError}</p>
            {/if}
          </div>
        {:else if section === 'editor'}
          <div class="group">
            <label class="toggle">
              <input
                type="checkbox"
                aria-describedby="vim-hint"
                checked={settings.vimMode}
                onchange={(event) =>
                  settings.setVimMode(event.currentTarget.checked)}
              />
              Vim mode
            </label>
            <p class="hint" id="vim-hint">
              Vim keys in the editor. Applies to this device only.
            </p>
          </div>
        {:else if section === 'export'}
          <div class="group">
            <h3>Export all notes</h3>
            <p class="hint">
              Saves every note as a markdown file, with its dates and state, in
              a new folder. Archived notes are included.
            </p>
            <p class="warning-text" id="export-warning">
              Exported files are plain, unencrypted text. Anyone who can open
              them can read your notes.
            </p>
            <label class="toggle">
              <input
                type="checkbox"
                bind:checked={includeTrashed}
                disabled={exporting}
              />
              Include notes in Trash
            </label>
            <button
              type="button"
              aria-describedby="export-warning"
              disabled={exporting || !unlocked}
              onclick={exportNotes}
            >
              {exporting ? 'Exporting…' : 'Export all notes…'}
            </button>
            {#if exported}
              <p class="hint" role="status">
                Exported {exported.count === 1
                  ? '1 note'
                  : `${exported.count} notes`} to
                <span class="path">{exported.path}</span>
              </p>
            {/if}
            {#if exportError}
              <p class="form-error" role="alert">{exportError}</p>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
</Modal>

<style>
  .settings {
    width: min(620px, calc(100vw - 4rem));
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 1.1rem;
  }

  .body {
    display: grid;
    grid-template-columns: 9rem 1fr;
    min-height: 20rem;
  }

  .tabs {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    padding: 0.75rem 0.5rem;
    border-right: 1px solid var(--border);
    background: var(--surface);
  }

  .tabs button {
    text-align: left;
  }

  .tabs button[aria-selected='true'] {
    background: var(--selected);
  }

  .panel {
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
    padding: 1rem 1.25rem;
  }

  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.4rem 1rem;
    margin: 0;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .alert {
    color: var(--warning);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    align-items: flex-start;
  }

  h3 {
    margin: 0;
    font-size: 0.95rem;
  }

  .change {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    width: 100%;
    max-width: 22rem;
  }

  .buttons {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
  }

  .buttons.start {
    justify-content: flex-start;
  }

  .warning p {
    margin: 0 0 0.5rem;
  }

  .group p {
    margin: 0;
  }

  .warning-text {
    color: var(--warning);
  }

  .path {
    overflow-wrap: anywhere;
    font-family: ui-monospace, monospace;
  }

  .toggle {
    display: flex;
    gap: 0.6rem;
    align-items: flex-start;
  }

  @media (max-width: 560px) {
    .body {
      grid-template-columns: 1fr;
    }

    .tabs {
      flex-direction: row;
      border-right: none;
      border-bottom: 1px solid var(--border);
    }
  }
</style>
