import { syntaxTree } from '@codemirror/language'
import type { ChangeSpec, EditorState, StateCommand } from '@codemirror/state'
import type { SyntaxNode } from '@lezer/common'
import { touchedLines } from './activeLines'

/** A list item with a task marker: group 1 runs up to the `[`. */
const TASK = /^(\s*(?:>\s?)*\s*(?:[-*+]|\d{1,9}[.)])[ \t]+)\[([ xX])\](?=\s|$)/
/** A list item: group 1 is the indent and marker, group 2 the space after. */
const ITEM = /^(\s*(?:>\s?)*\s*(?:[-*+]|\d{1,9}[.)]))([ \t]+|$)/

/**
 * Whether the parser sees a list item marker on this line, outside code.
 * The patterns below then only find where the marker and task box are.
 */
function startsListItem(state: EditorState, from: number, text: string) {
  const item = ITEM.exec(text)
  if (!item) return false
  // The marker is the last non-space character of group 1.
  const at = from + item[1].length - 1
  for (
    let node: SyntaxNode | null = syntaxTree(state).resolveInner(at, 1);
    node;
    node = node.parent
  ) {
    if (node.name === 'ListMark') return true
    if (node.name === 'FencedCode' || node.name === 'CodeBlock') return false
  }
  return false
}

/**
 * Toggles the task on every line the selection touches: `[ ]` and `[x]`
 * swap, and a list item without a marker gets `[ ] `. Other lines are left
 * alone. Does nothing (and lets the key through) when no line is a list
 * item. Available in live preview and source view alike.
 */
export const toggleTask: StateCommand = ({ state, dispatch }) => {
  if (state.readOnly) return false
  const changes: ChangeSpec[] = []
  const seen = new Set<number>()
  for (const range of state.selection.ranges) {
    const [first, last] = touchedLines(state.doc, range)
    for (let n = first; n <= last; n++) {
      if (seen.has(n)) continue
      seen.add(n)
      const line = state.doc.line(n)
      if (!startsListItem(state, line.from, line.text)) continue
      const task = TASK.exec(line.text)
      if (task) {
        const at = line.from + task[1].length + 1
        changes.push({
          from: at,
          to: at + 1,
          insert: task[2] === ' ' ? 'x' : ' ',
        })
        continue
      }
      const item = ITEM.exec(line.text)
      if (item) {
        const at = line.from + item[1].length
        changes.push(
          item[2]
            ? { from: at + item[2].length, insert: '[ ] ' }
            : { from: at, insert: ' [ ] ' },
        )
      }
    }
  }
  if (changes.length === 0) return false
  dispatch(state.update({ changes, userEvent: 'input.toggle-task' }))
  return true
}
