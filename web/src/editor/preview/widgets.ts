import { EditorView, WidgetType } from '@codemirror/view'

/** Stands in for an unordered list marker (`-`, `*`, `+`). */
export class BulletWidget extends WidgetType {
  eq() {
    return true
  }

  toDOM() {
    const span = document.createElement('span')
    span.className = 'cm-md-bullet'
    span.setAttribute('aria-hidden', 'true')
    span.textContent = '•'
    return span
  }
}

/** Stands in for a thematic break (`---`). */
export class RuleWidget extends WidgetType {
  eq() {
    return true
  }

  toDOM() {
    const span = document.createElement('span')
    span.className = 'cm-md-hr'
    span.setAttribute('aria-hidden', 'true')
    return span
  }
}

/**
 * Flips the task marker (`[ ]`, `[x]`, or `[X]`) starting at `pos`, as an
 * ordinary local edit: it syncs and undoes like typing. Returns whether it
 * did.
 */
export function toggleTaskMarker(view: EditorView, pos: number): boolean {
  if (view.state.readOnly) return false
  const marker = view.state.sliceDoc(pos, pos + 3)
  if (!/^\[[ xX]\]$/.test(marker)) return false
  view.dispatch({
    changes: {
      from: pos + 1,
      to: pos + 2,
      insert: marker[1] === ' ' ? 'x' : ' ',
    },
    userEvent: 'input.toggle-task',
  })
  return true
}

/**
 * Stands in for a task marker. Clicking it toggles the marker
 * without moving the cursor; the editor ignores events on it.
 */
export class CheckboxWidget extends WidgetType {
  constructor(readonly checked: boolean) {
    super()
  }

  eq(other: CheckboxWidget) {
    return other.checked === this.checked
  }

  toDOM(view: EditorView) {
    const box = document.createElement('input')
    box.type = 'checkbox'
    box.className = 'cm-md-task'
    box.checked = this.checked
    box.setAttribute('aria-label', 'Done')
    // Keeps focus and the selection where they are.
    box.addEventListener('pointerdown', (event) => event.preventDefault())
    box.addEventListener('mousedown', (event) => event.preventDefault())
    box.addEventListener('click', (event) => {
      // The edit re-renders the box; the browser mustn't flip it first.
      event.preventDefault()
      toggleTaskMarker(view, view.posAtDOM(box))
    })
    return box
  }
}
