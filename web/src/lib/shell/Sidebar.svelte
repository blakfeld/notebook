<script lang="ts">
  import type { View } from '../../platform'
  import { useVault } from '../state'
  import BackupNotice from './BackupNotice.svelte'
  import type { SettingsSection } from './Settings.svelte'
  import TagTree from './TagTree.svelte'
  import { TagRenamer } from './tagRename.svelte'

  interface Props {
    /** Opens Settings, at `section` if given. */
    onOpenSettings: (section?: SettingsSection) => void
  }

  let { onOpenSettings }: Props = $props()

  const vault = useVault()
  const { selection, notes } = vault
  const renamer = new TagRenamer(vault)

  const VIEWS: { view: View; label: string }[] = [
    { view: 'notes', label: 'Notes' },
    { view: 'untagged', label: 'Untagged' },
    { view: 'pinned', label: 'Pinned' },
    { view: 'archive', label: 'Archive' },
    { view: 'trash', label: 'Trash' },
  ]

  function isSelected(view: View): boolean {
    const current = selection.current
    return 'view' in current && current.view === view
  }

  // Up and Down move between the sidebar's items.
  function onkeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
    const target = event.target as HTMLElement
    if (!target.matches('[data-nav-item]')) return
    const items = [
      ...(event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>(
        '[data-nav-item]',
      ),
    ]
    const next =
      items[items.indexOf(target) + (event.key === 'ArrowDown' ? 1 : -1)]
    if (next) {
      event.preventDefault()
      next.focus()
    }
  }
</script>

<aside class="sidebar">
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <nav aria-label="Library" {onkeydown}>
    <ul class="views">
      {#each VIEWS as { view, label } (view)}
        <li>
          <button
            type="button"
            class="item plain"
            data-nav-item
            aria-current={isSelected(view) ? 'true' : undefined}
            onclick={() => selection.select({ view })}
          >
            {label}
          </button>
        </li>
      {/each}
    </ul>
    {#if notes.tree.length > 0}
      <h2 class="heading" id="tags-heading">Tags</h2>
      <div aria-labelledby="tags-heading" role="group">
        <TagTree nodes={notes.tree} {renamer} />
      </div>
    {/if}
  </nav>
  <footer>
    <div class="status">
      <BackupNotice onOpen={() => onOpenSettings('backup')} />
    </div>
    <button
      type="button"
      class="plain settings"
      aria-label="Settings"
      title="Settings"
      onclick={() => onOpenSettings()}
    >
      <span aria-hidden="true">⚙</span>
    </button>
  </footer>
</aside>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border-right: 1px solid var(--border);
    background: var(--surface);
  }

  nav {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0.75rem 0.5rem;
  }

  .views {
    margin: 0 0 1rem;
    padding: 0;
    list-style: none;
  }

  .item {
    width: 100%;
    padding: 0.3rem 0.5rem 0.3rem 1.5rem;
    text-align: left;
  }

  .item[aria-current='true'] {
    background: var(--selected);
  }

  .heading {
    margin: 0 0 0.25rem;
    padding: 0 0.5rem;
    color: var(--muted);
    font-size: 0.8rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  footer {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem 0.75rem;
    border-top: 1px solid var(--border);
  }

  .status {
    min-width: 0;
    font-size: 0.85rem;
  }

  .settings {
    padding: 0.2rem 0.45rem;
    font-size: 1.05rem;
  }
</style>
