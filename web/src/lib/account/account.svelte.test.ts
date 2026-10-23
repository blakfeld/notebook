// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest'
import type { Phase } from '../../platform'
import { FakeCoreApi } from '../../platform/fake'
import { SyncStatusState } from '../state'
import {
  alerts,
  button,
  check,
  click,
  field,
  fill,
  render,
  settle,
} from '../testing'
import AccountScreens from './AccountScreens.svelte'
import ReauthPrompt from './ReauthPrompt.svelte'
import SecureStoreNotice from './SecureStoreNotice.svelte'
import Unlock from './Unlock.svelte'

const cleanups: (() => void)[] = []
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup()
})

function screens(core = new FakeCoreApi()) {
  const phases: Phase[] = []
  const view = render(AccountScreens, {
    core,
    onPhase: (phase: Phase) => phases.push(phase),
  })
  cleanups.push(view.destroy)
  return { core, phases, root: view.target }
}

function signIn(root: HTMLElement, username: string, passphrase: string) {
  fill(field(root, 'Server'), 'https://notes.example.com')
  fill(field(root, 'Username'), username)
  fill(field(root, 'Passphrase'), passphrase)
  click(button(root, 'Sign in'))
}

async function openRegister(root: HTMLElement) {
  click(button(root, 'Create an account'))
  await settle()
  fill(field(root, 'Server'), 'https://notes.example.com')
  fill(field(root, 'Username'), 'bob')
  fill(field(root, 'Passphrase'), 'twelve chars long')
  fill(field(root, 'Passphrase again'), 'twelve chars long')
  fill(field(root, 'Invite code'), 'INVITE')
}

describe('sign in', () => {
  it('signs in and reports the new phase', async () => {
    const { core, phases, root } = screens()
    signIn(root, 'alice', 'correct horse battery')
    await settle()
    expect(core.callsTo('signIn')).toEqual([
      ['https://notes.example.com', 'alice', 'correct horse battery'],
    ])
    expect(phases.map((p) => p.phase)).toEqual(['unlocked'])
  })

  it('shows one message for an unknown user and a wrong passphrase', async () => {
    const { root } = screens()
    signIn(root, 'nobody', 'correct horse battery')
    await settle()
    const unknownUser = alerts(root)
    signIn(root, 'alice', 'wrong passphrase!')
    await settle()
    const wrongPassphrase = alerts(root)
    expect(unknownUser).toEqual(['The username or passphrase is incorrect.'])
    expect(wrongPassphrase).toEqual(unknownUser)
  })

  it('says the server is unreachable or rate limiting, not bad credentials', async () => {
    const { core, root } = screens()
    core.offline = true
    signIn(root, 'alice', 'correct horse battery')
    await settle()
    const unreachable = alerts(root)[0]
    expect(unreachable).toMatch(/could not be reached/)

    core.offline = false
    core.failNext('signIn', 'rate_limited')
    signIn(root, 'alice', 'correct horse battery')
    await settle()
    const limited = alerts(root)[0]
    expect(limited).toMatch(/Too many attempts/)
    const bad = 'The username or passphrase is incorrect.'
    expect(new Set([unreachable, limited, bad]).size).toBe(3)
  })

  it('names the problem with a server address', async () => {
    const { core, root } = screens()
    core.failNext(
      'signIn',
      'invalid_server_url',
      'HTTPS is required for this server',
    )
    signIn(root, 'alice', 'correct horse battery')
    await settle()
    expect(alerts(root)).toEqual(['HTTPS is required for this server.'])
  })
})

describe('register', () => {
  it('shows the recovery key and gates submit on the acknowledgment', async () => {
    const { core, root } = screens()
    await openRegister(root)
    expect(core.callsTo('generateRecoveryKey')).toHaveLength(1)
    expect(
      root.querySelector('[data-testid="recovery-key"]')?.textContent,
    ).toBe(core.recoveryKey)

    const submit = button(root, 'Create account')
    expect(submit.disabled).toBe(true)
    root.querySelector('form')!.requestSubmit()
    await settle()
    expect(core.callsTo('register')).toHaveLength(0)

    check(field(root, 'I’ve saved my recovery key somewhere safe'))
    expect(submit.disabled).toBe(false)
    click(submit)
    await settle()
    expect(core.callsTo('register')).toEqual([
      [
        {
          server: 'https://notes.example.com',
          username: 'bob',
          passphrase: 'twelve chars long',
          confirm: 'twelve chars long',
          inviteCode: 'INVITE',
        },
      ],
    ])
  })

  it('refuses mismatched passphrases without a request', async () => {
    const { core, root } = screens()
    await openRegister(root)
    fill(field(root, 'Passphrase again'), 'twelve chars lonG')
    check(field(root, 'I’ve saved my recovery key somewhere safe'))
    click(button(root, 'Create account'))
    await settle()
    expect(alerts(root)).toEqual(['The passphrases don’t match.'])
    expect(core.callsTo('register')).toHaveLength(0)
  })

  it('refuses a short passphrase without a request', async () => {
    const { core, root } = screens()
    await openRegister(root)
    fill(field(root, 'Passphrase'), 'short')
    fill(field(root, 'Passphrase again'), 'short')
    check(field(root, 'I’ve saved my recovery key somewhere safe'))
    click(button(root, 'Create account'))
    await settle()
    expect(alerts(root)[0]).toMatch(/at least 12 characters/)
    expect(core.callsTo('register')).toHaveLength(0)
  })

  it('says when the invite code is refused', async () => {
    const { core, root } = screens()
    await openRegister(root)
    check(field(root, 'I’ve saved my recovery key somewhere safe'))
    core.failNext('register', 'invalid_invite')
    click(button(root, 'Create account'))
    await settle()
    expect(alerts(root)).toEqual([
      'That invite code wasn’t accepted. It may be mistyped, already used, or expired.',
    ])
  })

  it('asks for another username and keeps the other entries', async () => {
    const { core, root } = screens()
    await openRegister(root)
    check(field(root, 'I’ve saved my recovery key somewhere safe'))
    core.failNext('register', 'username_taken')
    click(button(root, 'Create account'))
    await settle()
    expect(alerts(root)).toEqual([
      'That username is taken. Choose a different one.',
    ])
    expect(field(root, 'Invite code').value).toBe('INVITE')
    expect(field(root, 'Passphrase').value).toBe('twelve chars long')
    // The same key the user saved stays; it isn't regenerated.
    expect(core.callsTo('generateRecoveryKey')).toHaveLength(1)
    expect(root.textContent).toContain(core.recoveryKey)
  })

  it('never shows the recovery key once registered', async () => {
    const core = new FakeCoreApi()
    const { phases, root } = screens(core)
    await openRegister(root)
    check(field(root, 'I’ve saved my recovery key somewhere safe'))
    click(button(root, 'Create account'))
    await settle()
    expect(phases.map((p) => p.phase)).toEqual(['unlocked'])
    expect(root.textContent).not.toContain(core.recoveryKey)
  })
})

describe('recover', () => {
  function fillRecover(root: HTMLElement, key: string) {
    fill(field(root, 'Server'), 'https://notes.example.com')
    fill(field(root, 'Username'), 'alice')
    fill(field(root, 'Recovery key'), key)
    fill(field(root, 'New passphrase'), 'a new passphrase')
    fill(field(root, 'New passphrase again'), 'a new passphrase')
    click(button(root, 'Recover account'))
  }

  it('tells a mistyped key from a wrong one', async () => {
    const { root } = screens()
    click(button(root, 'Forgot passphrase?'))
    await settle()
    fillRecover(root, 'not a key')
    await settle()
    const typo = alerts(root)
    fillRecover(root, 'ZZZZ-ZZZZ-ZZZZ-ZZZZ')
    await settle()
    const wrong = alerts(root)
    expect(typo[0]).toMatch(/looks mistyped/)
    expect(wrong[0]).toMatch(/doesn’t unlock this account/)
  })

  it('recovers with the right key', async () => {
    const { core, phases, root } = screens()
    click(button(root, 'Forgot passphrase?'))
    await settle()
    fillRecover(root, core.recoveryKey)
    await settle()
    expect(phases.map((p) => p.phase)).toEqual(['unlocked'])
  })
})

describe('unlock', () => {
  it('stays locked with a wrong passphrase and unlocks with the right one', async () => {
    const core = new FakeCoreApi()
    core.phase = {
      phase: 'locked',
      account: { server: 'https://s', username: 'alice' },
      credentials_remembered: true,
    }
    const phases: Phase[] = []
    const view = render(Unlock, {
      core,
      account: core.phase.account,
      onPhase: (p: Phase) => phases.push(p),
    })
    cleanups.push(view.destroy)
    const root = view.target
    fill(field(root, 'Passphrase'), 'nope')
    click(button(root, 'Unlock'))
    await settle()
    expect(alerts(root)).toEqual(['That passphrase is incorrect.'])
    expect(phases).toEqual([])

    fill(field(root, 'Passphrase'), 'correct horse battery')
    click(button(root, 'Unlock'))
    await settle()
    expect(phases.map((p) => p.phase)).toEqual(['unlocked'])
    // The settings commands are vault-only; the unlock screen never calls them.
    expect(core.callsTo('getSettings')).toEqual([])
  })
})

describe('in-shell prompts', () => {
  it('shows the no-secure-store notice only when credentials aren’t remembered', () => {
    const remembered = render(SecureStoreNotice, { remembered: true })
    cleanups.push(remembered.destroy)
    expect(remembered.target.textContent).toBe('')

    const forgotten = render(SecureStoreNotice, { remembered: false })
    cleanups.push(forgotten.destroy)
    expect(forgotten.target.textContent).toMatch(/sign in each time/)
    click(button(forgotten.target, 'Dismiss'))
    expect(forgotten.target.textContent).toBe('')
  })

  it('asks for the passphrase when sign-in is required, without leaving', async () => {
    const core = new FakeCoreApi()
    core.phase = {
      phase: 'unlocked',
      account: null,
      credentials_remembered: true,
    }
    const sync = new SyncStatusState(core)
    await sync.start()
    const view = render(ReauthPrompt, { core, sync })
    cleanups.push(view.destroy)
    const root = view.target
    expect(root.textContent).toBe('')

    core.setSync({
      state: 'offline',
      pending: 1,
      attention: ['sign_in_required'],
    })
    await settle()
    expect(root.textContent).toMatch(/Sign in to keep syncing/)

    fill(field(root, 'Passphrase'), 'wrong')
    click(button(root, 'Sign in'))
    await settle()
    expect(alerts(root)).toEqual(['That passphrase is incorrect.'])

    fill(field(root, 'Passphrase'), 'correct horse battery')
    click(button(root, 'Sign in'))
    await settle()
    expect(core.callsTo('reauthenticate')).toEqual([
      ['wrong'],
      ['correct horse battery'],
    ])
    expect(root.textContent).toBe('')
  })

  it('appears when a command finds the session ended', async () => {
    const core = new FakeCoreApi()
    const sync = new SyncStatusState(core)
    const view = render(ReauthPrompt, { core, sync })
    cleanups.push(view.destroy)
    sync.checkError({ code: 'bad_credentials', message: '' })
    await settle()
    expect(view.target.textContent).toBe('')
    sync.checkError({ code: 'sign_in_required', message: '' })
    await settle()
    expect(view.target.textContent).toMatch(/Sign in to keep syncing/)
  })
})
