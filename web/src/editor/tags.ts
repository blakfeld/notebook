import { syntaxTree } from '@codemirror/language'
import type { Range } from '@codemirror/state'
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from '@codemirror/view'

/**
 * A port of the tag scanner in `notebook-core::tags::parse`, run one line at
 * a time (tags never span lines). It must agree with the Rust parser; the
 * shared cases in `crates/notebook-core/tests/fixtures/tag_cases.json` run
 * against both, and any change to the tag rules adds cases there.
 *
 * Offsets are UTF-16, like everything in the editor. Every character the
 * rules look at (whitespace, `#`, `/`, punctuation) is a single code unit.
 */

/** Unicode `White_Space`, which is what Rust's `char::is_whitespace` uses. */
const WHITESPACE =
  /[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/
const WHITESPACE_RUN = new RegExp(`${WHITESPACE.source}+`, 'g')
const TRAILING_PUNCTUATION = new Set([
  '.',
  ',',
  ';',
  ':',
  '!',
  '?',
  '"',
  "'",
  ')',
  ']',
  '}',
])

const isWhitespace = (c: string | undefined) =>
  c !== undefined && WHITESPACE.test(c)

/** Whether the character after a closing `#` lets it close a tag. */
const isBoundary = (next: string | undefined) =>
  next === undefined || isWhitespace(next) || TRAILING_PUNCTUATION.has(next)

/** Strips trailing punctuation, `/`, and whitespace from a raw name. */
function trimName(name: string): string {
  let end = name.length
  while (
    end > 0 &&
    (name[end - 1] === '/' ||
      isWhitespace(name[end - 1]) ||
      TRAILING_PUNCTUATION.has(name[end - 1]))
  ) {
    end--
  }
  return name.slice(0, end)
}

/** Whether a raw name normalizes to a tag path (`TagPath::from_raw`). */
function isTagName(raw: string): boolean {
  const segments = raw
    .split('/')
    .map((s) => s.replace(WHITESPACE_RUN, ' ').trim())
    .filter((s) => s.length > 0)
  return segments.length > 0 && !segments.every((s) => /^[0-9]+$/.test(s))
}

/**
 * Reads a tag starting at the `#` at `start`, looking no further than
 * `limit`. Returns the end of the tag, or `null` if there is none.
 */
function scanTag(line: string, start: number, limit: number): number | null {
  const rest = line.slice(start + 1, limit)
  const first = rest[0]
  if (first === undefined || isWhitespace(first) || first === '#') {
    return null
  }
  let wordEnd = 0
  while (
    wordEnd < rest.length &&
    !isWhitespace(rest[wordEnd]) &&
    rest[wordEnd] !== '#'
  ) {
    wordEnd++
  }
  const name = trimName(rest.slice(0, wordEnd))
  // A multi-word tag must start the way a single-word tag would.
  if (!isTagName(name)) {
    return null
  }

  const close = rest.indexOf('#')
  if (close >= 0) {
    const content = rest.slice(0, close)
    if (
      [...content].some(isWhitespace) &&
      !isWhitespace(content[content.length - 1]) &&
      isBoundary(rest[close + 1]) &&
      isTagName(trimName(content))
    ) {
      return start + 1 + close + 1
    }
  }

  const end = start + 1 + name.length
  if (
    name.length === wordEnd &&
    rest[wordEnd] === '#' &&
    isBoundary(rest[wordEnd + 1])
  ) {
    return end + 1
  }
  return end
}

/**
 * The tag occurrences in one line, as `[from, to)` offsets into it, from
 * the opening `#` through the name and any closing `#`. `excluded` lists
 * code ranges in the line, sorted and non-overlapping.
 */
export function findTagRanges(
  line: string,
  excluded: readonly (readonly [number, number])[] = [],
): [number, number][] {
  const found: [number, number][] = []
  let next = 0
  let i = 0
  while (i < line.length) {
    while (next < excluded.length && excluded[next][1] <= i) next++
    const code = excluded[next]
    if (code && code[0] <= i) {
      i = code[1]
      continue
    }
    // A line starts after a line break, which is whitespace.
    if (line[i] === '#' && (i === 0 || isWhitespace(line[i - 1]))) {
      const limit = code ? Math.min(code[0], line.length) : line.length
      const end = scanTag(line, i, limit)
      if (end !== null) {
        found.push([i, end])
        i = end
        continue
      }
    }
    i++
  }
  return found
}

const CODE_NODES = new Set(['InlineCode', 'FencedCode', 'CodeBlock'])

const tagMark = Decoration.mark({ class: 'cm-tag' })

function tagDecorations(view: EditorView): DecorationSet {
  const { state } = view
  const tree = syntaxTree(state)
  const marks: Range<Decoration>[] = []
  let lastLine = 0
  for (const visible of view.visibleRanges) {
    let pos = visible.from
    while (pos <= visible.to) {
      const line = state.doc.lineAt(pos)
      pos = line.to + 1
      if (line.number <= lastLine) continue
      lastLine = line.number
      const excluded: [number, number][] = []
      tree.iterate({
        from: line.from,
        to: line.to,
        enter(node) {
          if (!CODE_NODES.has(node.name)) return
          const from = Math.max(node.from, line.from) - line.from
          const to = Math.min(node.to, line.to) - line.from
          if (to > from) excluded.push([from, to])
          return false
        },
      })
      for (const [from, to] of findTagRanges(line.text, excluded)) {
        marks.push(tagMark.range(line.from + from, line.from + to))
      }
    }
  }
  return Decoration.set(marks)
}

/** Marks every tag occurrence in the visible lines with `cm-tag`. */
export const tagHighlighter = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet

    constructor(view: EditorView) {
      this.decorations = tagDecorations(view)
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.viewportChanged ||
        syntaxTree(update.startState) !== syntaxTree(update.state)
      ) {
        this.decorations = tagDecorations(update.view)
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
)

export const tagTheme = EditorView.baseTheme({
  '.cm-tag': { color: '#0969da' },
  '&dark .cm-tag': { color: '#4493f8' },
})
