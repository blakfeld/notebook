import { FakeCoreApi } from './fake'
import type { CoreError } from './types'

/**
 * The fake core, open, with a few sample notes, for viewing the UI with
 * `npm run dev` in a plain browser. Dev builds only; never part of a
 * production bundle.
 */
export function demoCore(): FakeCoreApi {
  const core = new FakeCoreApi([
    {
      body: '# Weekly plan\n\nNotes for #work/acme and #home this week.\n\n- [ ] call the plumber about the sink',
      pinned: true,
    },
    { body: '# Groceries\n- eggs\n- milk\n#home' },
    { body: 'Standup notes\nShipped the tag tree. #work' },
    { body: '' },
    { body: '# Old idea\nSomething for later. #ideas', archived: true },
    { body: '# Draft\nNot needed any more.', trashed: true },
  ])
  core.phase = { phase: 'open', secure_store: true, upgrade: false }
  core.latency = 50
  // The browser's zip download comes with `browser-web-client`.
  core.exportNotes = async () => {
    throw {
      code: 'export_failed',
      message: 'export isn’t available in the browser yet',
    } satisfies CoreError
  }
  // Backups need the desktop app's folder and file pickers.
  const unavailable = async (): Promise<never> => {
    throw {
      code: 'backup_failed',
      message: 'backups aren’t available in the browser',
    } satisfies CoreError
  }
  core.chooseBackupFolder = unavailable
  core.backupNow = unavailable
  core.pickBackup = unavailable
  return core
}
