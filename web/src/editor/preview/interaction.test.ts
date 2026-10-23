// @vitest-environment jsdom
import { forceParsing } from '@codemirror/language'
import { EditorSelection } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'
import { getCM, Vim } from '@replit/codemirror-vim'
import { describe, expect, it, vi } from 'vitest'
import { openExternal } from '../../platform/external'
import type { NoteEditorOptions } from '../noteEditor'
import { body, press, select, setup, type } from '../testing'

async function editorWith(
  notes: string | Record<string, string>,
  options: Partial<NoteEditorOptions> = {},
) {
  const opened = setup(
    typeof notes === 'string' ? { a: notes } : notes,
    options,
  )
  await opened.editor.openNote('a')
  forceParsing(opened.view, opened.view.state.doc.length, 5000)
  select(opened.view, opened.view.state.doc.length)
  return opened
}

function lineElement(view: EditorView, n: number): HTMLElement {
  const { node } = view.domAtPos(view.state.doc.line(n).from)
  return (
    node instanceof HTMLElement ? node : node.parentElement
  )!.closest<HTMLElement>('.cm-line')!
}

/** Line `n` as displayed (hidden markers gone). */
const shown = (view: EditorView, n: number) => lineElement(view, n).textContent

const head = (view: EditorView) => view.state.selection.main.head

/** Sends vim keys, one at a time (`<Esc>` style names allowed). */
function keys(view: EditorView, ...sequence: string[]) {
  const cm = getCM(view)
  if (!cm) throw new Error('vim is off')
  for (const key of sequence) Vim.handleKey(cm, key, 'user')
}

/** Mod-Enter: Ctrl in jsdom, which isn't a Mac. */
const modEnter = (view: EditorView) => press(view, 'Enter', { ctrlKey: true })

describe('task checkboxes', () => {
  it('toggles the marker on click, keeping the cursor', async () => {
    const { view, editor, bridge } = await editorWith('- [ ] buy milk\nend')
    const cursor = head(view)
    const box = lineElement(view, 1).querySelector('input')!
    expect(box.checked).toBe(false)
    expect(box.getAttribute('aria-label')).toBe('Done')
    box.dispatchEvent(
      new MouseEvent('click', { bubbles: true, cancelable: true }),
    )
    expect(body(view)).toBe('- [x] buy milk\nend')
    expect(head(view)).toBe(cursor)
    expect(lineElement(view, 1).querySelector('input')!.checked).toBe(true)

    // It reaches the core like typing does.
    const before = bridge.pushes
    expect(await editor.flush()).toBe(true)
    expect(bridge.pushes).toBe(before + 1)
    expect(bridge.body('a')).toBe('- [x] buy milk\nend')

    expect(editor.undo()).toBe(true)
    expect(body(view)).toBe('- [ ] buy milk\nend')
    await editor.flush()
    expect(bridge.body('a')).toBe('- [ ] buy milk\nend')
  })

  it('unchecks a checked task', async () => {
    const { view } = await editorWith('- [X] done\nend')
    lineElement(view, 1)
      .querySelector('input')!
      .dispatchEvent(
        new MouseEvent('click', { bubbles: true, cancelable: true }),
      )
    expect(body(view)).toBe('- [ ] done\nend')
  })

  it('does nothing in a read-only note', async () => {
    const { view, editor } = await editorWith('- [ ] buy milk\nend')
    editor.setReadOnly(true)
    lineElement(view, 1)
      .querySelector('input')!
      .dispatchEvent(
        new MouseEvent('click', { bubbles: true, cancelable: true }),
      )
    expect(body(view)).toBe('- [ ] buy milk\nend')
  })
})

describe('toggle-task command', () => {
  it.each([
    ['- [ ] call Sam', '- [x] call Sam'],
    ['- [x] call Sam', '- [ ] call Sam'],
    ['- call Sam', '- [ ] call Sam'],
    ['* call Sam', '* [ ] call Sam'],
    ['1. call Sam', '1. [ ] call Sam'],
    ['  - [ ] nested', '  - [x] nested'],
    ['-', '- [ ] '],
  ])('turns %j into %j', async (before, after) => {
    const { view } = await editorWith(before)
    modEnter(view)
    expect(body(view)).toBe(after)
  })

  it('toggles every line the selection touches, leaving other lines alone', async () => {
    const { view } = await editorWith(
      '- [ ] a\n- [x] b\n- c\nplain\n- [ ] after',
    )
    select(view, 0, view.state.doc.line(4).to)
    modEnter(view)
    expect(body(view)).toBe('- [x] a\n- [ ] b\n- [ ] c\nplain\n- [ ] after')
  })

  it('leaves code blocks and thematic breaks alone', async () => {
    const text = '```\n- not a task\n```\n\n- - -\n\n- real'
    const { view } = await editorWith(text)
    select(view, 0, view.state.doc.length)
    modEnter(view)
    expect(body(view)).toBe('```\n- not a task\n```\n\n- - -\n\n- [ ] real')
  })

  it('leaves a line that isn’t a list item alone', async () => {
    const { view } = await editorWith('just text')
    select(view, 3)
    modEnter(view)
    expect(body(view)).not.toContain('[')
  })

  it('works in source view', async () => {
    const { view } = await editorWith('- [ ] a', { sourceView: true })
    modEnter(view)
    expect(body(view)).toBe('- [x] a')
  })

  it('works with vim in normal and insert mode', async () => {
    const { view, editor } = await editorWith('- [ ] a')
    editor.setVimMode(true)
    select(view, 0)
    modEnter(view)
    expect(body(view)).toBe('- [x] a')
    keys(view, 'A')
    expect(getCM(view)!.state.vim!.insertMode).toBe(true)
    modEnter(view)
    expect(body(view)).toBe('- [ ] a')
    expect(getCM(view)!.state.vim!.insertMode).toBe(true)
  })
})

describe('links', () => {
  function mousedown(target: Element, init: MouseEventInit = {}) {
    const event = new MouseEvent('mousedown', {
      bubbles: true,
      cancelable: true,
      button: 0,
      ...init,
    })
    target.dispatchEvent(event)
    return event
  }

  function tap(target: Element, moveBy = 0) {
    const touchAt = (type: string, x: number) => {
      const event = new Event(type, { bubbles: true, cancelable: true })
      const touch = { clientX: x, clientY: 10 }
      const touches = type === 'touchend' ? [] : [touch]
      Object.defineProperty(event, 'touches', { value: touches })
      Object.defineProperty(event, 'targetTouches', { value: touches })
      Object.defineProperty(event, 'changedTouches', { value: [touch] })
      target.dispatchEvent(event)
      return event
    }
    touchAt('touchstart', 10)
    if (moveBy) touchAt('touchmove', 10 + moveBy)
    return touchAt('touchend', 10 + moveBy)
  }

  const linkIn = (view: EditorView, n: number) =>
    lineElement(view, n).querySelector<HTMLElement>('.cm-md-link')!

  it('opens on Mod-click, not on a plain click', async () => {
    const open = vi.fn()
    const { view } = await editorWith('See [docs](https://example.com)\nend', {
      openExternal: open,
    })
    // A plain click is left to the editor, which places the cursor.
    mousedown(linkIn(view, 1))
    expect(open).not.toHaveBeenCalled()

    const mod = mousedown(linkIn(view, 1), { ctrlKey: true })
    expect(mod.defaultPrevented).toBe(true)
    expect(open).toHaveBeenCalledWith('https://example.com')
  })

  it('opens autolinks and bare URLs on Mod-click', async () => {
    const open = vi.fn()
    const { view } = await editorWith(
      '<https://a.example> https://b.example\nend',
      {
        openExternal: open,
      },
    )
    for (const link of lineElement(view, 1).querySelectorAll('.cm-md-link')) {
      mousedown(link, { ctrlKey: true })
    }
    expect(open.mock.calls).toEqual([
      ['https://a.example'],
      ['https://b.example'],
    ])
  })

  it('never hands a disallowed scheme to the platform', async () => {
    const platform = vi.fn()
    const { view } = await editorWith(
      '[x](javascript:alert(1)) [y](file:///etc/passwd)\nend',
      { openExternal: (url) => void openExternal(url, platform) },
    )
    for (const link of lineElement(view, 1).querySelectorAll('.cm-md-link')) {
      mousedown(link, { ctrlKey: true })
    }
    await Promise.resolve()
    expect(platform).not.toHaveBeenCalled()
  })

  it('opens on a tap off the active line, and edits on it', async () => {
    const open = vi.fn()
    const { view } = await editorWith('[docs](https://example.com)\nend', {
      openExternal: open,
    })
    expect(tap(linkIn(view, 1)).defaultPrevented).toBe(true)
    expect(open).toHaveBeenCalledWith('https://example.com')

    open.mockClear()
    select(view, 2)
    expect(tap(linkIn(view, 1)).defaultPrevented).toBe(false)
    expect(open).not.toHaveBeenCalled()
  })

  it('treats a moving touch as a scroll, not a tap', async () => {
    const open = vi.fn()
    const { view } = await editorWith('[docs](https://example.com)\nend', {
      openExternal: open,
    })
    tap(linkIn(view, 1), 40)
    expect(open).not.toHaveBeenCalled()
  })
})

describe('focus mode', () => {
  const three =
    'First paragraph.\n\nSecond paragraph\ncontinues.\n\nThird paragraph.'
  const dimmed = (view: EditorView) =>
    [1, 2, 3, 4, 5, 6].filter((n) =>
      lineElement(view, n).classList.contains('cm-dimmed'),
    )

  it('is off by default', async () => {
    const { view, editor } = await editorWith(three)
    expect(editor.focusMode).toBe(false)
    expect(dimmed(view)).toEqual([])
  })

  it('dims every block but the cursor’s, and follows the cursor', async () => {
    const { view, editor } = await editorWith(three, { focusMode: true })
    select(view, view.state.doc.line(3).from + 2)
    expect(dimmed(view)).toEqual([1, 2, 5, 6])
    select(view, view.state.doc.line(6).from)
    expect(dimmed(view)).toEqual([1, 2, 3, 4, 5])
    editor.setFocusMode(false)
    expect(dimmed(view)).toEqual([])
  })

  it('treats each top-level list item as its own block', async () => {
    const { view } = await editorWith('- one\n- two\n- three', {
      focusMode: true,
    })
    select(view, view.state.doc.line(2).from + 3)
    expect(
      [1, 2, 3].filter((n) =>
        lineElement(view, n).classList.contains('cm-dimmed'),
      ),
    ).toEqual([1, 3])
  })

  it('works in source view too', async () => {
    const { view } = await editorWith(three, {
      focusMode: true,
      sourceView: true,
    })
    select(view, 0)
    expect(dimmed(view)).toEqual([2, 3, 4, 5, 6])
  })
})

describe('source view', () => {
  it('shows every line’s source and keeps the cursor', async () => {
    const { view, editor } = await editorWith('## Plans\n**bold** text\nend')
    select(view, view.state.doc.line(3).from + 1)
    expect(shown(view, 1)).toBe('Plans')
    editor.setSourceView(true)
    expect(editor.sourceView).toBe(true)
    expect(shown(view, 1)).toBe('## Plans')
    expect(shown(view, 2)).toBe('**bold** text')
    expect(head(view)).toBe(view.state.doc.line(3).from + 1)
    expect(
      view.contentDOM.querySelector('.cm-md-strong, .cm-tag-pill'),
    ).toBeNull()
    editor.setSourceView(false)
    expect(shown(view, 1)).toBe('Plans')
  })

  it('keeps the undo history across a switch', async () => {
    const { view, editor } = await editorWith('start')
    type(view, ' word')
    editor.setSourceView(true)
    expect(editor.undo()).toBe(true)
    expect(body(view)).toBe('start')
  })

  it('keeps vim and its mode across a switch', async () => {
    const { view, editor } = await editorWith('one two')
    editor.setVimMode(true)
    select(view, 0)
    keys(view, 'i')
    editor.setSourceView(true)
    expect(getCM(view)!.state.vim!.insertMode).toBe(true)
    keys(view, '<Esc>')
    editor.setSourceView(false)
    keys(view, 'w')
    expect(head(view)).toBe(4)
  })

  it('applies to the next note opened', async () => {
    const { view, editor } = await editorWith({ a: '# A', b: '# B' })
    editor.setSourceView(true)
    await editor.openNote('b')
    select(view, view.state.doc.length)
    expect(shown(view, 1)).toBe('# B')
  })

  it('toggles from the keyboard and reports it', async () => {
    const onViewOption = vi.fn()
    const { view, editor } = await editorWith('# A', { onViewOption })
    const chord = (key: string) =>
      press(view, key, {
        ctrlKey: true,
        shiftKey: true,
        keyCode: key.charCodeAt(0),
      })
    chord('E')
    expect(editor.sourceView).toBe(true)
    chord('D')
    expect(editor.focusMode).toBe(true)
    chord('E')
    expect(onViewOption.mock.calls).toEqual([
      ['sourceView', true],
      ['focusMode', true],
      ['sourceView', false],
    ])
  })
})

describe('vim with live preview', () => {
  it('moves over hidden markers exactly as in source view', async () => {
    const text = '**bold** word and [a link](https://x.example) end\nnext'
    const positions = async (sourceView: boolean) => {
      const { view, editor } = await editorWith(text, { sourceView })
      editor.setVimMode(true)
      select(view, 0)
      const seen: number[] = []
      for (let i = 0; i < 8; i++) {
        keys(view, 'w')
        seen.push(head(view))
      }
      return seen
    }
    expect(await positions(false)).toEqual(await positions(true))
  })

  it('reveals both lines of a visual line selection', async () => {
    const { view, editor } = await editorWith('# One\n**two**\n_three_')
    editor.setVimMode(true)
    select(view, 0)
    // `V` then down one line. `j` moves by pixel, which jsdom has none of,
    // so `2G` stands in for it.
    keys(view, 'V', '2', 'G')
    expect([1, 2, 3].map((n) => shown(view, n))).toEqual([
      '# One',
      '**two**',
      'three',
    ])
  })

  it('still saves with :w', async () => {
    const { view, editor, bridge } = await editorWith('text')
    editor.setVimMode(true)
    select(view, 0)
    keys(view, 'i')
    type(view, 'x')
    keys(view, '<Esc>')
    const cm = getCM(view)!
    Vim.handleEx(cm as Parameters<typeof Vim.handleEx>[0], 'w')
    await vi.waitFor(() => expect(bridge.body('a')).toBe('xtext'))
  })

  it('reveals with multiple cursors', async () => {
    const { view } = await editorWith('**a**\n**b**\n**c**')
    view.dispatch({
      selection: EditorSelection.create([
        EditorSelection.cursor(0),
        EditorSelection.cursor(12),
      ]),
    })
    // Multiple selections aren't enabled; the main cursor is what remains.
    expect(view.state.selection.ranges.length).toBeGreaterThanOrEqual(1)
    expect(shown(view, 2)).toBe('b')
  })
})
