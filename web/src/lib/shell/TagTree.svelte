<script lang="ts">
  import { tick } from 'svelte'
  import type { TagNode } from '../../platform'
  import { useVault } from '../state'
  import TagTree from './TagTree.svelte'
  import type { TagRenamer } from './tagRename.svelte'

  interface Props {
    nodes: TagNode[]
    renamer: TagRenamer
    depth?: number
  }

  let { nodes, renamer, depth = 0 }: Props = $props()

  const { selection, settings } = useVault()

  function selected(node: TagNode): boolean {
    const current = selection.current
    return (
      'tag' in current && current.tag.toLowerCase() === node.path.toLowerCase()
    )
  }

  async function startRename(node: TagNode) {
    renamer.start(node.path)
    await tick()
    const input = document.getElementById(inputId(node))
    if (input instanceof HTMLInputElement) {
      input.focus()
      input.select()
    }
  }

  function inputId(node: TagNode): string {
    return `rename-${node.path.toLowerCase()}`
  }

  function onRenameKey(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault()
      renamer.cancel()
    }
  }

  function onItemKey(event: KeyboardEvent, node: TagNode) {
    if (event.key === 'F2') {
      event.preventDefault()
      void startRename(node)
    }
  }
</script>

<ul class="tags" class:nested={depth > 0}>
  {#each nodes as node (node.path.toLowerCase())}
    {@const expanded = settings.isExpanded(node.path)}
    <li>
      <div class="row" style:padding-left="{depth * 0.9}rem">
        {#if node.children.length > 0}
          <button
            type="button"
            class="toggle plain"
            aria-expanded={expanded}
            aria-label="{expanded ? 'Collapse' : 'Expand'} {node.name}"
            onclick={() => settings.setExpanded(node.path, !expanded)}
          >
            <span class="chevron" class:open={expanded} aria-hidden="true"
              >›</span
            >
          </button>
        {:else}
          <span class="toggle" aria-hidden="true"></span>
        {/if}

        {#if renamer.path === node.path}
          <form
            class="rename-form"
            onsubmit={(event) => {
              event.preventDefault()
              void renamer.commit()
            }}
          >
            <input
              id={inputId(node)}
              type="text"
              autocapitalize="off"
              spellcheck="false"
              aria-label="New path for tag {node.path}"
              aria-invalid={renamer.error ? 'true' : undefined}
              aria-describedby={renamer.error ? 'rename-error' : undefined}
              readonly={renamer.busy}
              bind:value={renamer.value}
              onkeydown={onRenameKey}
              onblur={() => {
                if (!renamer.error && !renamer.busy) renamer.cancel()
              }}
            />
          </form>
        {:else}
          <button
            type="button"
            class="item plain"
            data-nav-item
            aria-current={selected(node) ? 'true' : undefined}
            title="Double-click or press F2 to rename"
            onclick={() => selection.select({ tag: node.path })}
            ondblclick={() => startRename(node)}
            onkeydown={(event) => onItemKey(event, node)}
          >
            <span class="name">{node.name}</span>
            <span class="count">{node.count}</span>
          </button>
          <button
            type="button"
            class="rename plain"
            aria-label="Rename tag {node.path}"
            onclick={() => startRename(node)}
          >
            <span aria-hidden="true">✎</span>
          </button>
        {/if}
      </div>
      {#if renamer.path === node.path && renamer.error}
        <p class="field-error" id="rename-error" role="alert">
          {renamer.error}
        </p>
      {/if}
      {#if node.children.length > 0 && expanded}
        <TagTree nodes={node.children} {renamer} depth={depth + 1} />
      {/if}
    </li>
  {/each}
</ul>

<style>
  .tags {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.1rem;
  }

  .toggle {
    flex: none;
    width: 1.4rem;
    padding: 0;
    text-align: center;
  }

  .chevron {
    display: inline-block;
    transition: transform 0.1s;
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  .item {
    display: flex;
    flex: 1;
    min-width: 0;
    gap: 0.5rem;
    justify-content: space-between;
    padding: 0.3rem 0.5rem;
    text-align: left;
  }

  .item[aria-current='true'] {
    background: var(--selected);
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .count {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .rename {
    flex: none;
    padding: 0.2rem 0.4rem;
    color: var(--muted);
    opacity: 0;
  }

  .row:hover .rename,
  .rename:focus-visible {
    opacity: 1;
  }

  .rename-form {
    flex: 1;
  }

  .rename-form input {
    padding: 0.2rem 0.4rem;
  }

  .field-error {
    padding: 0.2rem 0.5rem 0.4rem 1.6rem;
    font-size: 0.9em;
  }
</style>
