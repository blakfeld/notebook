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
    await core.exportNotes({ includeTrashed: true })
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

  it('sends the backup commands with the contract’s names and arguments', async () => {
    invoke.mockResolvedValue(null)
    const core = tauriCore()
    await core.backupStatus()
    await core.chooseBackupFolder()
    await core.setBackupOptions({ auto: false, interval: 'weekly', keep: 3 })
    await core.backupNow()
    await core.pickBackup()
    await core.cancelRestore()
    await core.restoreBackup({ recoveryKey: 'k', skipSafetyBackup: false })
    await core.restoreBackup({ recoveryKey: null, skipSafetyBackup: true })
    expect(invoke.mock.calls).toEqual([
      ['backup_status', undefined],
      ['choose_backup_folder', undefined],
      ['set_backup_options', { auto: false, interval: 'weekly', keep: 3 }],
      ['backup_now', undefined],
      ['pick_backup', undefined],
      ['cancel_restore', undefined],
      ['restore_backup', { recoveryKey: 'k', skipSafetyBackup: false }],
      ['restore_backup', { recoveryKey: null, skipSafetyBackup: true }],
    ])
  })

  it('returns backup results as the host sends them, null when cancelled', async () => {
    const core = tauriCore()
    const status = {
      folder: '/Users/me/Dropbox/Notebook',
      auto: true,
      interval: 'daily',
      keep: 14,
      last: 1792677789000,
      next_due: 1792764189000,
      changed: false,
      last_error: null,
      failing: false,
      overdue: false,
      running: false,
    }
    invoke.mockResolvedValueOnce(status)
    await expect(core.backupStatus()).resolves.toEqual(status)
    invoke.mockResolvedValueOnce(null)
    await expect(core.chooseBackupFolder()).resolves.toBeNull()
    const picked = { file_name: 'notebook-backup-x.nbbackup', same_vault: true }
    invoke.mockResolvedValueOnce(picked)
    await expect(core.pickBackup()).resolves.toEqual(picked)
    invoke.mockResolvedValueOnce(null)
    await expect(core.pickBackup()).resolves.toBeNull()
    invoke.mockRejectedValueOnce({
      code: 'safety_backup_failed',
      message: 'the backup folder is unavailable',
    })
    await expect(
      core.restoreBackup({ recoveryKey: null, skipSafetyBackup: false }),
    ).rejects.toEqual({
      code: 'safety_backup_failed',
      message: 'the backup folder is unavailable',
    })
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
