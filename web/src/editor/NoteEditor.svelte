<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import type { DeviceSettings } from '../platform/settings'
  import type { NoteDocBridge } from './bridge'
  import { createNoteEditor, type NoteEditorHandle } from './noteEditor'

  interface Props {
    bridge: NoteDocBridge
    settings: DeviceSettings
    /** The note to show, or `null` for none. */
    noteId: string | null
    /**
     * Changes after mount turn vim mode on or off and save the choice on
     * this device. The initial value is ignored, so the saved one applies.
     */
    vimMode?: boolean
    readOnly?: boolean
    onError?: (error: unknown) => void
    /** The editor, once mounted, for toolbars and shortcuts. */
    handle?: NoteEditorHandle | null
  }

  let {
    bridge,
    settings,
    noteId,
    vimMode,
    readOnly = false,
    onError,
    handle = $bindable(null),
  }: Props = $props()

  let container: HTMLDivElement

  onMount(() => {
    const editor = createNoteEditor(container, {
      bridge,
      settings,
      onError: (error) => onError?.(error),
    })
    handle = editor
    return () => {
      handle = null
      void editor.destroy()
    }
  })

  $effect(() => {
    handle?.openNote(noteId).catch((error) => onError?.(error))
  })

  let appliedVimMode = untrack(() => vimMode)
  $effect(() => {
    if (!handle || vimMode === appliedVimMode) return
    appliedVimMode = vimMode
    if (vimMode !== undefined) handle.setVimMode(vimMode)
  })

  $effect(() => {
    handle?.setReadOnly(readOnly)
  })
</script>

<div class="note-editor" bind:this={container}></div>

<style>
  .note-editor {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }

  .note-editor :global(.note-editor-host) {
    flex: 1;
    min-height: 0;
  }
</style>
