import { beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.fn()
const listen = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke, isTauri: () => true }))
vi.mock('@tauri-apps/api/event', () => ({ listen }))

const { tauriCore } = await import('./tauri')

describe('tauriCore', () => {
  beforeEach(() => {
    invoke.mockReset()
    listen.mockReset()
  })

  it('sends commands with the host’s camelCase argument names', async () => {
    invoke.mockResolvedValue(undefined)
    const core = tauriCore()
    await core.createNotebook()
    await core.pendingRecoveryKey()
    await core.acknowledgeRecoveryKey()
    await core.openWithRecoveryKey('k')
    await core.upgradeWithPassphrase('p')
    await core.startOver()
    await core.coreVersion()
    await core.listNotes({ tag: 'work' }, 'q')
    await core.renameTag('work', 'job')
    await core.editorPush(3, 'AAE=')
    await core.setTagExpanded('work', false)
    expect(invoke.mock.calls).toEqual([
      ['create_notebook', undefined],
      ['pending_recovery_key', undefined],
      ['acknowledge_recovery_key', undefined],
      ['open_with_recovery_key', { recoveryKey: 'k' }],
      ['upgrade_with_passphrase', { passphrase: 'p' }],
      ['start_over', undefined],
      ['core_version', undefined],
      ['list_notes', { selection: { tag: 'work' }, query: 'q' }],
      ['rename_tag', { old: 'work', new: 'job' }],
      ['editor_push', { session: 3, update: 'AAE=' }],
      ['set_tag_expanded', { path: 'work', expanded: false }],
    ])
  })

  it('passes host errors through and wraps anything else', async () => {
    const core = tauriCore()
    invoke.mockRejectedValueOnce({ code: 'recovery_key_wrong', message: 'x' })
    await expect(core.openWithRecoveryKey('k')).rejects.toEqual({
      code: 'recovery_key_wrong',
      message: 'x',
    })
    invoke.mockRejectedValueOnce('command open_with_recovery_key not found')
    await expect(core.openWithRecoveryKey('k')).rejects.toEqual({
      code: 'internal',
      message: 'command open_with_recovery_key not found',
    })
  })

  it('delivers event payloads and unsubscribes, even before listen resolves', async () => {
    let handler: ((e: { payload: unknown }) => void) | undefined
    const unlisten = vi.fn()
    let resolve!: (fn: () => void) => void
    listen.mockImplementation((_event, h) => {
      handler = h
      return new Promise((r) => (resolve = r))
    })
    const core = tauriCore()
    const seen: unknown[] = []
    const off = core.on('vault-changed', (payload) => seen.push(payload))
    expect(listen.mock.calls[0][0]).toBe('vault-changed')
    handler!({ payload: { changed: ['a'], tree_changed: false } })
    expect(seen).toEqual([{ changed: ['a'], tree_changed: false }])

    let ready = false
    void off.ready.then(() => (ready = true))
    await Promise.resolve()
    expect(ready).toBe(false)

    off()
    resolve(unlisten)
    await off.ready
    expect(ready).toBe(true)
    expect(unlisten).toHaveBeenCalledOnce()
  })

  it('rejects `ready` when subscribing fails', async () => {
    listen.mockRejectedValue(new Error('no permission'))
    const off = tauriCore().on('app-phase-changed', () => {})
    await expect(off.ready).rejects.toThrow('no permission')
  })
})

describe('openExternal on Tauri', () => {
  it('opens through the opener plugin, and only allowed schemes', async () => {
    const openUrl = vi.fn().mockResolvedValue(undefined)
    vi.doMock('@tauri-apps/plugin-opener', () => ({ openUrl }))
    const { openExternal } = await import('./external')
    expect(await openExternal('mailto:sam@example.com')).toBe(true)
    expect(await openExternal('file:///etc/passwd')).toBe(false)
    expect(openUrl.mock.calls).toEqual([['mailto:sam@example.com']])
  })
})
