// @vitest-environment jsdom
import { syntaxTree } from '@codemirror/language'
import {
  openSearchPanel,
  replaceAll,
  SearchQuery,
  setSearchQuery,
} from '@codemirror/search'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { NoteClosedError } from './bridge'
import { body, cursorAtEnd, press, select, setup, type } from './testing'

afterEach(() => {
  vi.useRealTimers()
})

describe('editing the note document', () => {
  it('opening a note shows its body', async () => {
    const { editor, view } = setup({ a: '# Groceries\n- eggs' })
    await editor.openNote('a')
    expect(body(view)).toBe('# Groceries\n- eggs')
    expect(editor.noteId).toBe('a')
  })

  it('typing changes the document', async () => {
    const { editor, view, bridge } = setup({ a: '# Groceries\n- eggs' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, ' and milk')
    await editor.flush()
    expect(bridge.body('a')).toBe('# Groceries\n- eggs and milk')
  })

  it('shows nothing editable with no note open', async () => {
    const { editor, view } = setup({ a: 'x' })
    expect(view.state.readOnly).toBe(true)
    await editor.openNote('a')
    expect(view.state.readOnly).toBe(false)
    await editor.openNote(null)
    expect(body(view)).toBe('')
    expect(view.state.readOnly).toBe(true)
  })

  it('keeps carriage returns as text', async () => {
    const { editor, view, bridge } = setup({ a: 'one\r\ntwo' })
    await editor.openNote('a')
    expect(body(view)).toBe('one\r\ntwo')
    cursorAtEnd(view)
    type(view, '!')
    await editor.flush()
    expect(bridge.body('a')).toBe('one\r\ntwo!')
  })

  it('highlights headings and emphasis without changing the text', async () => {
    const { editor, view } = setup({ a: '# Title\nsome *emphasis*' })
    await editor.openNote('a')
    const nodes: string[] = []
    syntaxTree(view.state).iterate({ enter: (n) => void nodes.push(n.name) })
    expect(nodes).toContain('ATXHeading1')
    expect(nodes).toContain('Emphasis')
    expect(body(view)).toBe('# Title\nsome *emphasis*')
  })

  it('wraps long lines without changing the body', async () => {
    const long = 'word '.repeat(200)
    const { editor, view } = setup({ a: long })
    await editor.openNote('a')
    expect(view.contentDOM.classList).toContain('cm-lineWrapping')
    expect(body(view)).toBe(long)
  })

  it('keeps its styles out of the document', async () => {
    // A strict CSP blocks `<style>` elements in the document.
    const { editor, view, parent } = setup({ a: 'x' })
    await editor.openNote('a')
    expect(view.root).toBe(parent.firstElementChild!.shadowRoot)
    expect(document.querySelectorAll('style')).toHaveLength(0)
  })
})

describe('remote edits', () => {
  it('remote edit appears', async () => {
    const { editor, view, bridge } = setup({ a: 'one\ntwo' })
    await editor.openNote('a')
    bridge.remoteEdit('a', (text) => {
      text.delete(4, 3)
      text.insert(4, 'TWO')
    })
    expect(body(view)).toBe('one\nTWO')
  })

  it('concurrent local and remote edits', async () => {
    const { editor, view, bridge } = setup({ a: 'one\ntwo' })
    await editor.openNote('a')
    view.dispatch({
      changes: { from: 0, to: 3, insert: 'ONE' },
      userEvent: 'input.type',
    })
    bridge.remoteEdit('a', (text) => {
      text.delete(4, 3)
      text.insert(4, 'TWO')
    })
    await editor.flush()
    expect(body(view)).toBe('ONE\nTWO')
    expect(bridge.body('a')).toBe('ONE\nTWO')
  })

  it('remote insert before the cursor', async () => {
    const { editor, view, bridge } = setup({ a: 'hello world' })
    await editor.openNote('a')
    select(view, 11)
    bridge.remoteEdit('a', (text) => text.insert(6, 'big '))
    expect(body(view)).toBe('hello big world')
    expect(view.state.selection.main.head).toBe(15)
  })

  it('remote edit after the selection', async () => {
    const { editor, view, bridge } = setup({ a: 'hello world' })
    await editor.openNote('a')
    select(view, 0, 5)
    bridge.remoteEdit('a', (text) => text.insert(11, '!'))
    const { from, to } = view.state.selection.main
    expect(view.state.sliceDoc(from, to)).toBe('hello')
  })

  it('remote edits are not sent back', async () => {
    const { editor, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    bridge.remoteEdit('a', (text) => text.insert(0, '!'))
    await editor.flush()
    expect(bridge.pushes).toBe(0)
  })
})

describe('undo', () => {
  it('undo skips remote edits', async () => {
    const { editor, view, bridge } = setup({ a: 'one two' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, 'A')
    bridge.remoteEdit('a', (text) => text.insert(0, 'B'))
    expect(body(view)).toBe('Bone twoA')
    expect(editor.undo()).toBe(true)
    expect(body(view)).toBe('Bone two')
  })

  it('redo', async () => {
    const { editor, view } = setup({ a: 'x' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, 'A')
    editor.undo()
    expect(body(view)).toBe('x')
    expect(editor.redo()).toBe(true)
    expect(body(view)).toBe('xA')
  })

  it('undo and redo work from the keyboard', async () => {
    const { editor, view } = setup({ a: 'x' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, 'A')
    press(view, 'z', { ctrlKey: true })
    expect(body(view)).toBe('x')
    press(view, 'z', { ctrlKey: true, shiftKey: true })
    expect(body(view)).toBe('xA')
  })

  it('fresh history on open', async () => {
    const { editor, view } = setup({ a: 'first', b: 'second' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, '!')
    await editor.openNote('b')
    expect(editor.undo()).toBe(false)
    expect(body(view)).toBe('second')
    await editor.openNote('a')
    expect(editor.undo()).toBe(false)
    expect(body(view)).toBe('first!')
  })

  it('undone edits are delivered too', async () => {
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, 'A')
    await editor.flush()
    editor.undo()
    await editor.flush()
    expect(bridge.body('a')).toBe('x')
  })
})

describe('delivery', () => {
  it('idle delivery', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, ' word')
    await vi.advanceTimersByTimeAsync(299)
    expect(bridge.body('a')).toBe('x')
    await vi.advanceTimersByTimeAsync(1)
    expect(bridge.body('a')).toBe('x word')
    expect(bridge.pushes).toBe(1)
  })

  it('continuous typing is delivered within a second, merged', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge } = setup({ a: '' })
    await editor.openNote('a')
    for (let i = 0; i < 10; i++) {
      type(view, 'a')
      await vi.advanceTimersByTimeAsync(100)
    }
    expect(bridge.body('a')).toBe('aaaaaaaaaa')
    expect(bridge.pushes).toBe(1)
  })

  it('closing delivers immediately', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, 'y')
    await editor.destroy()
    expect(bridge.body('a')).toBe('xy')
    expect(bridge.attached('a')).toBe(0)
  })

  it('switching notes keeps pending edits', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge } = setup({ a: 'note a', b: 'note b' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, '!')
    await editor.openNote('b')
    expect(bridge.body('a')).toBe('note a!')
    expect(body(view)).toBe('note b')
    expect(bridge.attached('a')).toBe(0)
  })

  it('hiding the page delivers immediately', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, 'y')
    window.dispatchEvent(new Event('pagehide'))
    await vi.advanceTimersByTimeAsync(0)
    expect(bridge.body('a')).toBe('xy')

    type(view, 'z')
    const visibility = vi
      .spyOn(document, 'visibilityState', 'get')
      .mockReturnValue('hidden')
    document.dispatchEvent(new Event('visibilitychange'))
    visibility.mockRestore()
    await vi.advanceTimersByTimeAsync(0)
    expect(bridge.body('a')).toBe('xyz')
  })

  it('delivery fails then succeeds', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge, errors } = setup({ a: '' })
    await editor.openNote('a')
    bridge.failPushes = true
    type(view, 'one')
    await vi.advanceTimersByTimeAsync(300)
    expect(errors).toHaveLength(1)
    expect(bridge.body('a')).toBe('')
    type(view, ' two')
    bridge.failPushes = false
    await vi.advanceTimersByTimeAsync(300)
    expect(bridge.body('a')).toBe('one two')
  })

  it('a push the core will never take is dropped and the note detached', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge, errors } = setup({ a: '' })
    await editor.openNote('a')
    bridge.pushError = new NoteClosedError('locked')
    type(view, 'one')
    await vi.advanceTimersByTimeAsync(300)
    expect(errors).toEqual([bridge.pushError])
    expect(bridge.attached('a')).toBe(0)

    // No retries, and later typing goes nowhere.
    bridge.pushError = null
    type(view, ' two')
    await vi.advanceTimersByTimeAsync(5000)
    expect(errors).toHaveLength(1)
    expect(bridge.pushes).toBe(0)

    // Closing or switching afterwards doesn't try again either.
    await editor.openNote(null)
    await vi.advanceTimersByTimeAsync(5000)
    expect(bridge.pushes).toBe(0)
    expect(errors).toHaveLength(1)
  })

  it('a failed delivery is retried without more typing', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge, errors } = setup({ a: '' })
    await editor.openNote('a')
    bridge.failPushes = true
    type(view, 'one')
    await vi.advanceTimersByTimeAsync(300)
    bridge.failPushes = false
    await vi.advanceTimersByTimeAsync(1000)
    expect(errors).toHaveLength(1)
    expect(bridge.body('a')).toBe('one')
  })

  it('a failed delivery on switch keeps retrying the old note', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge, errors } = setup({ a: 'a', b: 'b' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, '!')
    bridge.failPushes = true
    await editor.openNote('b')
    expect(errors.length).toBeGreaterThan(0)
    expect(body(view)).toBe('b')
    bridge.failPushes = false
    await vi.advanceTimersByTimeAsync(1000)
    expect(bridge.body('a')).toBe('a!')
    expect(bridge.attached('a')).toBe(0)
  })

  it('retries a failed switch once a second', async () => {
    vi.useFakeTimers()
    const { editor, view, bridge, errors } = setup({ a: 'a', b: 'b' })
    await editor.openNote('a')
    type(view, '!')
    bridge.failPushes = true
    await editor.openNote('b')
    await vi.advanceTimersByTimeAsync(5000)
    expect(errors).toHaveLength(6)
  })

  it('nothing is editable while the next note opens', async () => {
    const { editor, view, bridge } = setup({ a: 'a', b: 'b' })
    await editor.openNote('a')
    cursorAtEnd(view)
    type(view, '!')
    const open = bridge.open.bind(bridge)
    let release: (() => void) | undefined
    bridge.open = (id) =>
      new Promise((resolve) => (release = () => resolve(open(id))))
    const opening = editor.openNote('b')
    await vi.waitFor(() => expect(release).toBeDefined())
    expect(body(view)).toBe('')
    expect(view.state.readOnly).toBe(true)
    expect(bridge.body('a')).toBe('a!')
    release!()
    await opening
    expect(body(view)).toBe('b')
  })

  it('a failed open leaves nothing editable', async () => {
    const { editor, view, bridge } = setup({ a: 'a' })
    await editor.openNote('a')
    bridge.open = () => Promise.reject(new Error('no such note'))
    await expect(editor.openNote('b')).rejects.toThrow('no such note')
    expect(editor.noteId).toBeNull()
    expect(body(view)).toBe('')
    expect(view.state.readOnly).toBe(true)
    expect(await editor.flush()).toBe(true)
  })
})

describe('markdown commands', () => {
  const enterAtEnd = async (text: string) => {
    const { editor, view } = setup({ a: text })
    await editor.openNote('a')
    cursorAtEnd(view)
    press(view, 'Enter')
    return view
  }

  it('bullet list', async () => {
    const view = await enterAtEnd('- eggs')
    expect(body(view)).toBe('- eggs\n- ')
    expect(view.state.selection.main.head).toBe(body(view).length)
  })

  it('ordered list', async () => {
    const view = await enterAtEnd('1. first')
    expect(body(view)).toBe('1. first\n2. ')
  })

  it('task list', async () => {
    const view = await enterAtEnd('- [x] done')
    expect(body(view)).toBe('- [x] done\n- [ ] ')
  })

  it('empty item ends the list', async () => {
    const view = await enterAtEnd('- eggs\n- ')
    expect(body(view)).toBe('- eggs\n')
  })

  it('replace all', async () => {
    const { editor, view, bridge } = setup({ a: 'cat and cat' })
    await editor.openNote('a')
    openSearchPanel(view)
    view.dispatch({
      effects: setSearchQuery.of(
        new SearchQuery({ search: 'cat', replace: 'dog' }),
      ),
    })
    expect(replaceAll(view)).toBe(true)
    expect(body(view)).toBe('dog and dog')
    await editor.flush()
    expect(bridge.body('a')).toBe('dog and dog')
    editor.undo()
    expect(body(view)).toBe('cat and cat')
  })
})

describe('read-only', () => {
  it('read-only note', async () => {
    const { editor, view, bridge } = setup({ a: 'cat and cat' })
    await editor.openNote('a')
    editor.setReadOnly(true)
    type(view, 'typed')
    press(view, 'Enter')
    openSearchPanel(view)
    view.dispatch({
      effects: setSearchQuery.of(
        new SearchQuery({ search: 'cat', replace: 'dog' }),
      ),
    })
    replaceAll(view)
    editor.setVimMode(true)
    const { Vim, getCM } = await import('@replit/codemirror-vim')
    const cm = getCM(view)!
    Vim.handleKey(cm, '<Esc>', 'user')
    Vim.handleKey(cm, 'd', 'user')
    Vim.handleKey(cm, 'd', 'user')
    expect(editor.undo()).toBe(false)
    await editor.flush()
    expect(body(view)).toBe('cat and cat')
    expect(bridge.pushes).toBe(0)
  })

  it('remote edits still appear while read-only', async () => {
    const { editor, view, bridge } = setup({ a: 'x' })
    await editor.openNote('a')
    editor.setReadOnly(true)
    bridge.remoteEdit('a', (text) => text.insert(1, 'y'))
    expect(body(view)).toBe('xy')
    expect(view.state.readOnly).toBe(true)
  })

  it('stays read-only across notes until turned off', async () => {
    const { editor, view } = setup({ a: 'x', b: 'y' })
    editor.setReadOnly(true)
    await editor.openNote('a')
    expect(view.state.readOnly).toBe(true)
    editor.setReadOnly(false)
    await editor.openNote('b')
    expect(view.state.readOnly).toBe(false)
  })
})
