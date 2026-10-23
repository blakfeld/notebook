<script lang="ts">
  import { onMount, type Snippet } from 'svelte'

  interface Props {
    /** The id of the element that names the dialog. */
    labelledby: string
    onClose: () => void
    children: Snippet
  }

  let { labelledby, onClose, children }: Props = $props()

  let dialog: HTMLDivElement

  const FOCUSABLE =
    'button:not(:disabled), input:not(:disabled), [href], [tabindex]:not([tabindex="-1"])'

  // Focus moves into the dialog and back to where it was on close.
  onMount(() => {
    const previous = document.activeElement as HTMLElement | null
    const first =
      dialog.querySelector<HTMLElement>('[data-autofocus]') ??
      dialog.querySelector<HTMLElement>(FOCUSABLE)
    ;(first ?? dialog).focus()
    return () => previous?.focus?.()
  })

  function onkeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.stopPropagation()
      onClose()
    } else if (event.key === 'Tab') {
      // Keep Tab inside the dialog.
      const items = [...dialog.querySelectorAll<HTMLElement>(FOCUSABLE)]
      if (items.length === 0) return
      const [first, last] = [items[0], items[items.length - 1]]
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault()
        last.focus()
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault()
        first.focus()
      }
    }
  }
</script>

<div class="backdrop">
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby={labelledby}
    tabindex="-1"
    bind:this={dialog}
    {onkeydown}
  >
    {@render children()}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: grid;
    place-items: center;
    padding: 1rem;
    background: rgb(0 0 0 / 0.35);
  }

  .dialog {
    box-sizing: border-box;
    max-width: 100%;
    max-height: calc(100vh - 2rem);
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg);
    box-shadow: 0 8px 32px rgb(0 0 0 / 0.25);
  }
</style>
