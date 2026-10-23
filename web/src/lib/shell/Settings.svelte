<script lang="ts">
  import { onMount } from 'svelte'
  import { useVault } from '../state'
  import { formatVersion } from '../version'
  import Modal from './Modal.svelte'

  interface Props {
    onClose: () => void
  }

  let { onClose }: Props = $props()

  const { core, settings, preferences } = useVault()

  /**
   * The screen's sections, in order. Other capabilities add theirs here
   * (Export from `markdown-export`, Backup from `vault-backup`).
   */
  const SECTIONS = [
    { id: 'appearance', label: 'Appearance' },
    { id: 'editor', label: 'Editor' },
    { id: 'about', label: 'About' },
  ] as const

  const THEMES = [
    { value: 'system', label: 'System' },
    { value: 'light', label: 'Light' },
    { value: 'dark', label: 'Dark' },
  ] as const
  type Section = (typeof SECTIONS)[number]['id']

  let section = $state<Section>('editor')

  /** `undefined` while loading, `null` if it couldn't be read. */
  let version = $state<string | null | undefined>(undefined)

  onMount(() => {
    core.coreVersion().then(
      (v) => (version = v),
      () => (version = null),
    )
  })
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
