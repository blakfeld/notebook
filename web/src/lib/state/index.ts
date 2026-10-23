import { getContext, setContext } from 'svelte'
import type { CoreApi } from '../../platform'
import { NotesState } from './notes.svelte'
import { PhaseState } from './phase.svelte'
import { SelectionState } from './selection.svelte'
import { SettingsState } from './settings.svelte'
import { SyncStatusState } from './sync.svelte'

export { NotesState, SEARCH_DEBOUNCE_MS } from './notes.svelte'
export { PhaseState } from './phase.svelte'
export { SelectionState, sameSelection, selectedTag } from './selection.svelte'
export { SettingsState } from './settings.svelte'
export { SyncStatusState } from './sync.svelte'

/**
 * The state of one unlocked session. The shell creates it when the vault
 * opens and drops it on lock, so nothing outlives the unlock and every
 * unlock starts on Notes.
 */
export interface Vault {
  core: CoreApi
  phase: PhaseState
  selection: SelectionState
  notes: NotesState
  sync: SyncStatusState
  settings: SettingsState
  /** The mounted editor, set by the editor pane, for flushing edits. */
  editor: { flush(): Promise<boolean> } | null
}

export function createVault(core: CoreApi, phase: PhaseState): Vault {
  const selection = new SelectionState()
  return {
    core,
    phase,
    selection,
    notes: new NotesState(core, selection),
    sync: new SyncStatusState(core),
    settings: new SettingsState(core),
    editor: null,
  }
}

/**
 * Delivers the open editor's pending edits. Called before anything that
 * would strand them: locking, signing out, trashing the open note.
 * Returns false if delivery failed (the editor reports why).
 */
export async function flushEditor(vault: Vault): Promise<boolean> {
  try {
    return (await vault.editor?.flush()) ?? true
  } catch {
    return false
  }
}

/** Subscribes and loads everything. */
export async function startVault(vault: Vault): Promise<void> {
  await Promise.all([
    vault.notes.start(),
    vault.sync.start(),
    vault.settings.load(),
  ])
}

export function stopVault(vault: Vault): void {
  vault.notes.stop()
  vault.sync.stop()
}

/** Delivers pending edits, then locks. Rejects if locking failed. */
export async function lockVault(vault: Vault): Promise<void> {
  await flushEditor(vault)
  vault.phase.set(await vault.core.lock())
}

const VAULT = Symbol('vault')

export function setVault(vault: Vault): void {
  setContext(VAULT, vault)
}

/** The vault of the enclosing shell. */
export function useVault(): Vault {
  const vault = getContext<Vault | undefined>(VAULT)
  if (!vault) throw new Error('useVault outside the shell')
  return vault
}
