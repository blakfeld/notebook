import { getContext, setContext } from 'svelte'
import type { CoreApi } from '../../platform'
import type { PreferencesStore } from '../preferences'
import { NotesState } from './notes.svelte'
import { PhaseState } from './phase.svelte'
import { SelectionState } from './selection.svelte'
import { SettingsState } from './settings.svelte'

export { NotesState, SEARCH_DEBOUNCE_MS } from './notes.svelte'
export { PhaseState } from './phase.svelte'
export { SelectionState, sameSelection, selectedTag } from './selection.svelte'
export { SettingsState } from './settings.svelte'

/**
 * The state of the open notebook. The shell creates it when the notebook
 * opens, so every launch starts on Notes.
 */
export interface Vault {
  core: CoreApi
  phase: PhaseState
  selection: SelectionState
  notes: NotesState
  settings: SettingsState
  /** Theme and editor view preferences; these outlive the vault. */
  preferences: PreferencesStore
  /** The mounted editor, set by the editor pane, for flushing edits. */
  editor: { flush(): Promise<boolean> } | null
}

export function createVault(
  core: CoreApi,
  phase: PhaseState,
  preferences: PreferencesStore,
): Vault {
  const selection = new SelectionState()
  return {
    core,
    phase,
    selection,
    notes: new NotesState(core, selection),
    settings: new SettingsState(core),
    preferences,
    editor: null,
  }
}

/**
 * Delivers the open editor's pending edits. Called before anything that
 * would strand them, such as trashing the open note. Returns false if
 * delivery failed (the editor reports why).
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
  await Promise.all([vault.notes.start(), vault.settings.load()])
}

export function stopVault(vault: Vault): void {
  vault.notes.stop()
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
