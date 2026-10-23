import { afterEach, describe, expect, it, vi } from 'vitest'
import { FakeCoreApi } from '../../platform/fake'
import { memorySettings } from '../../platform/settings'
import { createPreferences } from '../preferences'
import {
  createVault,
  PhaseState,
  SEARCH_DEBOUNCE_MS,
  startVault,
  stopVault,
} from '.'

function opened(notes: ConstructorParameters<typeof FakeCoreApi>[0] = []) {
  const core = new FakeCoreApi(notes)
  core.phase = { phase: 'open', secure_store: true, upgrade: false }
  const phase = new PhaseState(core)
  const vault = createVault(core, phase, createPreferences(memorySettings()))
  return { core, phase, vault }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('vault state', () => {
  it('starts on Notes with the list and tree loaded', async () => {
    const { vault } = opened([
      { id: 'a', body: '# A\n#work' },
      { id: 'b', body: 'B', archived: true },
    ])
    await startVault(vault)
    expect(vault.selection.current).toEqual({ view: 'notes' })
    expect(vault.notes.entries.map((e) => e.id)).toEqual(['a'])
    expect(vault.notes.tree.map((n) => n.path)).toEqual(['work'])
    stopVault(vault)
  })

  it('re-queries the list and tree on vault-changed', async () => {
    const { core, vault } = opened([{ id: 'a', body: 'A' }])
    await startVault(vault)
    // A change the frontend didn't make, e.g. from another device.
    core.seed({ id: 'b', body: '# B\n#ideas' })
    expect(vault.notes.entries.map((e) => e.id)).toEqual(['a'])

    const lists = core.callsTo('listNotes').length
    const trees = core.callsTo('tagTree').length
    core.emit('vault-changed', { changed: ['b'], tree_changed: true })
    await vi.waitFor(() =>
      expect(vault.notes.entries.map((e) => e.id)).toEqual(['b', 'a']),
    )
    expect(vault.notes.tree.map((n) => n.path)).toEqual(['ideas'])
    expect(core.callsTo('listNotes').length).toBe(lists + 1)
    expect(core.callsTo('tagTree').length).toBe(trees + 1)

    stopVault(vault)
    core.emit('vault-changed', { changed: [], tree_changed: true })
    expect(core.callsTo('listNotes').length).toBe(lists + 1)
  })

  it('re-queries only the list when the tree is unchanged', async () => {
    const { core, vault } = opened([{ id: 'a', body: 'A' }])
    await startVault(vault)
    const lists = core.callsTo('listNotes').length
    const trees = core.callsTo('tagTree').length
    core.emit('vault-changed', { changed: ['a'], tree_changed: false })
    await vi.waitFor(() =>
      expect(core.callsTo('listNotes').length).toBe(lists + 1),
    )
    expect(core.callsTo('tagTree').length).toBe(trees)
    stopVault(vault)
  })

  it('retries a failed tree query on the next change', async () => {
    const { core, vault } = opened([{ id: 'a', body: 'A' }])
    await startVault(vault)
    core.failNext('tagTree', 'internal')
    core.emit('vault-changed', { changed: ['a'], tree_changed: true })
    await vi.waitFor(() => expect(vault.notes.error).not.toBeNull())
    const trees = core.callsTo('tagTree').length
    core.emit('vault-changed', { changed: ['a'], tree_changed: false })
    await vi.waitFor(() =>
      expect(core.callsTo('tagTree').length).toBe(trees + 1),
    )
    stopVault(vault)
  })

  it('debounces the search query', async () => {
    vi.useFakeTimers()
    const { core, vault } = opened([
      { id: 'a', body: 'plumber sink' },
      { id: 'b', body: 'plumber' },
    ])
    await startVault(vault)
    const before = core.callsTo('listNotes').length

    vault.notes.setQuery('p')
    vault.notes.setQuery('plumber')
    vault.notes.setQuery('plumber sink')
    expect(vault.notes.query).toBe('plumber sink')
    await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS - 10)
    expect(core.callsTo('listNotes').length).toBe(before)

    await vi.advanceTimersByTimeAsync(20)
    expect(core.callsTo('listNotes').slice(before)).toEqual([
      [{ view: 'notes' }, 'plumber sink'],
    ])
    expect(vault.notes.entries.map((e) => e.id)).toEqual(['a'])
    stopVault(vault)
  })

  it('re-lists on a selection change, closing the open note', async () => {
    const { vault } = opened([
      { id: 'a', body: 'A' },
      { id: 't', body: 'T', trashed: true },
    ])
    await startVault(vault)
    vault.notes.openNote('a')
    vault.selection.select({ view: 'trash' })
    expect(vault.selection.noteId).toBeNull()
    await vi.waitFor(() =>
      expect(vault.notes.entries.map((e) => e.id)).toEqual(['t']),
    )
    stopVault(vault)
  })

  it('loads and writes settings', async () => {
    const { core, vault } = opened()
    core.settings = {
      ...core.settings,
      vim_mode: true,
      collapsed_tags: ['work'],
    }
    await startVault(vault)
    expect(vault.settings.vimMode).toBe(true)
    expect(vault.settings.isExpanded('Work')).toBe(false)
    await vault.settings.setExpanded('Work', true)
    expect(vault.settings.isExpanded('work')).toBe(true)
    expect(core.callsTo('setTagExpanded')).toEqual([['Work', true]])
    await vault.settings.setVimMode(false)
    expect(core.callsTo('setVimMode')).toEqual([[false]])
    stopVault(vault)
  })
})

describe('PhaseState', () => {
  it('reads the phase and follows app-phase-changed', async () => {
    const core = new FakeCoreApi()
    const phase = new PhaseState(core)
    await phase.start()
    expect(phase.current?.phase).toBe('setup')
    core.setPhase('pending_recovery_key', true)
    expect(phase.current).toEqual({
      phase: 'pending_recovery_key',
      secure_store: true,
      upgrade: true,
    })
    phase.stop()
    expect(core.listenerCount('app-phase-changed')).toBe(0)
  })
})
