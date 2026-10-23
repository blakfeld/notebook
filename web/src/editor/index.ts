export { default as NoteEditor } from './NoteEditor.svelte'
export type { NoteDocBridge, OpenNoteDoc } from './bridge'
export { memoryBridge, type MemoryBridge } from './memoryBridge'
export {
  createNoteEditor,
  VIM_MODE_KEY,
  type NoteEditorHandle,
  type NoteEditorOptions,
  type ViewOption,
} from './noteEditor'
export { findTagRanges } from './tags'
