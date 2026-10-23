<script lang="ts">
  interface Props {
    /** `credentials_remembered` from the app phase. */
    remembered: boolean
  }

  let { remembered }: Props = $props()

  let dismissed = $state(false)
</script>

{#if !remembered && !dismissed}
  <div class="notice" role="status">
    <p>
      This device has no secure credential store, so Notebook can’t remember
      your sign-in. You’ll need to sign in each time you open the app.
    </p>
    <button type="button" class="plain" onclick={() => (dismissed = true)}>
      Dismiss
    </button>
  </div>
{/if}

<style>
  .notice {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    padding: 0.5rem 1rem;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  p {
    flex: 1;
    margin: 0;
  }
</style>
