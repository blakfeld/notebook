<script lang="ts">
  import { onMount } from 'svelte'
  import OldPassphrase from './lib/setup/OldPassphrase.svelte'
  import RecoveryKeyPrompt from './lib/setup/RecoveryKeyPrompt.svelte'
  import RecoveryKeyShown from './lib/setup/RecoveryKeyShown.svelte'
  import Welcome from './lib/setup/Welcome.svelte'
  import Shell from './lib/shell/Shell.svelte'
  import type { PreferencesStore } from './lib/preferences'
  import { PhaseState } from './lib/state'
  import type { CoreApi, Phase } from './platform'

  interface Props {
    /** The platform's core, once loaded. */
    core: Promise<CoreApi>
    preferences: PreferencesStore
  }

  let { core, preferences }: Props = $props()

  let loaded = $state<{ core: CoreApi; phase: PhaseState } | null>(null)
  let error = $state('')
  /**
   * The recovery key `createNotebook` returned, held only until it is
   * acknowledged so the key screen needn't ask for it again.
   */
  let createdKey = $state<string | null>(null)

  onMount(() => {
    let phase: PhaseState | null = null
    core.then(
      (api) => {
        phase = new PhaseState(api)
        loaded = { core: api, phase }
        void phase.start()
      },
      (err) => (error = String(err)),
    )
    return () => phase?.stop()
  })

  let current = $derived(loaded?.phase.current ?? null)

  // Preferences are stored in the notebook's database on native clients,
  // so opening (or creating) it changes what is stored.
  let phaseName = $derived(current?.phase)
  let loadedFor: string | undefined
  $effect(() => {
    if (!phaseName || phaseName === loadedFor) return
    if (loadedFor !== undefined) void preferences.load()
    loadedFor = phaseName
  })

  function setPhase(phase: Phase) {
    loaded?.phase.set(phase)
  }

  function created(recoveryKey: string) {
    createdKey = recoveryKey
    // `createNotebook` returns the key, not the phase; the core also
    // reports the move in `app-phase-changed`, which may come later.
    if (current?.phase === 'setup') {
      setPhase({ ...current, phase: 'pending_recovery_key', upgrade: false })
    }
  }

  function acknowledged(phase: Phase) {
    createdKey = null
    setPhase(phase)
  }
</script>

{#if error || loaded?.phase.error}
  <p class="failed" role="alert">
    Notebook couldn’t start: {error || String(loaded?.phase.error)}
  </p>
{:else if loaded && current}
  {#if current.phase === 'open'}
    <Shell core={loaded.core} phase={loaded.phase} {preferences} />
  {:else}
    <main class="setup-page">
      {#if current.phase === 'setup'}
        <Welcome core={loaded.core} onCreated={created} />
      {:else if current.phase === 'pending_recovery_key'}
        <RecoveryKeyShown
          core={loaded.core}
          phase={current}
          recoveryKey={createdKey}
          onPhase={acknowledged}
        />
      {:else if current.phase === 'needs_recovery_key'}
        <RecoveryKeyPrompt
          core={loaded.core}
          phase={current}
          onPhase={setPhase}
        />
      {:else if current.phase === 'needs_old_passphrase'}
        <OldPassphrase core={loaded.core} onPhase={setPhase} />
      {/if}
    </main>
  {/if}
{/if}

<style>
  .failed {
    margin: 2rem;
    color: var(--danger);
  }
</style>
