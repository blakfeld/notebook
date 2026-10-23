<script lang="ts">
  import { onMount } from 'svelte'
  import AccountScreens from './lib/account/AccountScreens.svelte'
  import Unlock from './lib/account/Unlock.svelte'
  import Shell from './lib/shell/Shell.svelte'
  import type { PreferencesStore } from './lib/preferences'
  import { PhaseState } from './lib/state'
  import type { CoreApi } from './platform'

  interface Props {
    /** The platform's core, once loaded. */
    core: Promise<CoreApi>
    preferences: PreferencesStore
  }

  let { core, preferences }: Props = $props()

  let loaded = $state<{ core: CoreApi; phase: PhaseState } | null>(null)
  let error = $state('')

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

  // Preferences are stored with the account on native clients, so signing
  // in or out changes what is stored.
  let phaseName = $derived(current?.phase)
  let loadedFor: string | undefined
  $effect(() => {
    if (!phaseName || phaseName === loadedFor) return
    if (loadedFor !== undefined) void preferences.load()
    loadedFor = phaseName
  })
</script>

{#if error || loaded?.phase.error}
  <p class="failed" role="alert">
    Notebook couldn’t start: {error || String(loaded?.phase.error)}
  </p>
{:else if loaded && current}
  {#if current.phase === 'unlocked'}
    <Shell core={loaded.core} phase={loaded.phase} {preferences} />
  {:else if current.phase === 'locked'}
    <Unlock
      core={loaded.core}
      account={current.account}
      onPhase={(p) => loaded?.phase.set(p)}
    />
  {:else}
    <AccountScreens core={loaded.core} onPhase={(p) => loaded?.phase.set(p)} />
  {/if}
{/if}

<style>
  .failed {
    margin: 2rem;
    color: var(--danger);
  }
</style>
