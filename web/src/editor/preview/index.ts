import type { Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { previewPlugin } from './decorations'
import { linkHandlers } from './links'
import { previewTheme } from './theme'

export { activeLines } from './activeLines'
export { previewDecorations, previewPlugin } from './decorations'
export { focusedBlock, focusMode } from './focus'
export { toggleTask } from './tasks'
export { editorTheme, noteHighlightStyle } from './theme'

export interface LivePreviewOptions {
  /** Opens a link's URL outside the app (it checks the scheme). */
  openExternal: (url: string) => void
}

/**
 * Live preview: markdown rendered in place over the unchanged text. The
 * editor keeps it in a compartment, so source view is the same editor
 * with this left out.
 */
export function livePreview({ openExternal }: LivePreviewOptions): Extension {
  return [
    previewPlugin,
    linkHandlers(openExternal),
    previewTheme,
    EditorView.editorAttributes.of({ class: 'cm-live-preview' }),
  ]
}
