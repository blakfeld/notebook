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
    await core.register({
      server: 's',
      username: 'u',
      passphrase: 'p',
      confirm: 'p',
      inviteCode: 'i',
    })
    await core.recover({
      server: 's',
      username: 'u',
      recoveryKey: 'k',
      passphrase: 'p',
      confirm: 'p',
    })
    await core.listNotes({ tag: 'work' }, 'q')
    await core.renameTag('work', 'job')
    await core.editorPush(3, 'AAE=')
    await core.setTagExpanded('work', false)
    await core.signOut(true)
    await core.exportNotes({ includeTrashed: true })
    expect(invoke.mock.calls).toEqual([
      [
        'register',
        {
          server: 's',
          username: 'u',
          passphrase: 'p',
          confirm: 'p',
          inviteCode: 'i',
        },
      ],
      [
        'recover',
        {
          server: 's',
          username: 'u',
          recoveryKey: 'k',
          passphrase: 'p',
          confirm: 'p',
        },
      ],
      ['list_notes', { selection: { tag: 'work' }, query: 'q' }],
      ['rename_tag', { old: 'work', new: 'job' }],
      ['editor_push', { session: 3, update: 'AAE=' }],
      ['set_tag_expanded', { path: 'work', expanded: false }],
      ['sign_out', { confirm: true }],
      ['export_notes', { includeTrashed: true }],
    ])
  })

  it('returns the export result, or null when the picker is cancelled', async () => {
    const core = tauriCore()
    invoke.mockResolvedValueOnce({ count: 2, path: '/tmp/notebook-export-x' })
    await expect(core.exportNotes({ includeTrashed: false })).resolves.toEqual({
      count: 2,
      path: '/tmp/notebook-export-x',
    })
    invoke.mockResolvedValueOnce(null)
    await expect(
      core.exportNotes({ includeTrashed: false }),
    ).resolves.toBeNull()
  })

  it('passes host errors through and wraps anything else', async () => {
    const core = tauriCore()
    invoke.mockRejectedValueOnce({ code: 'bad_credentials', message: 'x' })
    await expect(core.signIn('s', 'u', 'p')).rejects.toEqual({
      code: 'bad_credentials',
      message: 'x',
    })
    invoke.mockRejectedValueOnce('command sign_in not found')
    await expect(core.signIn('s', 'u', 'p')).rejects.toEqual({
      code: 'internal',
      message: 'command sign_in not found',
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
    const off = tauriCore().on('sync-status', () => {})
    await expect(off.ready).rejects.toThrow('no permission')
  })
})
