<script lang="ts">
  import { useVault } from '../state'

  interface Props {
    /** Opens the Backup settings. */
    onOpen: () => void
  }

  let { onOpen }: Props = $props()

  const { backup } = useVault()
</script>

<!-- Shown only when backups need attention: failing, or overdue. -->
{#if backup.needsAttention}
  <button
    type="button"
    class="plain notice"
    title="Open Backup settings"
    onclick={onOpen}
  >
    {backup.status?.failing ? 'Backups are failing' : 'Backup overdue'}
  </button>
{/if}

<style>
  .notice {
    max-width: 100%;
    padding: 0.2rem 0.4rem;
    overflow: hidden;
    color: var(--warning);
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
