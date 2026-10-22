// @vitest-environment jsdom
import { getCM, Vim } from '@replit/codemirror-vim'
import type { EditorView } from '@codemirror/view'
import { describe, expect, it, vi } from 'vitest'
import { memorySettings } from '../platform/settings'
import { createNoteEditor, VIM_MODE_KEY } from './noteEditor'
import { body, cursorAtEnd, press, select, setup, type } from './testing'

/** Sends vim keys, one at a time (`<Esc>` style names allowed). */
function keys(view: EditorView, ...sequence: string[]) {
  const cm = getCM(view)
  if (!cm) throw new Error('vim is off')
  for (const key of sequence) Vim.handleKey(cm, key, 'user')
}

function ex(view: EditorView, command: string) {
  const cm = getCM(view)
  if (!cm) throw new Error('vim is off')
  // Vim has attached its state by now, which is all the narrower type adds.
  Vim.handleEx(cm as Parameters<typeof Vim.handleEx>[0], command)
}

describe('vim mode', () => {
  it('vim off by default', async () => {
    const { editor, view } = setup({ a: '' })
    await editor.ready
    await editor.openNote('a')
    expect(editor.vimMode).toBe(false)
    expect(getCM(view)).toBeNull()
    // No keymap claims a plain `d`, so the keystroke becomes text.
    const event = new KeyboardEvent('keydown', {
      key: 'd',
      bubbles: true,
      cancelable: true,
    })
    view.contentDOM.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(false)
    type(view, 'd')
    expect(body(view)).toBe('d')
  })

  it('vim motions when on', async () => {
    const { editor, view } = setup({ a: 'keep me\ndelete me\nkeep me too' })
    await editor.openNote('a')
    editor.setVimMode(true)
    select(view, 10)
    keys(view, 'd', 'd')
    expect(body(view)).toBe('keep me\nkeep me too')
  })

  it('toggle while editing', async () => {
    const { editor, view } = setup({ a: 'hello' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, ' world')
    const head = view.state.selection.main.head
    editor.setVimMode(true)
    expect(body(view)).toBe('hello world')
    expect(view.state.selection.main.head).toBe(head)
    const cm = getCM(view)!
    expect(cm.state.vim?.insertMode).toBeFalsy()
    // `u` goes through the note's undo history.
    keys(view, 'u')
    expect(body(view)).toBe('hello')
    keys(view, '<C-r>')
    expect(body(view)).toBe('hello world')
    editor.setVimMode(false)
    expect(getCM(view)).toBeNull()
    expect(editor.undo()).toBe(true)
    expect(body(view)).toBe('hello')
  })

  it('u undoes a local edit but not a remote one', async () => {
    const { editor, view, bridge } = setup({ a: 'one two' })
    await editor.openNote('a')
    editor.setVimMode(true)
    keys(view, 'A')
    type(view, 'x')
    keys(view, '<Esc>')
    bridge.remoteEdit('a', (text) => text.insert(0, 'R'))
    expect(body(view)).toBe('Rone twox')
    keys(view, 'u')
    expect(body(view)).toBe('Rone two')
  })

  it('applies to the next note too', async () => {
    const { editor, view } = setup({ a: 'a', b: 'b' })
    await editor.openNote('a')
    editor.setVimMode(true)
    await editor.openNote('b')
    expect(getCM(view)).not.toBeNull()
  })
})

describe('vim mode preference', () => {
  it('persists on the device', async () => {
    const first = setup({ a: 'x' })
    first.editor.setVimMode(true)
    expect(await first.settings.get(VIM_MODE_KEY)).toBe('true')
    await first.editor.destroy()

    const parent = document.createElement('div')
    const again = createNoteEditor(parent, {
      bridge: first.bridge,
      settings: first.settings,
    })
    await again.ready
    await again.openNote('a')
    expect(again.vimMode).toBe(true)
    expect(getCM(again.view)).not.toBeNull()
  })

  it('unreadable preference', async () => {
    const settings = memorySettings()
    await settings.set(VIM_MODE_KEY, 'true')
    settings.options.failReads = true
    const errors: unknown[] = []
    const { bridge } = setup({ a: 'x' })
    const editor = createNoteEditor(document.createElement('div'), {
      bridge,
      settings,
      onError: (e) => errors.push(e),
    })
    await editor.ready
    await editor.openNote('a')
    expect(editor.vimMode).toBe(false)
    expect(errors).toEqual([])
  })

  it('failed save', async () => {
    const { editor, view, settings, errors } = setup({ a: 'x' })
    await editor.openNote('a')
    settings.options.failWrites = true
    editor.setVimMode(true)
    await vi.waitFor(() => expect(errors).toHaveLength(1))
    expect(editor.vimMode).toBe(true)
    expect(getCM(view)).not.toBeNull()
  })

  it('an explicit choice wins over a slow read', async () => {
    const settings = memorySettings()
    await settings.set(VIM_MODE_KEY, 'true')
    const { bridge } = setup({ a: 'x' })
    const editor = createNoteEditor(document.createElement('div'), {
      bridge,
      settings,
    })
    editor.setVimMode(false)
    await editor.ready
    expect(editor.vimMode).toBe(false)
  })

  it('never goes through the bridge', async () => {
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    editor.setVimMode(true)
    editor.setVimMode(false)
    await editor.flush()
    expect(bridge.pushes).toBe(0)
    expect(body(view)).toBe('x')
    expect([...bridge.doc('a').share.keys()]).toEqual(['body'])
  })
})

describe('vim ex commands', () => {
  it('write', async () => {
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    editor.setVimMode(true)
    keys(view, 'A')
    type(view, 'y')
    keys(view, '<Esc>')
    ex(view, 'w')
    await vi.waitFor(() => expect(view.dom.textContent).toContain('Saved'))
    expect(bridge.body('a')).toBe('xy')
  })

  it('quit does nothing destructive', async () => {
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    editor.setVimMode(true)
    keys(view, 'A')
    type(view, 'y')
    keys(view, '<Esc>')
    for (const command of ['q', 'quit', 'wq', 'x']) ex(view, command)
    await editor.flush()
    expect(editor.noteId).toBe('a')
    expect(body(view)).toBe('xy')
    expect(bridge.body('a')).toBe('xy')
    expect(view.dom.isConnected).toBe(true)
  })

  it('read-only blocks vim edits', async () => {
    const { editor, view } = setup({ a: 'delete me' })
    await editor.openNote('a')
    editor.setVimMode(true)
    editor.setReadOnly(true)
    keys(view, 'd', 'd')
    press(view, 'x')
    expect(body(view)).toBe('delete me')
  })
})
