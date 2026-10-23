// Helpers for the editor's jsdom tests. Not part of the editor itself.
import { EditorSelection } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'
import { memorySettings } from '../platform/settings'
import { memoryBridge } from './memoryBridge'
import { createNoteEditor, type NoteEditorOptions } from './noteEditor'

// jsdom has no layout. CodeMirror measures ranges, which jsdom lacks.
if (typeof Range !== 'undefined' && !Range.prototype.getClientRects) {
  const rect = () => new DOMRect(0, 0, 0, 0)
  Range.prototype.getBoundingClientRect = rect
  Range.prototype.getClientRects = () =>
    Object.assign([], { item: () => null }) as unknown as DOMRectList
}
// jsdom reports Apple's vendor string, so CodeMirror applies a Safari
// selection workaround on pointer selection that needs `execCommand`.
if (typeof document !== 'undefined' && !document.execCommand) {
  document.execCommand = () => false
}

/** An editor on a memory bridge, with `notes` and nothing open yet. */
export function setup(
  notes: Record<string, string> = {},
  options: Partial<NoteEditorOptions> = {},
) {
  const bridge = memoryBridge(notes)
  const settings = memorySettings()
  const errors: unknown[] = []
  const parent = document.createElement('div')
  document.body.append(parent)
  const editor = createNoteEditor(parent, {
    bridge,
    settings,
    onError: (error) => errors.push(error),
    ...options,
  })
  return { bridge, settings, errors, parent, editor, view: editor.view }
}

/** Types `text` at the cursor, as a keyboard would. */
export function type(view: EditorView, text: string) {
  view.dispatch(view.state.replaceSelection(text), { userEvent: 'input.type' })
}

export function select(view: EditorView, anchor: number, head = anchor) {
  view.dispatch({ selection: EditorSelection.single(anchor, head) })
}

export function cursorAtEnd(view: EditorView) {
  select(view, view.state.doc.length)
}

/** Presses `key` in the editor, through its keymaps. */
export function press(
  view: EditorView,
  key: string,
  init: KeyboardEventInit = {},
) {
  view.contentDOM.dispatchEvent(
    new KeyboardEvent('keydown', {
      key,
      bubbles: true,
      cancelable: true,
      ...init,
    }),
  )
}

export const body = (view: EditorView) => view.state.doc.toString()
