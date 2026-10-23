import type { Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'

const isMac =
  typeof navigator !== 'undefined' &&
  /Mac|iPhone|iPad|iPod/.test(navigator.platform)

/** The URL of the rendered link at `target`, if any. */
function linkAt(target: EventTarget | null): string | undefined {
  if (!(target instanceof Element)) return undefined
  return target.closest<HTMLElement>('.cm-md-link')?.dataset.url
}

/** Whether Mod (Cmd on Apple platforms, Ctrl elsewhere) is held. */
export const modHeld = (event: MouseEvent) =>
  isMac ? event.metaKey : event.ctrlKey

/**
 * Opens rendered links outside the app on Mod-click. A plain click just
 * places the cursor. `open` decides whether the URL may open (see `platform/external.ts`).
 */
export function linkHandlers(open: (url: string) => void): Extension {
  return EditorView.domEventHandlers({
    mousedown(event) {
      if (event.button !== 0 || !modHeld(event)) return false
      const url = linkAt(event.target)
      if (url === undefined) return false
      // Mod-click would otherwise add a cursor.
      event.preventDefault()
      open(url)
      return true
    },
  })
}
