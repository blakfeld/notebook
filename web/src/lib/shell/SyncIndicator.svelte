<script lang="ts">
  import type { SyncStatus } from '../../platform'
  import { syncSummary } from '../format'

  interface Props {
    status: SyncStatus | null
  }

  let { status }: Props = $props()

  let summary = $derived(syncSummary(status))
</script>

<div class="sync" role="status" aria-live="polite">
  <div class="line">
    <span class="dot {status?.state ?? 'syncing'}" aria-hidden="true"></span>
    <span class="label">{summary.label}</span>
    {#if summary.pending}
      <span class="pending">· {summary.pending}</span>
    {/if}
  </div>
  {#each summary.alerts as alert (alert)}
    <p class="alert">{alert}</p>
  {/each}
</div>

<style>
  .sync {
    min-width: 0;
    font-size: 0.85rem;
  }

  .line {
    display: flex;
    gap: 0.35rem;
    align-items: center;
  }

  .dot {
    flex: none;
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--muted);
  }

  .dot.synced {
    background: #2da44e;
  }

  .dot.syncing {
    background: var(--accent);
  }

  .pending {
    color: var(--muted);
  }

  .alert {
    margin: 0.2rem 0 0;
    color: var(--warning);
  }
</style>
