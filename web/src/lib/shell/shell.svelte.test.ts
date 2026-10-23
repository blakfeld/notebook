// @vitest-environment jsdom
import { EditorView } from '@codemirror/view'
import { getCM } from '@replit/codemirror-vim'
import { flushSync } from 'svelte'
import { afterEach, describe, expect, it, vi } from 'vitest'
import '../../editor/testing'
import {
  FakeCoreApi,
  type FakeBackupFile,
  type SeedNote,
} from '../../platform/fake'
import { coreSettings } from '../../platform/settings'
import { dayTime } from '../format'
import { createPreferences } from '../preferences'
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

const cleanups: (() => void)[] = []
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup()
})

async function shell(
  notes: SeedNote[] = [],
  setup?: (core: FakeCoreApi) => void,
) {
  const core = new FakeCoreApi(notes)
  core.phase = { phase: 'open', secure_store: true, upgrade: false }
  setup?.(core)
  const phase = new PhaseState(core)
  await phase.start()
  const preferences = createPreferences(coreSettings(core))
  await preferences.load()
  const view = render(Shell, { core, phase, preferences })
  let mounted = true
  // Unmounts the shell, as the app does when it closes.
  const destroy = () => {
    if (mounted) view.destroy()
    mounted = false
  }
  cleanups.push(destroy)
  await settle()
  return { core, phase, preferences, root: view.target, destroy }
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
    const actions = [
      ...pane.querySelectorAll('[aria-label="Note actions"] button'),
    ].map((b) => b.textContent!.trim())
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
    const toolbar = root.querySelector<HTMLElement>(
      '[aria-label="Note actions"]',
    )!
    click(button(toolbar, 'Pin'))
    await settle()
    expect(core.callsTo('setPinned')).toEqual([['a', true]])
    click(
      button(
        root.querySelector<HTMLElement>('[aria-label="Note actions"]')!,
        'Archive',
      ),
    )
    await settle()
    expect(core.callsTo('setArchived')).toEqual([['a', true]])
    click(
      button(
        root.querySelector<HTMLElement>('[aria-label="Note actions"]')!,
        'Move to Trash',
      ),
    )
    await settle()
    expect(core.callsTo('setTrashed')).toEqual([['a', true]])
    // The trashed note stays open, read-only, with only Restore.
    const actions = [
      ...root.querySelectorAll('[aria-label="Note actions"] button'),
    ].map((b) => b.textContent!.trim())
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

  it('deletes at once, with no connection involved', async () => {
    const { core, root } = await shell([{ id: 't', body: 'T', trashed: true }])
    await selectView(root, 'Trash')
    click(button(contextMenu(root, 't'), 'Delete permanently…'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    expect(dialog.textContent).toMatch(/deleted from this device/)
    expect(dialog.textContent).not.toMatch(/all your devices/)
    click(button(dialog, 'Delete permanently'))
    await settle()
    expect(alerts(root)).toEqual([])
    expect(core.has('t')).toBe(false)
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

  it('handles Mod+N and Mod+Shift+F', async () => {
    const { core, root } = await shell([{ id: 'a', body: 'A' }])
    press('n', { ctrlKey: true })
    await settle()
    expect(core.callsTo('createNote')).toEqual([[{ view: 'notes' }]])

    press('F', { ctrlKey: true, shiftKey: true })
    expect(document.activeElement).toBe(field(root, 'Search notes'))
  })

  it('keeps Mod+Shift+F for search inside the editor; view keys don’t clash', async () => {
    const { preferences, root } = await shell([{ id: 'a', body: 'A' }])
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('A'),
    )
    const content = editorView(root).contentDOM
    const chord = (key: string) =>
      content.dispatchEvent(
        new KeyboardEvent('keydown', {
          key,
          keyCode: key.charCodeAt(0),
          // Real key events leave the shadow root and reach the window.
          composed: true,
          ctrlKey: true,
          shiftKey: true,
          bubbles: true,
          cancelable: true,
        }),
      )
    chord('F')
    expect(document.activeElement).toBe(field(root, 'Search notes'))
    expect(preferences.current.focusMode).toBe(false)

    field(root, 'Search notes').blur()
    chord('D')
    chord('E')
    await settle()
    expect(preferences.current).toMatchObject({
      focusMode: true,
      sourceView: true,
    })
    expect(document.activeElement).not.toBe(field(root, 'Search notes'))
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

  it('leaves nothing attached once the shell unmounts', async () => {
    const { core, destroy } = await editing()
    destroy()
    await settle()
    expect(core.listenerCount('editor-remote-update')).toBe(0)
    const pushes = core.callsTo('editorPush').length
    await new Promise((resolve) => setTimeout(resolve, 1200))
    expect(core.callsTo('editorPush')).toHaveLength(pushes)
  })

  it('trashes the open note after delivering its edits', async () => {
    const { core, root } = await editing()
    click(
      button(
        root.querySelector<HTMLElement>('[aria-label="Note actions"]')!,
        'Move to Trash',
      ),
    )
    await settle()
    expect(core.body('a')).toBe('start typed')
    expect(core.callsTo('setTrashed')).toEqual([['a', true]])
  })
})

describe('sidebar footer', () => {
  it('keeps the status slot beside Settings, empty while backups are fine', async () => {
    const { root } = await shell()
    const footer = root.querySelector<HTMLElement>('.sidebar footer')!
    expect(footer.querySelector('.status')!.textContent).toBe('')
    expect(hasButton(footer, 'Settings')).toBe(true)
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

  it('shows the vim toggle and the version, and nothing about accounts', async () => {
    const { core, dialog } = await openSettings()
    expect(field(dialog, 'Vim mode').type).toBe('checkbox')
    const seen = [dialog.textContent!]

    click(button(dialog, 'About'))
    await settle()
    expect(
      dialog.querySelector('[data-testid="app-version"]')!.textContent!.trim(),
    ).toBe(`v${core.version}`)
    seen.push(dialog.textContent!)
    click(button(dialog, 'Appearance'))
    await settle()
    seen.push(dialog.textContent!)

    const tabs = [...dialog.querySelectorAll('[role="tab"]')].map((t) =>
      t.textContent!.trim(),
    )
    expect(tabs).toEqual(['Appearance', 'Editor', 'Backup', 'Export', 'About'])
    for (const text of seen) {
      expect(text).not.toMatch(
        /server|account|passphrase|lock|sign[- ]?out|sign[- ]?in|sync/i,
      )
    }
  })

  it('says when the version can’t be read', async () => {
    const { core, root } = await shell()
    core.failNext('coreVersion', 'internal')
    click(button(root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'About'))
    await settle()
    expect(
      dialog.querySelector('[data-testid="app-version"]')!.textContent!.trim(),
    ).toBe('Unknown')
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

  it('sets the theme from Appearance and saves it on the device', async () => {
    const { core, dialog, preferences } = await openSettings()
    click(button(dialog, 'Appearance'))
    expect(field(dialog, 'System').checked).toBe(true)
    check(field(dialog, 'Dark'))
    await settle()
    expect(preferences.current.theme).toBe('dark')
    expect(core.callsTo('setPreference')).toEqual([
      ['appearance.theme', 'dark'],
    ])
    expect(field(dialog, 'Dark').checked).toBe(true)
    expect(field(dialog, 'System').checked).toBe(false)
  })

  it('shows a theme that couldn’t be saved, and still applies it', async () => {
    const { core, dialog, preferences } = await openSettings()
    core.failNext('setPreference', 'internal', 'disk full')
    click(button(dialog, 'Appearance'))
    check(field(dialog, 'Dark'))
    await settle()
    expect(preferences.current.theme).toBe('dark')
    const alert = document.querySelector('.shell [role="alert"]')!
    expect(alert.textContent).toMatch(/Couldn’t save that setting/)
    click(button(alert, 'Dismiss'))
    await settle()
    expect(document.querySelector('.shell [role="alert"]')).toBeNull()
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

describe('Backup settings', () => {
  const FOLDER = '/Users/alice/Dropbox/Notebook'
  const HOUR = 3_600_000

  async function openBackup(
    notes: SeedNote[] = [{ id: 'a', body: '# Current' }],
    setup?: (core: FakeCoreApi) => void,
  ) {
    const opened = await shell(notes, setup)
    click(button(opened.root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Backup'))
    return { ...opened, dialog }
  }

  /** A folder is set and the last backup was `ago` milliseconds back. */
  const backedUp =
    (ago: number, changes: Partial<FakeCoreApi['backup']> = {}) =>
    (core: FakeCoreApi) =>
      Object.assign(core.backup, {
        folder: FOLDER,
        last: Date.now() - ago,
        changed: true,
        ...changes,
      })

  const statusText = (dialog: HTMLElement) =>
    dialog
      .querySelector('[data-testid="backup-status"]')
      ?.textContent?.replace(/\s+/g, ' ')
      .trim()

  it('No folder yet: offers to choose one, and Back up now is unavailable', async () => {
    const { core, dialog } = await openBackup()
    expect(hasButton(dialog, 'Choose folder…')).toBe(true)
    expect(button(dialog, 'Back up now').disabled).toBe(true)
    expect(dialog.querySelector('[data-testid="backup-status"]')).toBeNull()
    expect(field(dialog, 'Back up automatically').disabled).toBe(true)
    expect(dialog.textContent!.replace(/\s+/g, ' ')).toMatch(
      /Backups are encrypted\. To open one on another computer, you need your recovery key\./,
    )

    click(button(dialog, 'Choose folder…'))
    await settle()
    // The first folder gets a backup at once.
    expect(core.backupsWritten).toHaveLength(1)
    expect(
      dialog.querySelector('[data-testid="backup-folder"]')!.textContent,
    ).toBe(FOLDER)
    expect(hasButton(dialog, 'Change folder…')).toBe(true)
    expect(button(dialog, 'Back up now').disabled).toBe(false)
    expect(statusText(dialog)).toMatch(/Last backup Today at/)
    expect(statusText(dialog)).toMatch(/Next backup After your next change/)
  })

  it('changes nothing when the folder picker is dismissed', async () => {
    const { core, dialog } = await openBackup()
    core.backupFolderChoice = null
    click(button(dialog, 'Choose folder…'))
    await settle()
    expect(core.callsTo('chooseBackupFolder')).toHaveLength(1)
    expect(hasButton(dialog, 'Choose folder…')).toBe(true)
    expect(alerts(dialog)).toEqual([])
    expect(core.backupsWritten).toEqual([])
  })

  it('Status shown: the last backup time and when the next is due', async () => {
    const ago = 2 * HOUR
    const { core, dialog } = await openBackup(undefined, backedUp(ago))
    const last = core.backup.last!
    expect(statusText(dialog)).toBe(
      `Last backup ${dayTime(last)} Next backup ${dayTime(last + 24 * HOUR)}`,
    )
    expect(dialog.querySelector('[data-testid="backup-error"]')).toBeNull()
  })

  it('says the next backup waits for a change, or that auto is off', async () => {
    const { core, dialog } = await openBackup(
      undefined,
      backedUp(HOUR, { changed: false }),
    )
    expect(statusText(dialog)).toMatch(/Next backup After your next change/)
    check(field(dialog, 'Back up automatically'), false)
    await settle()
    expect(core.callsTo('setBackupOptions')).toEqual([
      [{ auto: false, interval: 'daily', keep: 14 }],
    ])
    expect(statusText(dialog)).toMatch(/Next backup Automatic backups are off/)
  })

  it('saves the interval and how many to keep', async () => {
    const { core, dialog } = await openBackup(undefined, backedUp(HOUR))
    const select = dialog.querySelector('select')!
    select.value = 'weekly'
    select.dispatchEvent(new Event('change', { bubbles: true }))
    await settle()
    const keep = field(dialog, 'Backups to keep')
    keep.value = '30'
    keep.dispatchEvent(new Event('change', { bubbles: true }))
    await settle()
    // Not a count: put back, not saved.
    keep.value = '0'
    keep.dispatchEvent(new Event('change', { bubbles: true }))
    await settle()
    expect(core.callsTo('setBackupOptions')).toEqual([
      [{ auto: true, interval: 'weekly', keep: 14 }],
      [{ auto: true, interval: 'weekly', keep: 30 }],
    ])
    expect(keep.value).toBe('30')
    expect(core.backup.keep).toBe(30)
  })

  it('backs up now, delivering the editor’s edits first', async () => {
    const { core, root, dialog } = await openBackup(
      [{ id: 'a', body: 'A' }],
      backedUp(HOUR),
    )
    click(button(dialog, 'Close settings'))
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('A'),
    )
    typeInEditor(root, 'B')
    click(button(root, 'Settings'))
    await settle()
    const reopened = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(reopened, 'Backup'))
    click(button(reopened, 'Back up now'))
    await settle()
    expect(core.backupsWritten).toEqual([
      [expect.objectContaining({ id: 'a', body: 'BA' })],
    ])
    expect(statusText(reopened)).toMatch(/Next backup After your next change/)
  })

  it('shows why the last backup failed', async () => {
    const { core, dialog } = await openBackup(undefined, backedUp(HOUR))
    core.backupWriteError = 'the backup folder is unavailable'
    click(button(dialog, 'Back up now'))
    await settle()
    expect(
      dialog.querySelector('[data-testid="backup-error"]')!.textContent!.trim(),
    ).toBe('The last backup failed. The backup folder is unavailable.')
    expect(button(dialog, 'Back up now').disabled).toBe(false)
  })

  it('follows status changes from the core', async () => {
    const { core, dialog } = await openBackup(undefined, backedUp(HOUR))
    core.emit('backup-status-changed', {
      ...core.backup,
      running: true,
    })
    flushSync()
    expect(button(dialog, 'Backing up…').disabled).toBe(true)
  })
})

describe('Restore from settings', () => {
  const FOLDER = '/Users/alice/Dropbox/Notebook'
  const OLDER = {
    file_name: 'notebook-backup-20261001-090000.nbbackup',
    same_vault: true,
    notes: [{ id: 'old', body: '# From the backup' }],
  }

  async function openRestore(
    folder: string | null = FOLDER,
    backupFile: FakeBackupFile = OLDER,
  ) {
    const opened = await shell([{ id: 'a', body: '# Current' }], (core) => {
      core.backup.folder = folder
      core.backupFile = backupFile
    })
    click(button(opened.root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Backup'))
    click(button(dialog, 'Restore from backup…'))
    await settle()
    return { ...opened, dialog }
  }

  const dialogOpen = () => document.querySelector('[role="dialog"]') !== null

  it('Safety backup first: backs up the current notes, then replaces them', async () => {
    const { core, root, dialog } = await openRestore()
    expect(dialog.textContent).toMatch(/Replace your notes\?/)
    expect(dialog.textContent).toMatch(
      /A backup of your current notes is written to your backup folder first/,
    )
    expect(core.callsTo('restoreBackup')).toEqual([])

    click(button(dialog, 'Replace notes'))
    await settle()
    expect(core.callsTo('restoreBackup')).toEqual([
      [{ recoveryKey: null, skipSafetyBackup: false }],
    ])
    expect(core.backupsWritten).toEqual([
      [expect.objectContaining({ id: 'a', body: '# Current' })],
    ])
    expect(dialogOpen()).toBe(false)
    expect(titles(root)).toEqual(['From the backup'])
  })

  it('Cancelled confirmation: no backup is written and the notes are unchanged', async () => {
    const { core, root, dialog } = await openRestore()
    click(button(dialog, 'Cancel'))
    await settle()
    expect(core.callsTo('restoreBackup')).toEqual([])
    // The pick is forgotten, so it can't be restored without a new one.
    expect(core.callsTo('cancelRestore')).toHaveLength(1)
    expect(core.backupsWritten).toEqual([])
    expect(core.has('a')).toBe(true)
    expect(hasButton(dialog, 'Restore from backup…')).toBe(true)
    click(button(dialog, 'Close settings'))
    expect(titles(root)).toEqual(['Current'])
  })

  it('asks again when the safety backup fails, then restores without one', async () => {
    const { core, root, dialog } = await openRestore()
    core.backupWriteError = 'the backup folder is unavailable'
    click(button(dialog, 'Replace notes'))
    await settle()
    expect(alerts(dialog)).toEqual([
      'Your current notes couldn’t be backed up first, so nothing was replaced. The backup folder is unavailable.',
    ])
    expect(core.has('a')).toBe(true)
    expect(core.has('old')).toBe(false)

    click(button(dialog, 'Replace without a backup'))
    await settle()
    expect(core.callsTo('restoreBackup')).toEqual([
      [{ recoveryKey: null, skipSafetyBackup: false }],
      [{ recoveryKey: null, skipSafetyBackup: true }],
    ])
    expect(core.backupsWritten).toEqual([])
    expect(titles(root)).toEqual(['From the backup'])
  })

  it('cancelling the second confirmation keeps the notes', async () => {
    const { core, dialog } = await openRestore()
    core.backupWriteError = 'disk full'
    click(button(dialog, 'Replace notes'))
    await settle()
    click(button(dialog, 'Cancel'))
    await settle()
    expect(core.callsTo('restoreBackup')).toHaveLength(1)
    expect(core.has('a')).toBe(true)
  })

  it('says no safety backup is made without a folder', async () => {
    const { dialog } = await openRestore(null)
    expect(dialog.textContent).toMatch(
      /aren’t backed up first, since no backup folder is set/,
    )
  })

  it('asks for the recovery key of another notebook’s backup', async () => {
    const { core, root, dialog } = await openRestore(FOLDER, {
      ...OLDER,
      same_vault: false,
    })
    expect(hasButton(dialog, 'Replace notes')).toBe(false)
    fill(field(dialog, 'Recovery key'), 'ABCD')
    click(button(dialog, 'Continue'))
    click(button(dialog, 'Replace notes'))
    await settle()
    // Mistyped: back to the key, nothing replaced.
    expect(alerts(dialog)).toEqual([
      'That recovery key looks mistyped. Check each group and try again.',
    ])
    expect(core.backupsWritten).toEqual([])
    expect(core.has('a')).toBe(true)

    fill(field(dialog, 'Recovery key'), core.recoveryKey)
    click(button(dialog, 'Continue'))
    click(button(dialog, 'Replace notes'))
    await settle()
    expect(core.callsTo('restoreBackup').at(-1)).toEqual([
      { recoveryKey: core.recoveryKey, skipSafetyBackup: false },
    ])
    expect(titles(root)).toEqual(['From the backup'])
  })

  it('shows nothing when the file picker is dismissed', async () => {
    const opened = await shell([{ id: 'a', body: '# Current' }])
    click(button(opened.root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Backup'))
    click(button(dialog, 'Restore from backup…'))
    await settle()
    expect(opened.core.callsTo('pickBackup')).toHaveLength(1)
    expect(alerts(dialog)).toEqual([])
    expect(hasButton(dialog, 'Replace notes')).toBe(false)
  })

  it('closes the open note, whose notebook was replaced', async () => {
    const { core, root } = await shell([{ id: 'a', body: 'A' }], (core) => {
      core.backupFile = OLDER
    })
    click(entry(root, 'a'))
    await vi.waitFor(() =>
      expect(editorView(root).state.doc.toString()).toBe('A'),
    )
    typeInEditor(root, 'B')
    click(button(root, 'Settings'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    click(button(dialog, 'Backup'))
    click(button(dialog, 'Restore from backup…'))
    await settle()
    click(button(dialog, 'Replace notes'))
    await settle()
    const methods = core.calls.map((c) => c.method)
    expect(methods.lastIndexOf('editorPush')).toBeLessThan(
      methods.indexOf('restoreBackup'),
    )
    expect(root.querySelector('.editor')!.hasAttribute('hidden')).toBe(true)
    expect(titles(root)).toEqual(['From the backup'])
  })
})

describe('backup notice', () => {
  const FOLDER = '/Users/alice/Dropbox/Notebook'
  const footer = (root: HTMLElement) =>
    root.querySelector<HTMLElement>('.sidebar footer')!

  it('Dropbox folder gone: says backups are failing and opens Backup settings', async () => {
    const { core, root } = await shell([], (core) => {
      Object.assign(core.backup, { folder: FOLDER, last: Date.now() })
    })
    expect(hasButton(footer(root), 'Backups are failing')).toBe(false)
    // An automatic backup fails: the folder is gone.
    core.emit('backup-status-changed', {
      ...core.backup,
      last_error: 'the backup folder is unavailable',
      failing: true,
    })
    flushSync()
    click(button(footer(root), 'Backups are failing'))
    await settle()
    const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!
    expect(
      dialog.querySelector('[role="tab"][aria-selected="true"]')!.textContent,
    ).toMatch('Backup')
    expect(
      dialog.querySelector('[data-testid="backup-error"]')!.textContent!.trim(),
    ).toBe('The last backup failed. The backup folder is unavailable.')
  })

  it('says when backups are overdue', async () => {
    const { root } = await shell([], (core) => {
      Object.assign(core.backup, {
        folder: FOLDER,
        last: Date.now() - 3 * 86_400_000,
        changed: true,
      })
    })
    expect(hasButton(footer(root), 'Backup overdue')).toBe(true)
  })

  it('Healthy: no notice', async () => {
    const { core, root } = await shell([], (core) => {
      Object.assign(core.backup, { folder: FOLDER, last: Date.now() })
    })
    expect(footer(root).querySelector('.status')!.textContent!.trim()).toBe('')
    // Recovering clears it.
    core.emit('backup-status-changed', { ...core.backup, failing: true })
    flushSync()
    expect(hasButton(footer(root), 'Backups are failing')).toBe(true)
    core.emit('backup-status-changed', { ...core.backup, failing: false })
    flushSync()
    expect(footer(root).querySelector('.status')!.textContent!.trim()).toBe('')
  })

  it('shows nothing before a folder is chosen', async () => {
    const { core, root } = await shell()
    core.emit('backup-status-changed', {
      ...core.backup,
      failing: true,
      overdue: true,
    })
    flushSync()
    expect(footer(root).querySelector('.status')!.textContent!.trim()).toBe('')
  })
})
