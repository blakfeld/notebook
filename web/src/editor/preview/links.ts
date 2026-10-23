import type { Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { activeLines, isActiveLine } from './activeLines'

/** A touch that moves further than this is a scroll or a drag, not a tap. */
const TAP_SLOP_PX = 10

const isMac =
  typeof navigator !== 'undefined' &&
  /Mac|iPhone|iPad|iPod/.test(navigator.platform)

/** The URL of the rendered link at `target`, if any. */
function linkAt(
  target: EventTarget | null,
): { el: HTMLElement; url: string } | null {
  if (!(target instanceof Element)) return null
  const el = target.closest<HTMLElement>('.cm-md-link')
  const url = el?.dataset.url
  return el && url !== undefined ? { el, url } : null
}

/** Whether Mod (Cmd on Apple platforms, Ctrl elsewhere) is held. */
export const modHeld = (event: MouseEvent) =>
  isMac ? event.metaKey : event.ctrlKey

/**
 * Opens rendered links outside the app: Mod-click anywhere, or a tap on a
 * line no selection touches. A plain click just places the cursor. `open`
 * decides whether the URL may open (see `platform/external.ts`).
 */
export function linkHandlers(open: (url: string) => void): Extension {
  let tap: { x: number; y: number } | null = null
  return EditorView.domEventHandlers({
    mousedown(event) {
      if (event.button !== 0 || !modHeld(event)) return false
      const link = linkAt(event.target)
      if (!link) return false
      // Mod-click would otherwise add a cursor.
      event.preventDefault()
      open(link.url)
      return true
    },
    touchstart(event) {
      const touch = event.touches.length === 1 ? event.touches[0] : null
      tap = touch ? { x: touch.clientX, y: touch.clientY } : null
      return false
    },
    touchmove(event) {
      const touch = event.touches[0]
      if (
        tap &&
        touch &&
        Math.hypot(touch.clientX - tap.x, touch.clientY - tap.y) > TAP_SLOP_PX
      ) {
        tap = null
      }
      return false
    },
    touchend(event, view) {
      const started = tap
      tap = null
      if (!started) return false
      const link = linkAt(event.target)
      if (!link) return false
      const line = view.state.doc.lineAt(view.posAtDOM(link.el)).number
      // On a revealed line a tap edits, like a click.
      if (isActiveLine(activeLines(view.state), line)) return false
      event.preventDefault()
      open(link.url)
      return true
    },
  })
}
