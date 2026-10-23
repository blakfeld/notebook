<script lang="ts">
  import { onMount } from 'svelte'
  import { ACTION_LABELS, type NoteAction } from './actions.svelte'

  interface Props {
    actions: NoteAction[]
    /** Where to show it, in viewport pixels. */
    x: number
    y: number
    label: string
    onAction: (action: NoteAction) => void
    onClose: () => void
  }

  let { actions, x, y, label, onAction, onClose }: Props = $props()

  let menu: HTMLUListElement

  onMount(() => {
    menu.querySelector<HTMLElement>('[role="menuitem"]')?.focus()
    const away = (event: PointerEvent) => {
      if (!menu.contains(event.target as Node)) onClose()
    }
    document.addEventListener('pointerdown', away, true)
    return () => document.removeEventListener('pointerdown', away, true)
  })

  function onkeydown(event: KeyboardEvent) {
    const items = [...menu.querySelectorAll<HTMLElement>('[role="menuitem"]')]
    const index = items.indexOf(document.activeElement as HTMLElement)
    if (event.key === 'Escape' || event.key === 'Tab') {
      event.preventDefault()
      event.stopPropagation()
      onClose()
    } else if (event.key === 'ArrowDown') {
      event.preventDefault()
      items[(index + 1) % items.length]?.focus()
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      items[(index - 1 + items.length) % items.length]?.focus()
    }
  }
</script>

<ul
  class="menu"
  role="menu"
  aria-label={label}
  style:left="{x}px"
  style:top="{y}px"
  bind:this={menu}
  {onkeydown}
>
  {#each actions as action (action)}
    <li role="none">
      <button
        type="button"
        role="menuitem"
        class="plain"
        class:danger={action === 'delete'}
        onclick={() => onAction(action)}
      >
        {ACTION_LABELS[action]}
      </button>
    </li>
  {/each}
</ul>

<style>
  .menu {
    position: fixed;
    z-index: 5;
    min-width: 11rem;
    margin: 0;
    padding: 0.25rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.18);
    list-style: none;
  }

  button {
    width: 100%;
    padding: 0.35rem 0.6rem;
    border: none;
    text-align: left;
  }

  button:focus-visible {
    background: var(--hover);
  }

  .danger {
    color: var(--danger);
  }
</style>
