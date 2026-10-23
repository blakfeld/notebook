// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import App from '../../App.svelte'
import '../../editor/testing'
import type { Phase } from '../../platform'
import { FakeCoreApi, type SeedNote } from '../../platform/fake'
import { coreSettings } from '../../platform/settings'
import { createPreferences } from '../preferences'
import {
  alerts,
  button,
  check,
  click,
  field,
  fill,
  hasButton,
  render,
  settle,
} from '../testing'
import RecoveryKeyShown from './RecoveryKeyShown.svelte'

const cleanups: (() => void)[] = []
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup()
})

const ACK = 'I’ve saved my recovery key somewhere safe'

/** The app on a fake core that starts in `phase`. */
async function app(phase: Partial<Phase> = {}, notes: SeedNote[] = []) {
  const core = new FakeCoreApi(notes)
  core.phase = { phase: 'setup', secure_store: true, upgrade: false, ...phase }
  const preferences = createPreferences(coreSettings(core))
  const view = render(App, { core: Promise.resolve(core), preferences })
  cleanups.push(view.destroy)
  await settle()
  return { core, root: view.target }
}

const text = (root: HTMLElement) =>
  (root.textContent ?? '').replace(/\s+/g, ' ')

const shownKey = (root: HTMLElement) =>
  root.querySelector('[data-testid="recovery-key"]')?.textContent ?? null

/** True once the main layout (sidebar, list, editor) shows. */
const inShell = (root: HTMLElement) => root.querySelector('.shell') !== null

const titles = (root: HTMLElement) =>
  [...root.querySelectorAll('[role="option"] .entry-title')].map((el) =>
    el.textContent!.trim(),
  )

async function selectView(root: HTMLElement, label: string) {
  click(button(root.querySelector('nav')!, label))
  await settle()
}

async function acknowledge(root: HTMLElement) {
  check(field(root, ACK))
  click(button(root, 'Open notebook'))
  await settle()
}

describe('first launch creates a notebook', () => {
  it('creates a notebook and opens an empty Notes view after the key is acknowledged', async () => {
    const { core, root } = await app()
    expect(text(root)).toMatch(/Welcome to Notebook/)
    click(button(root, 'Create a notebook'))
    await settle()
    expect(shownKey(root)).toBe(core.recoveryKey)

    await acknowledge(root)
    expect(inShell(root)).toBe(true)
    expect(root.querySelector('#note-list-heading')!.textContent).toBe('Notes')
    expect(text(root)).toMatch(/No notes here yet/)
  })

  it('needs nothing but local commands: no account, server, or passphrase', async () => {
    const { core, root } = await app()
    // Nothing to fill in: no server, username, passphrase, or invite.
    expect(root.querySelectorAll('input')).toHaveLength(0)
    click(button(root, 'Create a notebook'))
    await settle()
    await acknowledge(root)
    expect(inShell(root)).toBe(true)
    const setupCalls = core.calls
      .map((c) => c.method)
      .filter(
        (m) =>
          !['listNotes', 'tagTree', 'getSettings', 'backupStatus'].includes(m),
      )
    // `pendingRecoveryKey` shows up when the phase event beats the
    // `createNotebook` result, as it does with the fake.
    expect(setupCalls.filter((m) => m !== 'pendingRecoveryKey')).toEqual([
      'appState',
      'createNotebook',
      'acknowledgeRecoveryKey',
    ])
  })

  it('offers restoring from a backup beside creating a notebook', async () => {
    const { root } = await app()
    expect(hasButton(root, 'Create a notebook')).toBe(true)
    expect(hasButton(root, 'Restore from backup')).toBe(true)
    expect(root.querySelectorAll('button')).toHaveLength(2)
  })
})

describe('restore from backup on first launch', () => {
  const BACKUP = {
    file_name: 'notebook-backup-20261022-140309.nbbackup',
    same_vault: false,
    notes: [
      { id: 'a', body: '# Groceries' },
      { id: 'b', body: '# Trip plan' },
      { id: 'c', body: '# Old', archived: true },
    ],
  }

  async function picked() {
    const opened = await app()
    opened.core.backupFile = BACKUP
    click(button(opened.root, 'Restore from backup'))
    await settle()
    return opened
  }

  function enter(root: HTMLElement, key: string) {
    fill(field(root, 'Recovery key'), key)
    click(button(root, 'Restore'))
  }

  it('New computer: restores the backup with the recovery key and shows its notes', async () => {
    const { core, root } = await picked()
    expect(text(root)).toContain(BACKUP.file_name)
    enter(root, core.recoveryKey.toLowerCase())
    await settle()
    expect(core.callsTo('restoreBackup')).toEqual([
      [
        {
          recoveryKey: core.recoveryKey.toLowerCase(),
          skipSafetyBackup: false,
        },
      ],
    ])
    expect(inShell(root)).toBe(true)
    expect(titles(root).sort()).toEqual(['Groceries', 'Trip plan'])
    await selectView(root, 'Archive')
    expect(titles(root)).toEqual(['Old'])
  })

  it('Cancelled picker: the welcome screen stays as it was, with no error', async () => {
    const { core, root } = await app()
    const before = root.innerHTML
    click(button(root, 'Restore from backup'))
    await settle()
    expect(core.callsTo('pickBackup')).toHaveLength(1)
    expect(root.innerHTML).toBe(before)
    expect(alerts(root)).toEqual([])
    expect(core.callsTo('restoreBackup')).toEqual([])
  })

  it('says a mistyped key looks mistyped, and a wrong one doesn’t open the backup', async () => {
    const { core, root } = await picked()
    enter(root, 'ABCD-EFGH')
    await settle()
    expect(alerts(root)).toEqual([
      'That recovery key looks mistyped. Check each group and try again.',
    ])
    expect(field(root, 'Recovery key').getAttribute('aria-invalid')).toBe(
      'true',
    )
    enter(root, 'ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ')
    await settle()
    expect(alerts(root)).toEqual([
      'That recovery key doesn’t open this backup.',
    ])
    expect(inShell(root)).toBe(false)
    expect(core.phase.phase).toBe('setup')
    // The right key still works afterwards.
    enter(root, core.recoveryKey)
    await settle()
    expect(inShell(root)).toBe(true)
  })

  it('explains a file that isn’t a backup', async () => {
    const { core, root } = await app()
    core.failNext('pickBackup', 'backup_not_a_backup')
    click(button(root, 'Restore from backup'))
    await settle()
    expect(alerts(root)).toEqual(['That file isn’t a Notebook backup.'])
    expect(root.querySelector('input')).toBeNull()
  })

  it('goes back to the welcome screen on Cancel', async () => {
    const { core, root } = await picked()
    click(button(root, 'Cancel'))
    expect(root.querySelector('input')).toBeNull()
    expect(hasButton(root, 'Restore from backup')).toBe(true)
    expect(core.callsTo('restoreBackup')).toEqual([])
  })

  it('shows progress while restoring', async () => {
    const { core, root } = await picked()
    core.latency = 20
    enter(root, core.recoveryKey)
    await settle()
    expect(button(root, 'Restoring…').disabled).toBe(true)
    await vi.waitFor(() => expect(inShell(root)).toBe(true))
  })
})

describe('recovery key is shown once and acknowledged', () => {
  it('does not open the notebook until saving is confirmed', async () => {
    const { core, root } = await app()
    click(button(root, 'Create a notebook'))
    await settle()
    const open = button(root, 'Open notebook')
    expect(open.disabled).toBe(true)
    root.querySelector('form')!.requestSubmit()
    await settle()
    expect(core.callsTo('acknowledgeRecoveryKey')).toEqual([])
    expect(core.phase.phase).toBe('pending_recovery_key')
    expect(inShell(root)).toBe(false)

    check(field(root, ACK))
    expect(open.disabled).toBe(false)
  })

  it('shows a key it was handed without asking the core again', async () => {
    const core = new FakeCoreApi()
    core.phase = {
      phase: 'pending_recovery_key',
      secure_store: true,
      upgrade: false,
    }
    const phases: Phase[] = []
    const view = render(RecoveryKeyShown, {
      core,
      phase: core.phase,
      recoveryKey: 'HAND-ED00-0VER',
      onPhase: (p: Phase) => phases.push(p),
    })
    cleanups.push(view.destroy)
    await settle()
    expect(shownKey(view.target)).toBe('HAND-ED00-0VER')
    expect(core.callsTo('pendingRecoveryKey')).toEqual([])
    await acknowledge(view.target)
    expect(phases.map((p) => p.phase)).toEqual(['open'])
    expect(shownKey(view.target)).toBeNull()
  })

  it('explains what the key is for', async () => {
    const { root } = await app()
    click(button(root, 'Create a notebook'))
    await settle()
    expect(text(root)).toMatch(/open a backup on another computer/)
    expect(text(root)).toMatch(/secure credential store/)
    expect(text(root)).toMatch(/won’t be shown again/)
  })

  it('never shows the key again once acknowledged', async () => {
    const { core, root } = await app()
    click(button(root, 'Create a notebook'))
    await settle()
    await acknowledge(root)
    expect(root.textContent).not.toContain(core.recoveryKey)
    expect(shownKey(root)).toBeNull()
    // Settings has nothing that shows it either.
    click(button(root, 'Settings'))
    await settle()
    for (const tab of ['Editor', 'About']) {
      click(button(document.querySelector('[role="dialog"]')!, tab))
      expect(document.body.textContent).not.toContain(core.recoveryKey)
    }
    // And the core no longer hands it out.
    await expect(core.pendingRecoveryKey()).rejects.toMatchObject({
      code: 'not_open',
    })
  })

  it('says the key will be needed at each launch without a secure store', async () => {
    const { root } = await app({ secure_store: false })
    click(button(root, 'Create a notebook'))
    await settle()
    expect(text(root)).toMatch(/each time you open Notebook/)
  })
})

describe('recovery key entry without a secure store', () => {
  async function prompt(secure_store = false) {
    return app({ phase: 'needs_recovery_key', secure_store }, [
      { id: 'a', body: '# Kept note' },
    ])
  }

  function enter(root: HTMLElement, key: string) {
    fill(field(root, 'Recovery key'), key)
    click(button(root, 'Open notebook'))
  }

  it('explains why it asks, and opens the notes with the right key', async () => {
    const { core, root } = await prompt()
    expect(
      root.querySelector('[data-testid="no-secure-store"]')!.textContent,
    ).toMatch(/couldn’t use this computer’s secure credential store/)
    // A denied prompt reads the same, so the copy says how to retry.
    expect(text(root)).toMatch(/quit and reopen Notebook to try again/)
    // Typed loosely, as it may have been written down.
    enter(root, core.recoveryKey.toLowerCase().replaceAll('-', ' '))
    await settle()
    expect(inShell(root)).toBe(true)
    expect(titles(root)).toEqual(['Kept note'])
  })

  it('gives a different reason when there is a secure store', async () => {
    const { root } = await prompt(true)
    expect(root.querySelector('[data-testid="no-secure-store"]')).toBeNull()
    expect(text(root)).toMatch(/isn’t in this computer’s secure/)
  })

  it('says a wrong key doesn’t open this notebook, and stays closed', async () => {
    const { core, root } = await prompt()
    enter(root, 'ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ')
    await settle()
    expect(alerts(root)).toEqual([
      'That recovery key doesn’t open this notebook.',
    ])
    expect(core.phase.phase).toBe('needs_recovery_key')
    expect(inShell(root)).toBe(false)
  })

  it('says a mistyped key looks mistyped, distinct from a wrong one', async () => {
    const { root } = await prompt()
    enter(root, 'ABCD-EFGH')
    await settle()
    const mistyped = alerts(root)
    enter(root, 'ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ')
    await settle()
    const wrong = alerts(root)
    expect(mistyped).toEqual([
      'That recovery key looks mistyped. Check each group and try again.',
    ])
    expect(mistyped).not.toEqual(wrong)
  })

  it('shows the generic error when the notebook can’t be read', async () => {
    const { core, root } = await prompt()
    core.failNext('openWithRecoveryKey', 'internal', 'database is corrupt')
    enter(root, core.recoveryKey)
    await settle()
    expect(alerts(root)).toEqual(['Something went wrong. Try again.'])
    expect(inShell(root)).toBe(false)
  })

  it('starts over only after confirmation, keeping the old notebook', async () => {
    const { core, root } = await prompt()
    click(button(root, 'Lost the key, or the notebook won’t open?'))
    expect(text(root)).toMatch(/new, empty notebook/)
    expect(text(root)).toMatch(/under another name, not deleted/)
    click(button(root, 'Cancel'))
    expect(core.callsTo('startOver')).toEqual([])
    expect(text(root)).toMatch(/Open your notebook/)

    click(button(root, 'Lost the key, or the notebook won’t open?'))
    click(button(root, 'Start over'))
    await settle()
    expect(core.callsTo('startOver')).toHaveLength(1)
    expect(core.setAside).toBe(1)
    expect(text(root)).toMatch(/Welcome to Notebook/)
  })
})

describe('upgrade from a server-backed install', () => {
  const NOTES: SeedNote[] = [
    { id: 'a', body: '# From the server days' },
    { id: 'b', body: '# Another' },
  ]

  it('offers starting over when the old notebook won’t open', async () => {
    const { core, root } = await app(
      { phase: 'pending_recovery_key', upgrade: true },
      NOTES,
    )
    const offer = 'The old notebook won’t open?'
    expect(hasButton(root, offer)).toBe(false)
    core.failNext('acknowledgeRecoveryKey', 'internal', 'layout too new')
    await acknowledge(root)
    expect(alerts(root)).toEqual(['Something went wrong. Try again.'])
    click(button(root, offer))
    click(button(root, 'Start over'))
    await settle()
    expect(core.setAside).toBe(1)
    expect(text(root)).toMatch(/Welcome to Notebook/)
  })

  it('doesn’t offer starting over on a new notebook’s key screen', async () => {
    const { core, root } = await app()
    click(button(root, 'Create a notebook'))
    await settle()
    core.failNext('acknowledgeRecoveryKey', 'internal')
    await acknowledge(root)
    expect(hasButton(root, 'The old notebook won’t open?')).toBe(false)
  })

  it('shows a new recovery key, then every note, for a signed-in install', async () => {
    const { core, root } = await app(
      { phase: 'pending_recovery_key', upgrade: true },
      NOTES,
    )
    // The key was generated at launch, so the screen asks the core for it.
    expect(core.callsTo('pendingRecoveryKey')).toHaveLength(1)
    expect(shownKey(root)).toBe(core.recoveryKey)
    expect(text(root)).toMatch(/new recovery key/)
    expect(text(root)).toMatch(/one you saved before no longer matters/)

    await acknowledge(root)
    expect(inShell(root)).toBe(true)
    expect(titles(root).sort()).toEqual(['Another', 'From the server days'])
  })

  it('asks a locked install for the old passphrase once', async () => {
    const { core, root } = await app(
      { phase: 'needs_old_passphrase', upgrade: true },
      NOTES,
    )
    fill(field(root, 'Passphrase'), 'not it')
    click(button(root, 'Continue'))
    await settle()
    expect(alerts(root)).toEqual(['That passphrase is incorrect.'])
    expect(core.phase.phase).toBe('needs_old_passphrase')

    fill(field(root, 'Passphrase'), core.oldPassphrase)
    click(button(root, 'Continue'))
    await settle()
    expect(shownKey(root)).toBe(core.recoveryKey)
    expect(text(root)).toMatch(/new recovery key/)
    await acknowledge(root)
    expect(inShell(root)).toBe(true)
    expect(titles(root)).toHaveLength(2)
    expect(core.callsTo('upgradeWithPassphrase')).toHaveLength(2)
  })

  it('starts over, after confirmation, when the passphrase is forgotten', async () => {
    const { core, root } = await app(
      { phase: 'needs_old_passphrase', upgrade: true },
      NOTES,
    )
    click(button(root, 'I can’t remember it'))
    expect(text(root)).toMatch(/under another name, not deleted/)
    expect(core.callsTo('startOver')).toEqual([])
    click(button(root, 'Start over'))
    await settle()
    expect(core.setAside).toBe(1)
    click(button(root, 'Create a notebook'))
    await settle()
    await acknowledge(root)
    expect(text(root)).toMatch(/No notes here yet/)
  })
})
