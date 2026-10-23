// @vitest-environment jsdom
import { markdown, markdownLanguage } from '@codemirror/lang-markdown'
import { forceParsing } from '@codemirror/language'
import { EditorSelection, EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { describe, expect, it, vi } from 'vitest'
import type { NoteEditorOptions } from '../noteEditor'
import { body, select, setup, type } from '../testing'
import { activeLines } from './activeLines'
import { previewPlugin } from './decorations'
import { livePreview } from '.'

/** An editor with `text` open in live preview, cursor at the very end. */
async function editorWith(
  text: string,
  options: Partial<NoteEditorOptions> = {},
) {
  const opened = setup({ a: text }, options)
  await opened.editor.openNote('a')
  forceParsing(opened.view, opened.view.state.doc.length, 5000)
  select(opened.view, opened.view.state.doc.length)
  return opened
}

/** Line `n` (1-based) as displayed: hidden markers gone, widgets as text. */
function shown(view: EditorView, n: number): string {
  const el = lineElement(view, n)
  let text = ''
  const walk = (node: Node) => {
    if (node instanceof HTMLInputElement) {
      text += node.checked ? '[✓]' : '[ ]'
    } else if (
      node instanceof HTMLElement &&
      node.classList.contains('cm-md-hr')
    ) {
      text += '—hr—'
    } else if (node.nodeType === Node.TEXT_NODE) {
      text += node.textContent
    } else {
      node.childNodes.forEach(walk)
    }
  }
  walk(el)
  return text
}

function lineElement(view: EditorView, n: number): HTMLElement {
  const { node } = view.domAtPos(view.state.doc.line(n).from)
  const el = (
    node instanceof HTMLElement ? node : node.parentElement
  )!.closest<HTMLElement>('.cm-line')
  if (!el) throw new Error(`line ${n} not rendered`)
  return el
}

/** The document text under every mark with class `cls`. */
function marked(view: EditorView, cls: string): string[] {
  const found: string[] = []
  view
    .plugin(previewPlugin)
    ?.decorations.between(0, view.state.doc.length, (from, to, deco) => {
      if (deco.spec.class === cls && to > from)
        found.push(view.state.sliceDoc(from, to))
    })
  return found
}

/** The document text hidden by replace decorations without a widget. */
function hidden(view: EditorView): string[] {
  const found: string[] = []
  view
    .plugin(previewPlugin)
    ?.decorations.between(0, view.state.doc.length, (from, to, deco) => {
      if (to > from && !deco.spec.class && !deco.spec.widget) {
        found.push(view.state.sliceDoc(from, to))
      }
    })
  return found
}

const lineClasses = (view: EditorView, n: number) => [
  ...lineElement(view, n).classList,
]

describe('active lines', () => {
  const state = (doc: string, ranges: [number, number][], main = 0) =>
    EditorState.create({
      doc,
      selection: EditorSelection.create(
        ranges.map(([a, h]) => EditorSelection.range(a, h)),
        main,
      ),
      extensions: EditorState.allowMultipleSelections.of(true),
    })
  const doc = 'one\ntwo\nthree\nfour\nfive'

  it('is the cursor line for a single cursor', () => {
    expect(activeLines(state(doc, [[5, 5]]))).toEqual([[2, 2]])
  })

  it('covers every cursor', () => {
    expect(
      activeLines(
        state(doc, [
          [0, 0],
          [14, 14],
          [20, 20],
        ]),
      ),
    ).toEqual([
      [1, 1],
      [4, 5],
    ])
  })

  it('covers every line of a multi-line selection, either direction', () => {
    expect(activeLines(state(doc, [[1, 10]]))).toEqual([[1, 3]])
    expect(activeLines(state(doc, [[10, 1]]))).toEqual([[1, 3]])
  })

  it('leaves out a line the selection only ends at the start of', () => {
    // `one\ntwo\n` selected linewise ends at the start of line 3.
    expect(activeLines(state(doc, [[0, 8]]))).toEqual([[1, 2]])
  })
})

describe('hidden and dimmed markers', () => {
  it.each([
    ['Some **bold** text', 'Some bold text', 'cm-md-strong', '**bold**'],
    ['Some _em_ text', 'Some em text', 'cm-md-em', '_em_'],
    ['Some *em* text', 'Some em text', 'cm-md-em', '*em*'],
    ['Some ~~gone~~ text', 'Some gone text', 'cm-md-strike', '~~gone~~'],
    ['Run `make` now', 'Run make now', 'cm-md-code', '`make`'],
  ])(
    '%s renders as %s off the active line',
    async (source, display, cls, content) => {
      const { view } = await editorWith(`${source}\nend`)
      expect(shown(view, 1)).toBe(display)
      expect(marked(view, cls)).toEqual([content])
      expect(body(view)).toBe(`${source}\nend`)

      select(view, 0)
      expect(shown(view, 1)).toBe(source)
      // The content keeps its formatting while revealed.
      expect(marked(view, cls)).toEqual([content])
      expect(marked(view, 'cm-md-syntax').length).toBeGreaterThan(0)
    },
  )

  it('hides ATX heading markers and the space after them', async () => {
    const { view } = await editorWith('## Plans\n# Closed #\nend')
    expect(shown(view, 1)).toBe('Plans')
    expect(shown(view, 2)).toBe('Closed')
    expect(lineClasses(view, 1)).toContain('cm-md-h2')
    select(view, 0)
    expect(shown(view, 1)).toBe('## Plans')
    expect(lineClasses(view, 1)).toContain('cm-md-h2')
  })

  it('hides blockquote markers', async () => {
    const { view } = await editorWith('> quoted\nend')
    expect(shown(view, 1)).toBe('quoted')
    expect(lineClasses(view, 1)).toContain('cm-md-quote')
    select(view, 0)
    expect(shown(view, 1)).toBe('> quoted')
  })

  it('hides link brackets and URLs, and keeps the URL on the mark', async () => {
    const { view } = await editorWith(
      'See [the docs](https://example.com/docs)\nend',
    )
    expect(shown(view, 1)).toBe('See the docs')
    const link = lineElement(view, 1).querySelector<HTMLElement>('.cm-md-link')!
    expect(link.textContent).toBe('the docs')
    expect(link.dataset.url).toBe('https://example.com/docs')
    select(view, 0)
    expect(shown(view, 1)).toBe('See [the docs](https://example.com/docs)')
  })

  it('hides a link title along with the URL', async () => {
    const { view } = await editorWith('[a](https://a.example "Title")\nend')
    expect(shown(view, 1)).toBe('a')
  })

  it.each([
    ['[a](https://x.example\n"title")', 'a', ''],
    ['[a](\nhttps://x.example)', 'a', ''],
  ])(
    'hides a link split over lines, %j, line by line',
    async (source, first, second) => {
      const { view } = await editorWith(`${source}\n\nend`)
      expect(shown(view, 1)).toBe(first)
      expect(shown(view, 2)).toBe(second)
      select(view, view.state.doc.line(2).from)
      expect(shown(view, 2)).toBe(source.split('\n')[1])
      expect(shown(view, 1)).toBe(first)
    },
  )

  it('hides autolink angle brackets and marks bare URLs as links', async () => {
    const { view } = await editorWith(
      '<https://a.example> and https://b.example\nend',
    )
    expect(shown(view, 1)).toBe('https://a.example and https://b.example')
    expect(marked(view, 'cm-md-link')).toEqual([
      'https://a.example',
      'https://b.example',
    ])
  })

  it('hides syntax again when the cursor leaves the line', async () => {
    const { view } = await editorWith('[link](https://a.example)\nnext')
    select(view, 1)
    expect(shown(view, 1)).toBe('[link](https://a.example)')
    select(view, view.state.doc.line(2).from)
    expect(shown(view, 1)).toBe('link')
  })

  it('reveals every line a multi-line selection touches, and only those', async () => {
    const { view } = await editorWith('# One\n**two**\n_three_\n## Four')
    select(view, 0, view.state.doc.line(3).to)
    expect([1, 2, 3, 4].map((n) => shown(view, n))).toEqual([
      '# One',
      '**two**',
      '_three_',
      'Four',
    ])
  })

  it('never changes the text', async () => {
    const text =
      '# H\n**b** _i_ `c` [l](https://x.example)\n> q\n- [ ] t\n\n---\n'
    const { view, bridge, editor } = await editorWith(text)
    select(view, 0)
    select(view, view.state.doc.length)
    await editor.flush()
    expect(body(view)).toBe(text)
    expect(bridge.body('a')).toBe(text)
  })
})

describe('rendered elements', () => {
  it('gives ATX headings decreasing sizes', async () => {
    const { view } = await editorWith(
      '# One\n## Two\n### Three\n#### 4\n##### 5\n###### 6\nend',
    )
    for (let n = 1; n <= 6; n++)
      expect(lineClasses(view, n)).toContain(`cm-md-h${n}`)
  })

  it('renders setext headings and hides the underline off the active line', async () => {
    const { view } = await editorWith('Title\n=====\nSub\n---\nend')
    expect(lineClasses(view, 1)).toContain('cm-md-h1')
    expect(lineClasses(view, 3)).toContain('cm-md-h2')
    expect(shown(view, 2)).toBe('')
    expect(shown(view, 4)).toBe('')
    select(view, view.state.doc.line(2).from)
    expect(shown(view, 2)).toBe('=====')
    expect(shown(view, 4)).toBe('')
  })

  it('renders a thematic break as a rule', async () => {
    const { view } = await editorWith('above\n\n---\n\nbelow')
    expect(shown(view, 3)).toBe('—hr—')
    select(view, view.state.doc.line(3).from)
    expect(shown(view, 3)).toBe('---')
  })

  it('renders unordered list markers as bullets, ordered ones as text', async () => {
    const { view } = await editorWith('- milk\n* eggs\n1. first\nend')
    expect(shown(view, 1)).toBe('• milk')
    expect(shown(view, 2)).toBe('• eggs')
    expect(shown(view, 3)).toBe('1. first')
    select(view, 0)
    expect(shown(view, 1)).toBe('- milk')
  })

  it('keeps the number of an ordered task item', async () => {
    const { view } = await editorWith('1. [ ] call\n2. [x] done\nend')
    expect(shown(view, 1)).toBe('1. [ ] call')
    expect(shown(view, 2)).toBe('2. [✓] done')
  })

  it('renders task markers as checkboxes, and source on the active line', async () => {
    const { view } = await editorWith(
      '- [ ] buy milk\n- [x] done\n- [X] also\nend',
    )
    expect(shown(view, 1)).toBe('[ ] buy milk')
    expect(shown(view, 2)).toBe('[✓] done')
    expect(shown(view, 3)).toBe('[✓] also')
    select(view, view.state.doc.line(2).from)
    expect(shown(view, 2)).toBe('- [x] done')
  })
})

describe('unrendered markdown', () => {
  it('shows HTML as text and creates no element from it', async () => {
    const { view } = await editorWith(
      '<b onclick="x()">hi</b>\n<div>block</div>\nend',
    )
    expect(shown(view, 1)).toBe('<b onclick="x()">hi</b>')
    expect(shown(view, 2)).toBe('<div>block</div>')
    expect(
      view.contentDOM.querySelector('b, div:not(.cm-line), [onclick]'),
    ).toBeNull()
    expect(hidden(view)).toEqual([])
  })

  it('leaves images as source and loads nothing', async () => {
    const { view } = await editorWith(
      '![x](https://tracker.example/pixel.png)\nend',
    )
    expect(shown(view, 1)).toBe('![x](https://tracker.example/pixel.png)')
    expect(view.contentDOM.querySelector('img, iframe, link, [src]')).toBeNull()
    expect(hidden(view)).toEqual([])
    expect(marked(view, 'cm-md-link')).toEqual([])
  })

  it('leaves reference links, footnotes, and tables as source', async () => {
    const text =
      '[ref][r] and [^1]\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n[r]: https://x.example\nend'
    const { view } = await editorWith(text)
    expect(shown(view, 1)).toBe('[ref][r] and [^1]')
    expect(shown(view, 3)).toBe('| a | b |')
    expect(shown(view, 7)).toBe('[r]: https://x.example')
    expect(hidden(view)).toEqual([])
  })
})

describe('copy', () => {
  it('copies the markdown source of a rendered selection', () => {
    // A light-DOM editor: jsdom can't hold a DOM selection in a shadow
    // root, and CodeMirror only handles a copy whose DOM selection is in
    // the editor.
    const parent = document.createElement('div')
    document.body.append(parent)
    const view = new EditorView({
      parent,
      doc: 'See [the docs](https://example.com/docs) **now**\nend',
      selection: { anchor: 50 },
      extensions: [
        markdown({ base: markdownLanguage }),
        livePreview({ openExternal: () => {} }),
      ],
    })
    expect(shown(view, 1)).toBe('See the docs now')
    // The user selects the rendered line.
    view.focus()
    document.getSelection()!.selectAllChildren(lineElement(view, 1))
    // jsdom fires no `selectionchange`; have the editor read it now
    // (through internals the typings leave out).
    const { observer } = view as unknown as {
      observer: { readSelectionRange(): void; flush(): void }
    }
    observer.readSelectionRange()
    observer.flush()
    expect(view.state.selection.main.empty).toBe(false)
    const data = new Map<string, string>()
    const event = new Event('copy', { bubbles: true, cancelable: true })
    Object.defineProperty(event, 'clipboardData', {
      value: {
        clearData: () => data.clear(),
        setData: (type: string, value: string) => data.set(type, value),
      },
    })
    view.contentDOM.dispatchEvent(event)
    expect(data.get('text/plain')).toBe(
      'See [the docs](https://example.com/docs) **now**',
    )
    view.destroy()
    parent.remove()
  })
})

describe('tag pills', () => {
  it('renders single and multi-word tags as pills', async () => {
    const { view } = await editorWith(
      'Plan #work/acme and #side project# today\nend',
    )
    expect(marked(view, 'cm-tag-pill')).toEqual([
      '#work/acme',
      '#side project#',
    ])
  })

  it('does not render non-tags', async () => {
    const { view } = await editorWith(
      'Fixed #42 in `#define` at https://a.example/#top\n# Heading\nend',
    )
    expect(marked(view, 'cm-tag-pill')).toEqual([])
  })

  it('stays one editable pill when typed into', async () => {
    const { view } = await editorWith('#work and more')
    select(view, 5)
    type(view, 's')
    expect(body(view)).toBe('#works and more')
    expect(marked(view, 'cm-tag-pill')).toEqual(['#works'])
  })
})

describe('code blocks', () => {
  it.each([
    ['rust', 'fn main() {}'],
    ['nosuchlang', 'plain text'],
    ['', 'no language'],
  ])(
    'renders info string %j without error and hides the fences',
    async (info, code) => {
      const { view, errors } = await editorWith(
        `\`\`\`${info}\n${code}\n\`\`\`\nend`,
      )
      expect(shown(view, 1)).toBe('')
      expect(shown(view, 2)).toBe(code)
      expect(shown(view, 3)).toBe('')
      for (const n of [1, 2, 3])
        expect(lineClasses(view, n)).toContain('cm-md-codeblock')
      select(view, 0)
      expect(shown(view, 1)).toBe(`\`\`\`${info}`)
      expect(errors).toEqual([])
    },
  )

  it('formats nothing inside code', async () => {
    const { view } = await editorWith(
      '```\n**not bold** #nottag [x](https://a.example)\n```\nend',
    )
    expect(shown(view, 2)).toBe('**not bold** #nottag [x](https://a.example)')
    expect(marked(view, 'cm-md-strong')).toEqual([])
    expect(marked(view, 'cm-tag-pill')).toEqual([])
    expect(marked(view, 'cm-md-link')).toEqual([])
  })

  it('hides quote markers on every line of a quoted code block', async () => {
    const { view } = await editorWith('> ```js\n> let a = 1\n> ```\n\nend')
    expect([1, 2, 3].map((n) => shown(view, n))).toEqual(['', 'let a = 1', ''])
  })

  it('keeps an unclosed fence’s last line as code', async () => {
    const { view } = await editorWith('```\nopen\nstill code')
    select(view, 0)
    select(view, view.state.doc.length)
    expect(shown(view, 1)).toBe('')
    expect(shown(view, 3)).toBe('still code')
  })
})

describe('viewport', () => {
  it('decorates only the visible part of a 5,000-line note', async () => {
    const lines = Array.from(
      { length: 5000 },
      (_, i) => `Line **${i}** with #tag${i}`,
    )
    const { view } = await editorWith(lines.join('\n'))
    select(view, 0)
    const visible = view.visibleRanges
    const end = visible[visible.length - 1].to
    expect(end).toBeLessThan(view.state.doc.length / 2)
    let outside = 0
    let count = 0
    view
      .plugin(previewPlugin)!
      .decorations.between(0, view.state.doc.length, (from) => {
        count++
        if (from > end) outside++
      })
    expect(count).toBeGreaterThan(0)
    expect(outside).toBe(0)
  })

  it('rebuilds a 5,000-line note’s decorations quickly on cursor moves', async () => {
    const lines = Array.from(
      { length: 5000 },
      (_, i) => `- [ ] item **${i}** #t${i}`,
    )
    const { view } = await editorWith(lines.join('\n'))
    const start = performance.now()
    for (let i = 0; i < 50; i++)
      select(view, view.state.doc.line(1 + (i % 20)).from)
    const perMove = (performance.now() - start) / 50
    // Generous for slow CI; a full-document walk takes far longer.
    expect(perMove).toBeLessThan(25)
  })
})

describe('remote edits', () => {
  it('re-renders a remote change without moving the cursor or the reveal', async () => {
    const { view, bridge } = await editorWith('first *draft* line\nmy line')
    const cursor = view.state.doc.line(2).from + 3
    select(view, cursor)
    bridge.remoteEdit('a', (text) => {
      text.delete(6, 7)
      text.insert(6, '**final**')
    })
    await vi.waitFor(() =>
      expect(body(view)).toBe('first **final** line\nmy line'),
    )
    expect(shown(view, 1)).toBe('first final line')
    expect(marked(view, 'cm-md-strong')).toEqual(['**final**'])
    expect(view.state.selection.main.head).toBe(view.state.doc.line(2).from + 3)
    expect(shown(view, 2)).toBe('my line')
  })

  it('keeps only the cursor’s line revealed when lines are added above', async () => {
    const { view, bridge } = await editorWith('# Top\n**mine**')
    select(view, view.state.doc.line(2).from + 2)
    bridge.remoteEdit('a', (text) => text.insert(0, '## New\n**other**\n'))
    await vi.waitFor(() => expect(view.state.doc.lines).toBe(4))
    expect(view.state.doc.lineAt(view.state.selection.main.head).number).toBe(4)
    expect([1, 2, 3, 4].map((n) => shown(view, n))).toEqual([
      'New',
      'other',
      'Top',
      '**mine**',
    ])
  })
})

describe('cursor and active line', () => {
  /** Every CSS rule in the editor's shadow root, as text. */
  function rules(view: EditorView): string[] {
    const root = view.root as ShadowRoot
    const sheets = [
      ...(root.adoptedStyleSheets ?? []),
      ...[...root.querySelectorAll('style')].map((s) => s.sheet!),
    ]
    return sheets.flatMap((sheet) => [...sheet.cssRules].map((r) => r.cssText))
  }

  it('moves the cursor with no transition', async () => {
    const { view } = await editorWith('Hello')
    const cursor = rules(view).filter((r) => r.includes('.cm-cursor'))
    // The theme's own cursor rule is there, so the check isn't vacuous.
    expect(cursor.some((r) => r.includes('border-left-width: 2px'))).toBe(true)
    expect(cursor.filter((r) => r.includes('transition'))).toEqual([])
  })

  it('marks only the line holding a single cursor', async () => {
    const { view } = await editorWith('one\ntwo\nthree')
    select(view, view.state.doc.line(2).from + 1)
    const active = [...view.contentDOM.querySelectorAll('.cm-activeLine')]
    expect(active).toEqual([lineElement(view, 2)])
  })
})
