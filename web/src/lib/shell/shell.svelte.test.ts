// @vitest-environment jsdom
import { EditorView } from '@codemirror/view'
import { getCM } from '@replit/codemirror-vim'
import { flushSync } from 'svelte'
import { afterEach, describe, expect, it, vi } from 'vitest'
import '../../editor/testing'
import { FakeCoreApi, type SeedNote } from '../../platform/fake'
import { coreSettings } from '../../platform/settings'
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
  it('keeps the status slot beside Settings, empty for now', async () => {
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
    expect(tabs).toEqual(['Appearance', 'Editor', 'About'])
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
