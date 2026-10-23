<script module lang="ts">
  export type SettingsSection =
    'appearance' | 'editor' | 'backup' | 'export' | 'about'
</script>

<script lang="ts">
  import { onMount, tick, untrack } from 'svelte'
  import {
    errorCode,
    type BackupInterval,
    type BackupOptions,
    type ExportResult,
    type PickedBackup,
  } from '../../platform'
  import { errorMessage, restoreErrorMessage, sentence } from '../errors'
  import { dayTime } from '../format'
  import { flushEditor, useVault } from '../state'
  import { formatVersion } from '../version'
  import Modal from './Modal.svelte'

  interface Props {
    /** The section shown first. */
    initialSection?: SettingsSection
    onClose: () => void
  }

  let { initialSection = 'editor', onClose }: Props = $props()

  const vault = useVault()
  const { core, settings, preferences, backup } = vault

  /**
   * The screen's sections, in order. Other capabilities add theirs here
   * (Export from `markdown-export`, Backup from `vault-backup`).
   */
  const SECTIONS: { id: SettingsSection; label: string }[] = [
    { id: 'appearance', label: 'Appearance' },
    { id: 'editor', label: 'Editor' },
    { id: 'backup', label: 'Backup' },
    { id: 'export', label: 'Export' },
    { id: 'about', label: 'About' },
  ]

  const THEMES = [
    { value: 'system', label: 'System' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
  ] as const

  let section = $state<SettingsSection>(untrack(() => initialSection))

  /** `undefined` while loading, `null` if it couldn't be read. */
  let version = $state<string | null | undefined>(undefined)

  onMount(() => {
    core.coreVersion().then(
      (v) => (version = v),
      () => (version = null),
    )
  })

  // ---- export ----
  let includeTrashed = $state(false)
  let exporting = $state(false)
  let exported = $state<ExportResult | null>(null)
  let exportError = $state('')

  async function exportNotes() {
    if (exporting) return
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

  // ---- backup ----
  const INTERVALS: { value: BackupInterval; label: string }[] = [
    { value: 'hourly', label: 'Hourly' },
    { value: 'daily', label: 'Daily' },
    { value: 'weekly', label: 'Weekly' },
  ]

  let status = $derived(backup.status)
  let backingUp = $state(false)
  let backupError = $state('')

  /** When the next backup happens, in words. */
  let nextBackup = $derived.by(() => {
    if (!status?.folder) return ''
    if (!status.auto) return 'Automatic backups are off'
    if (!status.changed) return 'After your next change'
    if (status.next_due === null || status.next_due <= Date.now()) {
      return 'Soon'
    }
    return dayTime(status.next_due)
  })

  async function chooseFolder() {
    backupError = ''
    try {
      // `null`: the picker was dismissed, and nothing changed.
      const next = await core.chooseBackupFolder()
      if (next) backup.set(next)
    } catch (error) {
      backupError = errorMessage(error)
    }
  }

  async function setOptions(changes: Partial<BackupOptions>) {
    if (!status) return
    const { auto, interval, keep } = status
    backupError = ''
    try {
      backup.set(
        await core.setBackupOptions({ auto, interval, keep, ...changes }),
      )
    } catch (error) {
      backupError = errorMessage(error)
    }
  }

  function setKeep(input: HTMLInputElement) {
    const keep = Number(input.value)
    if (Number.isInteger(keep) && keep >= 1) void setOptions({ keep })
    else if (status) input.value = String(status.keep)
  }

  async function backUpNow() {
    if (backingUp) return
    backingUp = true
    backupError = ''
    try {
      // Edits still in the editor are part of the backup.
      await flushEditor(vault)
      backup.set(await core.backupNow())
    } catch (error) {
      // A failed write shows as the status's last error.
      if (errorCode(error) === 'backup_failed') await backup.refresh()
      else backupError = errorMessage(error)
    } finally {
      backingUp = false
    }
  }

  // ---- restore ----
  /**
   * Where a restore from here is: asking for the backup's recovery key,
   * confirming that the notes will be replaced, or confirming again after
   * the backup of the current notes failed.
   */
  type Restore =
    | { step: 'key'; picked: PickedBackup }
    | { step: 'confirm'; picked: PickedBackup; key: string | null }
    | { step: 'unprotected'; picked: PickedBackup; key: string | null }

  let restore = $state<Restore | null>(null)
  let restoreKey = $state('')
  let restoreError = $state('')
  let restoring = $state(false)

  async function pickBackup() {
    if (restoring) return
    restoring = true
    restore = null
    restoreError = ''
    try {
      // `null`: the picker was dismissed, and nothing changes.
      const picked = await core.pickBackup()
      if (picked?.same_vault) restore = { step: 'confirm', picked, key: null }
      else if (picked) restore = { step: 'key', picked }
    } catch (error) {
      restoreError = errorMessage(error)
    } finally {
      restoring = false
    }
  }

  function enterKey(event: SubmitEvent) {
    event.preventDefault()
    if (restore?.step !== 'key') return
    restoreError = ''
    // The core ignores case, spaces, and hyphens, as when opening.
    restore = {
      step: 'confirm',
      picked: restore.picked,
      key: restoreKey.trim(),
    }
  }

  function cancelRestore() {
    void core.cancelRestore()
    restore = null
    restoreKey = ''
    restoreError = ''
  }

  async function replaceNotes() {
    if (!restore || restore.step === 'key' || restoring) return
    const { picked, key } = restore
    const skipSafetyBackup = restore.step === 'unprotected'
    restoring = true
    restoreError = ''
    try {
      // Edits still in the editor are part of the safety backup.
      await flushEditor(vault)
      vault.phase.set(
        await core.restoreBackup({ recoveryKey: key, skipSafetyBackup }),
      )
      restore = null
      restoreKey = ''
      // The notes are all new: start over on Notes, with nothing open.
      vault.selection.openNote(null)
      vault.selection.select({ view: 'notes' })
      void vault.notes.refresh()
      void backup.refresh()
      onClose()
    } catch (error) {
      restoreError = restoreErrorMessage(error)
      const code = errorCode(error)
      if (code === 'safety_backup_failed') {
        restore = { step: 'unprotected', picked, key }
      } else if (code.startsWith('recovery_key_')) {
        restore = { step: 'key', picked }
      } else {
        void core.cancelRestore()
        restore = null
        // A rolled-back restore reopened the notebook, ending the editor's
        // session: open the note again.
        const open = vault.selection.noteId
        vault.selection.openNote(null)
        await tick()
        vault.selection.openNote(open)
      }
    } finally {
      restoring = false
    }
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
        {#if section === 'appearance'}
          <fieldset class="group" aria-describedby="theme-hint">
            <legend>Theme</legend>
            <div class="choices">
              {#each THEMES as { value, label } (value)}
                <label class="toggle">
                  <input
                    type="radio"
                    name="theme"
                    {value}
                    checked={$preferences.theme === value}
                    onchange={() => preferences.set('theme', value)}
                  />
                  {label}
                </label>
              {/each}
            </div>
            <p class="hint" id="theme-hint">
              System follows your device’s light or dark setting. Applies to
              this device only.
            </p>
          </fieldset>
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
        {:else if section === 'backup'}
          <div class="group">
            <h3>Backup folder</h3>
            {#if status?.folder}
              <p class="path" data-testid="backup-folder">{status.folder}</p>
            {:else}
              <p class="hint">
                Choose a folder for backups of your notes, such as one in
                Dropbox or iCloud Drive.
              </p>
            {/if}
            <button type="button" onclick={chooseFolder}>
              {status?.folder ? 'Change folder…' : 'Choose folder…'}
            </button>
            <p class="hint">
              Backups are encrypted. To open one on another computer, you need
              your recovery key.
            </p>
          </div>

          <div class="group">
            <label class="toggle">
              <input
                type="checkbox"
                checked={status?.auto ?? true}
                disabled={!status?.folder}
                onchange={(event) =>
                  setOptions({ auto: event.currentTarget.checked })}
              />
              Back up automatically
            </label>
            <div class="options">
              <label>
                Every
                <select
                  value={status?.interval ?? 'daily'}
                  disabled={!status?.folder || !status.auto}
                  onchange={(event) =>
                    setOptions({
                      interval: event.currentTarget.value as BackupInterval,
                    })}
                >
                  {#each INTERVALS as { value, label } (value)}
                    <option {value}>{label}</option>
                  {/each}
                </select>
              </label>
              <label>
                Backups to keep
                <input
                  type="number"
                  min="1"
                  step="1"
                  value={status?.keep ?? 14}
                  disabled={!status?.folder}
                  onchange={(event) => setKeep(event.currentTarget)}
                />
              </label>
            </div>
          </div>

          {#if status?.folder}
            <dl data-testid="backup-status">
              <dt>Last backup</dt>
              <dd>{status.last === null ? 'Never' : dayTime(status.last)}</dd>
              <dt>Next backup</dt>
              <dd>{nextBackup}</dd>
            </dl>
            {#if status.last_error}
              <p class="warning-text" data-testid="backup-error">
                The last backup failed. {sentence(status.last_error)}
              </p>
            {/if}
          {/if}
          <div class="group">
            <button
              type="button"
              disabled={!status?.folder || backingUp || status.running}
              onclick={backUpNow}
            >
              {backingUp || status?.running ? 'Backing up…' : 'Back up now'}
            </button>
            {#if backupError}
              <p class="form-error" role="alert">{backupError}</p>
            {/if}
          </div>

          <div class="group">
            <h3>Restore</h3>
            <p class="hint">
              Replaces the notes on this computer with the notes in a backup.
            </p>
            {#if restore?.step === 'key'}
              <form class="restore" onsubmit={enterKey}>
                <p>
                  Enter the recovery key of the notebook in
                  <span class="path">{restore.picked.file_name}</span>.
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
                    aria-invalid={restoreError ? 'true' : undefined}
                    bind:value={restoreKey}
                  />
                </label>
                {#if restoreError}
                  <p class="form-error" role="alert">{restoreError}</p>
                {/if}
                <div class="buttons">
                  <button type="button" onclick={cancelRestore}>Cancel</button>
                  <button type="submit" class="primary">Continue</button>
                </div>
              </form>
            {:else if restore}
              <div
                class="restore"
                role="group"
                aria-labelledby="restore-confirm-title"
              >
                <h4 id="restore-confirm-title">Replace your notes?</h4>
                {#if restore.step === 'unprotected'}
                  <p class="form-error" role="alert">{restoreError}</p>
                  <p>
                    Replace the notes on this computer anyway, without a backup
                    of them?
                  </p>
                {:else}
                  <p>
                    The notes on this computer will be replaced by the notes in
                    <span class="path">{restore.picked.file_name}</span>.
                    {status?.folder
                      ? 'A backup of your current notes is written to your backup folder first.'
                      : 'Your current notes aren’t backed up first, since no backup folder is set.'}
                  </p>
                {/if}
                <div class="buttons">
                  <button
                    type="button"
                    disabled={restoring}
                    onclick={cancelRestore}
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    class="danger"
                    disabled={restoring}
                    onclick={replaceNotes}
                  >
                    {restoring
                      ? 'Restoring…'
                      : restore.step === 'unprotected'
                        ? 'Replace without a backup'
                        : 'Replace notes'}
                  </button>
                </div>
              </div>
            {:else}
              <button type="button" disabled={restoring} onclick={pickBackup}>
                Restore from backup…
              </button>
              {#if restoreError}
                <p class="form-error" role="alert">{restoreError}</p>
              {/if}
            {/if}
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
              disabled={exporting}
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
        {:else if section === 'about'}
          <dl>
            <dt>Notebook</dt>
            <dd data-testid="app-version">
              {version === undefined ? '…' : formatVersion(version)}
            </dd>
          </dl>
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

  fieldset {
    margin: 0;
    padding: 0;
    border: none;
  }

  legend {
    padding: 0;
    margin-bottom: 0.5rem;
    font-weight: 600;
    font-size: 0.95rem;
  }

  .choices {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 1.25rem;
  }

  .toggle {
    display: flex;
    gap: 0.6rem;
    align-items: flex-start;
  }

  .options {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 1.25rem;
  }

  .options label {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  select,
  input[type='number'] {
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--control);
    border-radius: 6px;
    background: var(--bg);
    color: inherit;
    font: inherit;
  }

  input[type='number'] {
    width: 4.5rem;
  }

  .restore {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    align-self: stretch;
    padding: 0.75rem;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  h4 {
    margin: 0;
    font-size: 0.95rem;
  }

  .restore p {
    margin: 0;
  }

  .buttons {
    display: flex;
    gap: 0.5rem;
    justify-content: flex-end;
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
