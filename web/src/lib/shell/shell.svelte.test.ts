// @vitest-environment jsdom
import { EditorView } from '@codemirror/view'
import { getCM } from '@replit/codemirror-vim'
import { flushSync } from 'svelte'
import { afterEach, describe, expect, it, vi } from 'vitest'
import '../../editor/testing'
import type { SyncStatus } from '../../platform'
import { FakeCoreApi, type SeedNote } from '../../platform/fake'
import { PhaseState } from '../state'
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
import Shell from './Shell.svelte'
import SyncIndicator from './SyncIndicator.svelte'

const cleanups: (() => void)[] = []
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup()
})

async function shell(
  notes: SeedNote[] = [],
  setup?: (core: FakeCoreApi) => void,
) {
  const core = new FakeCoreApi(notes)
  core.phase = {
    phase: 'unlocked',
    account: { server: 'https://notes.example.com', username: 'alice' },
    credentials_remembered: true,
  }
  setup?.(core)
  const phase = new PhaseState(core)
  await phase.start()
  const view = render(Shell, { core, phase })
  let mounted = true
  // Unmounts the shell, as the app does when the vault locks.
  const destroy = () => {
    if (mounted) view.destroy()
    mounted = false
  }
  cleanups.push(destroy)
  await settle()
  return { core, phase, root: view.target, destroy }
}

/** Types into the open note, leaving the edit inside the editor's delay. */
function typeInEditor(root: HTMLElement, text: string) {
  const view = editorView(root)
  view.dispatch(view.state.replaceSelection(text), {
    userEvent: 'input.type',
  })
}

const list = (root: HTMLElement) => [
  ...root.querySelectorAll<HTMLElement>('[role="option"]'),
]

const titles = (root: HTMLElement) =>
  list(root).map((li) => li.querySelector('.entry-title')!.textContent!.trim())

function entry(root: HTMLElement, id: string): HTMLElement {
  const found = root.querySelector<HTMLElement>(
    `[role="option"][data-id="${id}"]`,
  )
  if (!found) throw new Error(`note ${id} not listed`)
  return found
}

function editorView(root: HTMLElement): EditorView {
  const host = root.querySelector('.note-editor-host')!.shadowRoot!
  return EditorView.findFromDOM(host.querySelector<HTMLElement>('.cm-editor')!)!
}

async function selectView(root: HTMLElement, label: string) {
  click(button(root.querySelector('nav')!, label))
  await settle()
}

function contextMenu(root: HTMLElement, id: string): HTMLElement {
  entry(root, id).dispatchEvent(
    new MouseEvent('contextmenu', { bubbles: true, clientX: 10, clientY: 10 }),
  )
  flushSync()
  return document.querySelector<HTMLElement>('[role="menu"]')!
}

function press(key: string, init: KeyboardEventInit = {}) {
  window.dispatchEvent(
    new KeyboardEvent('keydown', {
      key,
      bubbles: true,
      cancelable: true,
      ...init,
    }),
  )
}

describe('three-pane shell', () => {
  it('starts on Notes with the sidebar, list, and an empty editor', async () => {
    const { root } = await shell([
      { id: 'a', body: '# First\nmore text', updated: 1 },
      { id: 'b', body: '# Second', updated: 2 },
    ])
    const notes = button(root.querySelector('nav')!, 'Notes')
    expect(notes.getAttribute('aria-current')).toBe('true')
    expect(root.querySelector('#note-list-heading')!.textContent).toBe('Notes')
    expect(titles(root)).toEqual(['Second', 'First'])
    expect(root.querySelector('.editor-pane')!.textContent).toMatch(
      /No note selected/,
    )
  })

  it('shows Untitled, the preview, and the updated time', async () => {
    const { root } = await shell([
      { id: 'a', body: '', updated: Date.now() },
      {
        id: 'b',
        body: '# Groceries\neggs and milk',
        updated: Date.now() - 5 * 60_000,
      },
    ])
    expect(titles(root)).toEqual(['Untitled', 'Groceries'])
    expect(entry(root, 'b').querySelector('.preview')!.textContent).toBe(
      'eggs and milk',
    )
    expect(entry(root, 'b').querySelector('time')!.textContent!.trim()).toBe(
      '5 min ago',
    )
  })

  it('opens a selected note in the editor', async () => {
    const { root } = await shell([{ id: 'a', body: '# Hello\nworld' }])
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('# Hello\nworld'),
    )
    expect(editorView(root).state.readOnly).toBe(false)
    expect(entry(root, 'a').getAttribute('aria-selected')).toBe('true')
  })

  it('shows the empty state when the selection has no notes', async () => {
    const { root } = await shell([])
    expect(root.textContent).toMatch(/No notes here yet/)
  })

  it('opens trashed notes read-only with Restore', async () => {
    const { core, root } = await shell([
      { id: 't', body: 'binned', trashed: true },
    ])
    await selectView(root, 'Trash')
    click(entry(root, 't'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('binned'),
    )
    expect(editorView(root).state.readOnly).toBe(true)
    const pane = root.querySelector<HTMLElement>('.editor-pane')!
    expect(pane.textContent).toMatch(/in Trash and can’t be edited/)

    click(button(pane.querySelector('.trash-banner')!, 'Restore'))
    await settle()
    expect(core.callsTo('setTrashed')).toEqual([['t', false]])
    await vi.waitFor(() => expect(editorView(root).state.readOnly).toBe(false))
  })

  it('updates the list when the core reports a change', async () => {
    const { core, root } = await shell([{ id: 'a', body: '# Old title' }])
    core.remoteEdit('a', (body) => {
      body.delete(0, body.length)
      body.insert(0, '# New title')
    })
    await settle()
    expect(titles(root)).toEqual(['New title'])
  })

  it('turns the open note read-only when another device trashes it', async () => {
    const { core, root } = await shell([{ id: 'a', body: '# Shared' }])
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('# Shared'),
    )
    expect(editorView(root).state.readOnly).toBe(false)

    // Trashed elsewhere: it leaves Notes, and the core reports the change.
    await core.setTrashed('a', true)
    await settle()
    await settle()
    expect(titles(root)).toEqual([])
    await vi.waitFor(() => expect(editorView(root).state.readOnly).toBe(true))
    const pane = root.querySelector<HTMLElement>('.editor-pane')!
    expect(pane.textContent).toMatch(/in Trash and can’t be edited/)
    const actions = [...pane.querySelectorAll('[role="toolbar"] button')].map(
      (b) => b.textContent!.trim(),
    )
    expect(actions).toEqual(['Restore'])
  })

  it('closes the open note when another device deletes it', async () => {
    const { core, root } = await shell([
      { id: 'a', body: '# Doomed', trashed: true },
      { id: 'b', body: '# Keeper' },
    ])
    await selectView(root, 'Trash')
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('# Doomed'),
    )

    core.remotePurge('a')
    await settle()
    await settle()
    const pane = root.querySelector<HTMLElement>('.editor-pane')!
    expect(pane.textContent).toMatch(/No notes here yet/)
    expect(pane.querySelector('[role="toolbar"]')).toBeNull()
    expect(root.querySelector<HTMLElement>('.editor')!.hidden).toBe(true)
  })

  it('keeps the open note when it only leaves the list', async () => {
    const { core, root } = await shell([{ id: 'a', body: '# Moving' }])
    click(entry(root, 'a'))
    await settle()
    // Archived on another device: gone from Notes, but it still exists.
    await core.setArchived('a', true)
    await settle()
    await settle()
    expect(titles(root)).toEqual([])
    expect(root.querySelector('[role="toolbar"]')).not.toBeNull()
    expect(root.querySelector<HTMLElement>('.editor')!.hidden).toBe(false)
  })
})

describe('tag tree', () => {
  it('nests tags and remembers expansion', async () => {
    const { core, root } = await shell([
      { id: 'a', body: 'x #work/acme' },
      { id: 'b', body: 'y #home' },
    ])
    const nav = root.querySelector('nav')!
    const names = () =>
      [...nav.querySelectorAll('.tags .name')].map((n) => n.textContent)
    expect(names()).toEqual(['home', 'work', 'acme'])

    click(button(nav, 'Collapse work'))
    await settle()
    expect(names()).toEqual(['home', 'work'])
    expect(core.callsTo('setTagExpanded')).toEqual([['work', false]])
    expect(core.settings.collapsed_tags).toEqual(['work'])
  })

  it('starts with saved collapsed tags collapsed', async () => {
    const { root } = await shell([{ id: 'a', body: '#work/acme' }], (core) => {
      core.settings.collapsed_tags = ['work']
    })
    expect(hasButton(root, 'Expand work')).toBe(true)
    expect(root.querySelector('nav')!.textContent).not.toMatch(/acme/)
  })

  it('lists a tag’s notes, including descendants, when selected', async () => {
    const { root } = await shell([
      { id: 'a', body: '# A #work' },
      { id: 'b', body: '# B #work/acme' },
      { id: 'c', body: '# C #home' },
    ])
    click(button(root, 'work 2'))
    await settle()
    expect(root.querySelector('#note-list-heading')!.textContent).toBe('#work')
    expect(titles(root).sort()).toEqual(['A #work', 'B #work/acme'])
  })

  it('renames a tag inline with its full path', async () => {
    const { core, root } = await shell([{ id: 'a', body: '#work/acme notes' }])
    click(button(root, 'Rename tag work'))
    await settle()
    const input = field(root, 'New path for tag work')
    expect(input.value).toBe('work')
    fill(input, 'job')
    input.form!.requestSubmit()
    await settle()
    expect(core.callsTo('renameTag')).toEqual([['work', 'job']])
    expect(core.body('a')).toBe('#job/acme notes')
    const names = [...root.querySelectorAll('nav .tags .name')].map(
      (n) => n.textContent,
    )
    expect(names).toEqual(['job', 'acme'])
  })

  it('explains an invalid tag name and changes nothing', async () => {
    const { core, root } = await shell([{ id: 'a', body: '#work notes' }])
    click(button(root, 'Rename tag work'))
    await settle()
    const input = field(root, 'New path for tag work')
    fill(input, '42')
    input.form!.requestSubmit()
    await settle()
    expect(alerts(root)).toContain(
      'That isn’t a valid tag name. A tag can’t be only digits, contain # or a line break, or end in punctuation.',
    )
    expect(core.body('a')).toBe('#work notes')
  })
})

describe('note actions', () => {
  it('pins, archives, and trashes from the context menu', async () => {
    const { core, root } = await shell([{ id: 'a', body: 'A' }])
    click(button(contextMenu(root, 'a'), 'Pin'))
    await settle()
    expect(core.callsTo('setPinned')).toEqual([['a', true]])

    click(button(contextMenu(root, 'a'), 'Unpin'))
    await settle()
    expect(core.callsTo('setPinned')).toEqual([
      ['a', true],
      ['a', false],
    ])

    click(button(contextMenu(root, 'a'), 'Archive'))
    await settle()
    expect(core.callsTo('setArchived')).toEqual([['a', true]])
    expect(titles(root)).toEqual([])

    await selectView(root, 'Archive')
    click(button(contextMenu(root, 'a'), 'Unarchive'))
    await settle()
    expect(core.callsTo('setArchived')).toEqual([
      ['a', true],
      ['a', false],
    ])

    await selectView(root, 'Notes')
    click(button(contextMenu(root, 'a'), 'Move to Trash'))
    await settle()
    expect(core.callsTo('setTrashed')).toEqual([['a', true]])
  })

  it('offers the same actions in the editor toolbar', async () => {
    const { core, root } = await shell([{ id: 'a', body: 'A' }])
    click(entry(root, 'a'))
    await settle()
    const toolbar = root.querySelector<HTMLElement>('[role="toolbar"]')!
    click(button(toolbar, 'Pin'))
    await settle()
    expect(core.callsTo('setPinned')).toEqual([['a', true]])
    click(
      button(root.querySelector<HTMLElement>('[role="toolbar"]')!, 'Archive'),
    )
    await settle()
    expect(core.callsTo('setArchived')).toEqual([['a', true]])
    click(
      button(
        root.querySelector<HTMLElement>('[role="toolbar"]')!,
        'Move to Trash',
      ),
    )
    await settle()
    expect(core.callsTo('setTrashed')).toEqual([['a', true]])
    // The trashed note stays open, read-only, with only Restore.
    const actions = [...root.querySelectorAll('[role="toolbar"] button')].map(
      (b) => b.textContent!.trim(),
    )
    expect(actions).toEqual(['Restore'])
  })

  it('offers permanent deletion only in Trash, after confirmation', async () => {
    const { core, root } = await shell([
      { id: 'a', body: 'A' },
      { id: 't', body: 'T', trashed: true },
    ])
    const menu = contextMenu(root, 'a')
    expect(hasButton(menu, 'Delete permanently…')).toBe(false)
    menu.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
    )
    await settle()
    expect(document.querySelector('[role="menu"]')).toBeNull()

    await selectView(root, 'Trash')
    click(button(contextMenu(root, 't'), 'Delete permanently…'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    expect(dialog.textContent).toMatch(/Delete permanently\?/)
    expect(core.callsTo('deletePermanently')).toEqual([])

    click(button(dialog, 'Cancel'))
    await settle()
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(core.has('t')).toBe(true)
    expect(core.callsTo('deletePermanently')).toEqual([])

    click(button(contextMenu(root, 't'), 'Delete permanently…'))
    await settle()
    click(
      button(
        document.querySelector<HTMLElement>('[role="dialog"]')!,
        'Delete permanently',
      ),
    )
    await settle()
    expect(core.callsTo('deletePermanently')).toEqual([['t']])
    expect(titles(root)).toEqual([])
  })

  it('says a connection is required to delete while offline', async () => {
    const { core, root } = await shell([{ id: 't', body: 'T', trashed: true }])
    core.offline = true
    await selectView(root, 'Trash')
    click(button(contextMenu(root, 't'), 'Delete permanently…'))
    await settle()
    click(
      button(
        document.querySelector<HTMLElement>('[role="dialog"]')!,
        'Delete permanently',
      ),
    )
    await settle()
    expect(alerts(root).join(' ')).toMatch(/needs a connection to the server/)
    expect(core.has('t')).toBe(true)
  })

  it('creates a note under the selected tag and opens it', async () => {
    const { core, root } = await shell([{ id: 'a', body: '#work/acme' }])
    click(button(root, 'acme 1'))
    await settle()
    click(button(root, 'New note'))
    await settle()
    expect(core.callsTo('createNote')).toEqual([[{ tag: 'work/acme' }]])
    const selected = list(root).find(
      (li) => li.getAttribute('aria-selected') === 'true',
    )
    const id = selected!.dataset.id!
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('#work/acme\n'),
    )
    expect(core.body(id)).toBe('#work/acme\n')
  })

  it('handles Mod+N, Mod+Shift+F, and Mod+L', async () => {
    const { core, phase, root } = await shell([{ id: 'a', body: 'A' }])
    press('n', { ctrlKey: true })
    await settle()
    expect(core.callsTo('createNote')).toEqual([[{ view: 'notes' }]])

    press('F', { ctrlKey: true, shiftKey: true })
    expect(document.activeElement).toBe(field(root, 'Search notes'))

    press('l', { ctrlKey: true })
    await settle()
    expect(core.callsTo('lock')).toHaveLength(1)
    expect(phase.current?.phase).toBe('locked')
  })

  it('creates a note from Pinned in Notes, where it shows', async () => {
    const { core, root } = await shell([{ id: 'a', body: 'A', pinned: true }])
    await selectView(root, 'Pinned')
    click(button(root, 'New note'))
    await settle()
    expect(core.callsTo('createNote')).toEqual([[{ view: 'notes' }]])
    expect(root.querySelector('#note-list-heading')!.textContent).toBe('Notes')
    const selected = list(root).find(
      (li) => li.getAttribute('aria-selected') === 'true',
    )
    expect(selected).toBeDefined()
    expect(root.querySelector('[role="toolbar"]')).not.toBeNull()
  })
})

describe('pending edits are delivered first', () => {
  async function editing() {
    const opened = await shell([{ id: 'a', body: 'start' }])
    click(entry(opened.root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(opened.root).state.doc.toString()).toBe('start'),
    )
    editorView(opened.root).dispatch({ selection: { anchor: 5 } })
    // Well inside the editor's 300 ms delay.
    typeInEditor(opened.root, ' typed')
    expect(opened.core.callsTo('editorPush')).toEqual([])
    return opened
  }

  it('locks only after the last edit reaches the core, leaving nothing open', async () => {
    const { core, phase, root, destroy } = await editing()
    press('l', { ctrlKey: true })
    await settle()
    expect(phase.current?.phase).toBe('locked')
    expect(core.body('a')).toBe('start typed')
    const methods = core.calls.map((c) => c.method)
    expect(methods.lastIndexOf('editorPush')).toBeLessThan(
      methods.indexOf('lock'),
    )

    // The app unmounts the shell on lock.
    void root
    destroy()
    await settle()
    expect(core.listenerCount('editor-remote-update')).toBe(0)
    const pushes = core.callsTo('editorPush').length
    await new Promise((resolve) => setTimeout(resolve, 1200))
    expect(core.callsTo('editorPush')).toHaveLength(pushes)
  })

  it('locks from Settings after delivering edits', async () => {
    const { core, phase, root } = await editing()
    click(button(root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, `Lock ${'Ctrl+L'}`))
    await settle()
    expect(phase.current?.phase).toBe('locked')
    expect(core.body('a')).toBe('start typed')
  })

  it('counts editor edits before warning about sign-out', async () => {
    const { core, root } = await editing()
    click(button(root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Sign out'))
    await settle()
    const methods = core.calls.map((c) => c.method)
    expect(methods.indexOf('editorPush')).toBeGreaterThanOrEqual(0)
    expect(methods.indexOf('editorPush')).toBeLessThan(
      methods.indexOf('pendingCount'),
    )
  })

  it('trashes the open note after delivering its edits', async () => {
    const { core, root } = await editing()
    click(
      button(
        root.querySelector<HTMLElement>('[role="toolbar"]')!,
        'Move to Trash',
      ),
    )
    await settle()
    expect(core.body('a')).toBe('start typed')
    expect(core.callsTo('setTrashed')).toEqual([['a', true]])
  })
})

describe('SyncIndicator', () => {
  function indicator(status: SyncStatus | null) {
    const view = render(SyncIndicator, { status })
    cleanups.push(view.destroy)
    return view.target.textContent!.replace(/\s+/g, ' ').trim()
  }

  it('shows each sync state and the pending count when there is one', () => {
    expect(indicator({ state: 'synced', pending: 0, attention: [] })).toBe(
      'Synced',
    )
    expect(indicator({ state: 'syncing', pending: 1, attention: [] })).toBe(
      'Syncing… · 1 pending',
    )
    expect(indicator({ state: 'offline', pending: 2, attention: [] })).toBe(
      'Offline · 2 pending',
    )
  })

  it('says when sign-in, an upgrade, or storage needs the user', () => {
    expect(
      indicator({
        state: 'offline',
        pending: 1,
        attention: ['sign_in_required'],
      }),
    ).toMatch(/Sign in required to sync/)
    expect(
      indicator({
        state: 'synced',
        pending: 0,
        attention: ['upgrade_required'],
      }),
    ).toMatch(/App upgrade required/)
    expect(
      indicator({ state: 'syncing', pending: 3, attention: ['quota_full'] }),
    ).toMatch(/3 pending.*Storage full/)
  })

  it('follows sync-status events in the shell', async () => {
    const { core, root } = await shell()
    core.setSync({ state: 'offline', pending: 2, attention: [] })
    await settle()
    expect(root.querySelector('.sync')!.textContent).toMatch(
      /Offline\s*· 2 pending/,
    )
  })
})

describe('Settings', () => {
  async function openSettings(notes: SeedNote[] = []) {
    const opened = await shell(notes)
    click(button(opened.root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    return { ...opened, dialog }
  }

  it('shows the account and its actions', async () => {
    const { dialog } = await openSettings()
    expect(dialog.textContent).toMatch(/https:\/\/notes\.example\.com/)
    expect(dialog.textContent).toMatch(/alice/)
    expect(dialog.textContent).toMatch(/Signed in · Synced/)
    for (const name of ['Change passphrase…', 'Sign out']) {
      expect(hasButton(dialog, name)).toBe(true)
    }
    expect(button(dialog, 'Lock Ctrl+L')).toBeTruthy()
    expect(hasButton(dialog, 'Editor')).toBe(true)
  })

  it('applies vim mode to the open editor at once', async () => {
    const { core, root } = await shell([{ id: 'a', body: 'A' }])
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('A'),
    )
    expect(getCM(editorView(root))).toBeNull()

    click(button(root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Editor'))
    check(field(dialog, 'Vim mode'))
    await settle()
    expect(core.callsTo('setVimMode')).toEqual([[true]])
    expect(getCM(editorView(root))).not.toBeNull()
  })

  it('warns how many changes sign-out loses and needs confirmation', async () => {
    const { core, dialog, phase } = await openSettings()
    core.syncState = { state: 'offline', pending: 3, attention: [] }
    click(button(dialog, 'Sign out'))
    await settle()
    expect(dialog.textContent).toMatch(
      /3 changes haven’t synced and will be lost/,
    )
    expect(core.callsTo('signOut')).toEqual([])

    click(button(dialog, 'Sign out and lose changes'))
    await settle()
    expect(core.callsTo('signOut')).toEqual([[true]])
    expect(phase.current?.phase).toBe('signed_out')
  })

  it('signs out at once when everything is synced', async () => {
    const { core, dialog, phase } = await openSettings()
    click(button(dialog, 'Sign out'))
    await settle()
    expect(core.callsTo('signOut')).toEqual([[false]])
    expect(phase.current?.phase).toBe('signed_out')
  })

  it('locks', async () => {
    const { core, dialog, phase } = await openSettings()
    click(button(dialog, 'Lock Ctrl+L'))
    await settle()
    expect(core.callsTo('lock')).toHaveLength(1)
    expect(phase.current?.phase).toBe('locked')
  })

  it('changes the passphrase, and says when that needs a connection', async () => {
    const { core, dialog } = await openSettings()
    click(button(dialog, 'Change passphrase…'))
    fill(field(dialog, 'Current passphrase'), 'correct horse battery')
    fill(field(dialog, 'New passphrase'), 'an even better one')
    fill(field(dialog, 'New passphrase again'), 'an even better one')
    core.offline = true
    click(button(dialog, 'Change passphrase'))
    await settle()
    expect(alerts(dialog)[0]).toMatch(/needs a connection/)

    core.offline = false
    click(button(dialog, 'Change passphrase'))
    await settle()
    expect(core.account.passphrase).toBe('an even better one')
    expect(dialog.textContent).toMatch(/Passphrase changed/)
  })

  it('offers sign-in when the session has ended', async () => {
    const { core, dialog, root } = await openSettings()
    click(button(dialog, 'Change passphrase…'))
    fill(field(dialog, 'Current passphrase'), 'correct horse battery')
    fill(field(dialog, 'New passphrase'), 'an even better one')
    fill(field(dialog, 'New passphrase again'), 'an even better one')
    core.failNext('changePassphrase', 'sign_in_required')
    click(button(dialog, 'Change passphrase'))
    await settle()
    expect(alerts(dialog)[0]).toMatch(/session has ended/)
    expect(root.textContent).toMatch(/Sign in to keep syncing/)
  })
})

describe('Export', () => {
  const seeded: SeedNote[] = [
    { id: 'a', body: 'Active' },
    { id: 'b', body: 'Archived', archived: true },
    { id: 'c', body: 'Trashed', trashed: true },
  ]

  async function openExport(notes: SeedNote[] = seeded) {
    const opened = await shell(notes)
    click(button(opened.root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Export'))
    return { ...opened, dialog }
  }

  const status = (dialog: HTMLElement) =>
    dialog.querySelector('[role="status"]')?.textContent?.replace(/\s+/g, ' ')

  it('warns that the export is unencrypted before it starts', async () => {
    const { core, dialog } = await openExport()
    expect(dialog.textContent).toMatch(/plain, unencrypted text/)
    expect(field(dialog, 'Include notes in Trash').checked).toBe(false)
    expect(core.callsTo('exportNotes')).toEqual([])
  })

  it('exports, then reports the count and where the notes went', async () => {
    const { core, dialog } = await openExport()
    click(button(dialog, 'Export all notes…'))
    await settle()
    expect(core.callsTo('exportNotes')).toEqual([[{ includeTrashed: false }]])
    expect(status(dialog)).toMatch(
      'Exported 2 notes to /Users/alice/Backups/notebook-export-20261021-140305',
    )
    expect(alerts(dialog)).toEqual([])
  })

  it('includes trashed notes when asked', async () => {
    const { core, dialog } = await openExport()
    check(field(dialog, 'Include notes in Trash'))
    click(button(dialog, 'Export all notes…'))
    await settle()
    expect(core.callsTo('exportNotes')).toEqual([[{ includeTrashed: true }]])
    expect(status(dialog)).toMatch(/Exported 3 notes/)
  })

  it('says “1 note” for one', async () => {
    const { dialog } = await openExport([{ body: 'Only' }])
    click(button(dialog, 'Export all notes…'))
    await settle()
    expect(status(dialog)).toMatch(/Exported 1 note to/)
  })

  it('shows nothing when the folder picker is cancelled', async () => {
    const { core, dialog } = await openExport()
    core.exportCancelled = true
    click(button(dialog, 'Export all notes…'))
    await settle()
    expect(core.callsTo('exportNotes')).toHaveLength(1)
    expect(status(dialog)).toBeUndefined()
    expect(alerts(dialog)).toEqual([])
  })

  it('explains a failure and does not report success', async () => {
    const { core, dialog } = await openExport()
    core.failNext(
      'exportNotes',
      'export_failed',
      'permission denied (os error 13)',
    )
    click(button(dialog, 'Export all notes…'))
    await settle()
    expect(alerts(dialog)).toEqual([
      'The export failed. Nothing was written. Permission denied (os error 13).',
    ])
    expect(status(dialog)).toBeUndefined()
  })

  it('disables the button while exporting', async () => {
    const { core, dialog } = await openExport()
    core.latency = 20
    click(button(dialog, 'Export all notes…'))
    await settle()
    expect(button(dialog, 'Exporting…').disabled).toBe(true)
    click(button(dialog, 'Exporting…'))
    await vi.waitFor(() => expect(status(dialog)).toMatch(/Exported 2 notes/))
    expect(core.callsTo('exportNotes')).toHaveLength(1)
    expect(button(dialog, 'Export all notes…').disabled).toBe(false)
  })

  it('disables the button while locked', async () => {
    const { dialog, phase } = await openExport()
    phase.set({
      phase: 'locked',
      account: { server: 'https://notes.example.com', username: 'alice' },
      credentials_remembered: true,
    })
    flushSync()
    expect(button(dialog, 'Export all notes…').disabled).toBe(true)
  })

  it('delivers the open editor’s edits before exporting', async () => {
    const { core, root } = await shell([{ id: 'a', body: 'A' }])
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('A'),
    )
    typeInEditor(root, 'B')
    click(button(root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Export'))
    click(button(dialog, 'Export all notes…'))
    await settle()
    const methods = core.calls.map((c) => c.method)
    expect(methods.lastIndexOf('editorPush')).toBeLessThan(
      methods.indexOf('exportNotes'),
    )
    expect(core.body('a')).toBe('BA')
  })
})
