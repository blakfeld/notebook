// @vitest-environment jsdom
import { EditorView } from '@codemirror/view'
import { getCM } from '@replit/codemirror-vim'
import { flushSync, mount, unmount } from 'svelte'
import { describe, expect, it, vi } from 'vitest'
import { memorySettings } from '../platform/settings'
import { memoryBridge } from './memoryBridge'
import NoteEditor from './NoteEditor.svelte'
import { type NoteEditorHandle, VIM_MODE_KEY } from './noteEditor'
import './testing'

describe('NoteEditor', () => {
  it('mounts, switches notes, and toggles vim', async () => {
    const bridge = memoryBridge({ a: 'note a', b: 'note b' })
    const target = document.createElement('div')
    document.body.append(target)
    const props = $state({
      bridge,
      settings: memorySettings(),
      noteId: 'a' as string | null,
      vimMode: false,
      readOnly: false,
    })
    const component = mount(NoteEditor, { target, props })
    const view = () =>
      EditorView.findFromDOM(
        target
          .querySelector('.note-editor-host')!
          .shadowRoot!.querySelector<HTMLElement>('.cm-editor')!,
      )!

    await vi.waitFor(() => expect(view().state.doc.toString()).toBe('note a'))
    expect(bridge.attached('a')).toBe(1)

    props.noteId = 'b'
    flushSync()
    await vi.waitFor(() => expect(view().state.doc.toString()).toBe('note b'))
    expect(bridge.attached('a')).toBe(0)

    expect(getCM(view())).toBeNull()
    props.vimMode = true
    flushSync()
    expect(getCM(view())).not.toBeNull()

    props.readOnly = true
    flushSync()
    expect(view().state.readOnly).toBe(true)

    unmount(component)
    await vi.waitFor(() => expect(bridge.attached('b')).toBe(0))
  })

  it('keeps the saved vim mode until the prop changes', async () => {
    const settings = memorySettings()
    await settings.set(VIM_MODE_KEY, 'true')
    const target = document.createElement('div')
    document.body.append(target)
    const props = $state({
      bridge: memoryBridge({ a: 'x' }),
      settings,
      noteId: 'a' as string | null,
      vimMode: false,
      handle: null as NoteEditorHandle | null,
    })
    const component = mount(NoteEditor, { target, props })
    await vi.waitFor(() => expect(props.handle?.vimMode).toBe(true))
    expect(await settings.get(VIM_MODE_KEY)).toBe('true')

    props.vimMode = true
    flushSync()
    props.vimMode = false
    flushSync()
    expect(props.handle!.vimMode).toBe(false)
    expect(await settings.get(VIM_MODE_KEY)).toBe('false')
    unmount(component)
  })
})
