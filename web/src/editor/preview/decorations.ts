import { syntaxTree } from '@codemirror/language'
import type { Range } from '@codemirror/state'
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from '@codemirror/view'
import type { SyntaxNode } from '@lezer/common'
import { visibleTagRanges } from '../tags'
import { activeLines, isActiveLine } from './activeLines'
import { BulletWidget, CheckboxWidget, RuleWidget } from './widgets'

const hidden = Decoration.replace({})
const syntax = Decoration.mark({ class: 'cm-md-syntax' })
const bullet = Decoration.replace({ widget: new BulletWidget() })
const rule = Decoration.replace({ widget: new RuleWidget() })
const checked = Decoration.replace({ widget: new CheckboxWidget(true) })
const unchecked = Decoration.replace({ widget: new CheckboxWidget(false) })
const pill = Decoration.mark({ class: 'cm-tag-pill' })

const markOf = (cls: string) => Decoration.mark({ class: cls })
const lineOf = (cls: string) => Decoration.line({ class: cls })

/** Styling for the content between markers, kept on revealed lines too. */
const CONTENT: Record<string, Decoration> = {
  Emphasis: markOf('cm-md-em'),
  StrongEmphasis: markOf('cm-md-strong'),
  Strikethrough: markOf('cm-md-strike'),
}
/** Markers that are hidden off the active lines and dimmed on them. */
const MARKERS = new Set(['EmphasisMark', 'StrikethroughMark'])
/** Left as source: nothing inside is formatted. */
const SOURCE_ONLY = new Set([
  'Image',
  'HTMLBlock',
  'HTMLTag',
  'Comment',
  'CommentBlock',
  'ProcessingInstruction',
  'ProcessingInstructionBlock',
  'Table',
  'LinkReference',
])
const inlineCode = markOf('cm-md-code')
const codeBlockLine = lineOf('cm-md-codeblock')
const fenceLine = lineOf('cm-md-codeblock cm-md-fence')
const quoteLine = lineOf('cm-md-quote')
const setextMarkLine = lineOf('cm-md-setext-mark')
const headingLines = [1, 2, 3, 4, 5, 6].map((n) => lineOf(`cm-md-h${n}`))

/**
 * The URL a link's source text points to: angle brackets around a
 * destination dropped, and a GFM `www.` autolink read as https, as GFM does.
 */
function linkTarget(source: string): string {
  const url =
    source.startsWith('<') && source.endsWith('>')
      ? source.slice(1, -1)
      : source
  return /^www\./i.test(url) ? `https://${url}` : url
}

const linkMark = (source: string) =>
  Decoration.mark({
    class: 'cm-md-link',
    attributes: { 'data-url': linkTarget(source) },
  })

/** The decorations live preview draws for the visible part of the note. */
export function previewDecorations(view: EditorView): DecorationSet {
  const { state } = view
  const { doc } = state
  const tree = syntaxTree(state)
  const active = activeLines(state)
  const out: Range<Decoration>[] = []
  if (view.visibleRanges.length === 0) return Decoration.none
  // One walk over the whole visible span. Without folding there is only
  // one visible range anyway, and one walk visits each node once.
  const from = view.visibleRanges[0].from
  const to = view.visibleRanges[view.visibleRanges.length - 1].to

  const lineActive = (pos: number) =>
    isActiveLine(active, doc.lineAt(pos).number)
  /**
   * Hides a marker, or dims it on a revealed line. A marker can span lines
   * (a link's title on the next line); a plugin may not hide a line break,
   * so each line's part is handled on its own.
   */
  const marker = (start: number, end: number) => {
    while (start < end) {
      const line = doc.lineAt(start)
      const stop = Math.min(end, line.to)
      if (stop > start) {
        out.push((lineActive(start) ? syntax : hidden).range(start, stop))
      }
      start = line.to + 1
    }
  }
  /** Adds `deco` to every line of `[start, end]` that is in view. */
  const eachLine = (
    start: number,
    end: number,
    deco: (n: number) => Decoration | null,
  ) => {
    const first = doc.lineAt(Math.max(start, from)).number
    const last = doc.lineAt(Math.min(end, to)).number
    for (let n = first; n <= last; n++) {
      const line = doc.line(n)
      const value = deco(n)
      if (value) out.push(value.range(line.from))
    }
  }
  const spaceAfter = (pos: number) =>
    doc.sliceString(pos, pos + 1) === ' ' ? 1 : 0

  const codeBlock = (node: SyntaxNode) => {
    const firstLine = doc.lineAt(node.from).number
    const marks = node.getChildren('CodeMark')
    const closing =
      marks.length > 1 ? doc.lineAt(marks[marks.length - 1].from).number : -1
    const fences = node.name === 'FencedCode' ? [firstLine, closing] : []
    eachLine(node.from, node.to, (n) =>
      fences.includes(n) ? fenceLine : codeBlockLine,
    )
    for (const n of fences) {
      if (n < 0) continue
      const line = doc.line(n)
      if (line.to < from || line.from > to) continue
      if (isActiveLine(active, n)) {
        for (const child of [...marks, ...node.getChildren('CodeInfo')]) {
          if (doc.lineAt(child.from).number === n)
            out.push(syntax.range(child.from, child.to))
        }
      } else if (line.to > line.from) {
        out.push(hidden.range(line.from, line.to))
      }
    }
    // A code block in a blockquote: its quote markers, as elsewhere.
    for (const quote of node.getChildren('QuoteMark')) {
      if (!fences.includes(doc.lineAt(quote.from).number)) {
        marker(quote.from, quote.to + spaceAfter(quote.to))
      }
    }
  }

  const link = (node: SyntaxNode) => {
    const marks = node.getChildren('LinkMark')
    // Only inline links: `[text](url)`. Reference links stay as source.
    const open = marks.find((m) => doc.sliceString(m.from, m.to) === '(')
    // The destination, not a URL inside the link text.
    const url = open && node.getChildren('URL').find((u) => u.from > open.from)
    if (!url || !open || marks.length < 2) return
    const textEnd = marks[1].from
    if (textEnd > marks[0].to) {
      out.push(
        linkMark(doc.sliceString(url.from, url.to)).range(marks[0].to, textEnd),
      )
    }
    marker(marks[0].from, marks[0].to)
    marker(textEnd, node.to)
  }

  const headerMark = (node: SyntaxNode) => {
    const heading = node.parent
    if (!heading) return
    if (heading.name.startsWith('SetextHeading')) {
      if (lineActive(node.from)) {
        out.push(syntax.range(node.from, node.to))
      } else {
        out.push(hidden.range(node.from, node.to))
        out.push(setextMarkLine.range(doc.lineAt(node.from).from))
      }
    } else if (node.from === heading.from) {
      // Opening `##` and the space after it.
      marker(node.from, node.to + spaceAfter(node.to))
    } else {
      // A closing `##` and the space before it.
      const before = doc.sliceString(node.from - 1, node.from) === ' ' ? 1 : 0
      marker(node.from - before, node.to)
    }
  }

  tree.iterate({
    from,
    to,
    enter: (ref) => {
      const { name } = ref
      const node = ref.node
      if (SOURCE_ONLY.has(name)) return false
      if (name === 'FencedCode' || name === 'CodeBlock') {
        codeBlock(node)
        return false
      }
      if (name === 'InlineCode') {
        out.push(inlineCode.range(node.from, node.to))
        for (const mark of node.getChildren('CodeMark'))
          marker(mark.from, mark.to)
        return false
      }
      const atx = /^ATXHeading([1-6])$/.exec(name)
      if (atx) {
        out.push(
          headingLines[Number(atx[1]) - 1].range(doc.lineAt(node.from).from),
        )
        return
      }
      const setext = /^SetextHeading([12])$/.exec(name)
      if (setext) {
        const underline = node.getChild('HeaderMark')
        const end = underline ? doc.lineAt(underline.from).from - 1 : node.to
        eachLine(node.from, end, () => headingLines[Number(setext[1]) - 1])
        return
      }
      if (name === 'HeaderMark') return headerMark(node)
      if (CONTENT[name]) {
        out.push(CONTENT[name].range(node.from, node.to))
        return
      }
      if (MARKERS.has(name)) return marker(node.from, node.to)
      if (name === 'Link') return link(node)
      if (name === 'Autolink') {
        const url = node.getChild('URL')
        if (url)
          out.push(
            linkMark(doc.sliceString(url.from, url.to)).range(url.from, url.to),
          )
        for (const mark of node.getChildren('LinkMark'))
          marker(mark.from, mark.to)
        return false
      }
      if (name === 'URL') {
        // A bare URL; those in links and autolinks are handled above.
        if (node.parent?.name !== 'Link') {
          out.push(
            linkMark(doc.sliceString(node.from, node.to)).range(
              node.from,
              node.to,
            ),
          )
        }
        return
      }
      if (name === 'Blockquote') {
        eachLine(node.from, node.to, () => quoteLine)
        return
      }
      if (name === 'QuoteMark')
        return marker(node.from, node.to + spaceAfter(node.to))
      if (name === 'ListMark') {
        if (lineActive(node.from)) return
        // Ordered markers stay as text, task or not.
        if (node.parent?.parent?.name !== 'BulletList') return
        if (node.nextSibling?.name === 'Task') {
          out.push(hidden.range(node.from, node.to + spaceAfter(node.to)))
        } else {
          out.push(bullet.range(node.from, node.to))
        }
        return
      }
      if (name === 'TaskMarker') {
        if (!lineActive(node.from)) {
          const isChecked =
            doc.sliceString(node.from + 1, node.from + 2) !== ' '
          out.push((isChecked ? checked : unchecked).range(node.from, node.to))
        }
        return
      }
      if (name === 'HorizontalRule') {
        if (!lineActive(node.from)) out.push(rule.range(node.from, node.to))
        return false
      }
    },
  })

  for (const [start, end] of visibleTagRanges(view))
    out.push(pill.range(start, end))
  return Decoration.set(out, true)
}

/** Draws live preview over the visible lines, following the selection. */
export const previewPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet

    constructor(view: EditorView) {
      this.decorations = previewDecorations(view)
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged ||
        syntaxTree(update.startState) !== syntaxTree(update.state)
      ) {
        this.decorations = previewDecorations(update.view)
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
)
