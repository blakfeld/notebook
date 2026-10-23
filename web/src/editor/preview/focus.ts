import { syntaxTree } from '@codemirror/language'
import type { EditorState, Range } from '@codemirror/state'
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from '@codemirror/view'

const dimmed = Decoration.line({ class: 'cm-dimmed' })
const LISTS = new Set(['BulletList', 'OrderedList'])

/**
 * The top-level block holding the main cursor, as `[from, to]`: a direct
 * child of the document, or the top-level item of a list. `null` when the
 * cursor is between blocks.
 */
export function focusedBlock(state: EditorState): [number, number] | null {
  const pos = state.selection.main.head
  let block = syntaxTree(state).topNode.firstChild
  while (block && !(block.from <= pos && pos <= block.to))
    block = block.nextSibling
  if (!block) return null
  if (LISTS.has(block.name)) {
    let item = block.firstChild
    while (item && !(item.from <= pos && pos <= item.to))
      item = item.nextSibling
    if (item) return [item.from, item.to]
  }
  return [block.from, block.to]
}

function focusDecorations(view: EditorView): DecorationSet {
  const { doc } = view.state
  const block = focusedBlock(view.state)
  const first = block ? doc.lineAt(block[0]).number : 0
  const last = block ? doc.lineAt(block[1]).number : -1
  const out: Range<Decoration>[] = []
  for (const { from, to } of view.visibleRanges) {
    for (let n = doc.lineAt(from).number; n <= doc.lineAt(to).number; n++) {
      if (n < first || n > last) out.push(dimmed.range(doc.line(n).from))
    }
  }
  return Decoration.set(out, true)
}

/** Focus mode: dims every visible line outside the cursor's block. */
export const focusMode = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet

    constructor(view: EditorView) {
      this.decorations = focusDecorations(view)
    }

    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.viewportChanged ||
        syntaxTree(update.startState) !== syntaxTree(update.state)
      ) {
        this.decorations = focusDecorations(update.view)
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
)
