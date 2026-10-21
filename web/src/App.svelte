<script lang="ts">
  import { formatCoreVersion } from './lib/version'
  import { coreVersion } from './platform'

  const versionLine = coreVersion().then(formatCoreVersion)
</script>

<main>
  <h1>Notebook</h1>
  {#await versionLine}
    <p class="version">…</p>
  {:then line}
    <p class="version">{line}</p>
  {:catch error}
    <p class="version">core unavailable: {String(error)}</p>
  {/await}
</main>

<style>
  main {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
  }

  h1 {
    margin: 0;
  }

  .version {
    color: var(--muted);
  }
</style>
