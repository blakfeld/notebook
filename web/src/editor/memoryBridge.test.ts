// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { memoryBridge } from './memoryBridge'

/** An editor-side replica attached through `bridge`. */
async function attach(bridge: ReturnType<typeof memoryBridge>, noteId: string) {
  const open = await bridge.open(noteId)
  const doc = new Y.Doc()
  Y.applyUpdate(doc, open.initialState)
  open.onRemote((update) => Y.applyUpdate(doc, update, 'remote'))
  return { open, doc, text: doc.getText('body') }
}

describe('memoryBridge', () => {
  it('opens with the note body', async () => {
    const bridge = memoryBridge({ a: 'hello' })
    const { text } = await attach(bridge, 'a')
    expect(text.toString()).toBe('hello')
    expect(bridge.attached('a')).toBe(1)
  })

  it('takes pushes and delivers remote edits, converging', async () => {
    const bridge = memoryBridge({ a: 'one\ntwo' })
    const editor = await attach(bridge, 'a')
    const updates: Uint8Array[] = []
    editor.doc.on('update', (u: Uint8Array, origin: unknown) => {
      if (origin !== 'remote') updates.push(u)
    })
    editor.text.delete(0, 3)
    editor.text.insert(0, 'ONE')
    bridge.remoteEdit('a', (body) => {
      body.delete(4, 3)
      body.insert(4, 'TWO')
    })
    for (const u of updates) await editor.open.push(u)
    expect(bridge.body('a')).toBe('ONE\nTWO')
    expect(editor.text.toString()).toBe('ONE\nTWO')
    expect(bridge.pushes).toBe(2)
  })

  it('does not echo an editor its own pushes', async () => {
    const bridge = memoryBridge({ a: 'x' })
    const editor = await attach(bridge, 'a')
    const other = await attach(bridge, 'a')
    const seen: Uint8Array[] = []
    editor.open.onRemote((u) => seen.push(u))
    editor.text.insert(1, 'y')
    await editor.open.push(Y.encodeStateAsUpdate(editor.doc))
    expect(seen).toHaveLength(0)
    expect(other.text.toString()).toBe('xy')
  })

  it('can fail pushes', async () => {
    const bridge = memoryBridge({ a: 'x' })
    const { open, doc, text } = await attach(bridge, 'a')
    text.insert(1, 'y')
    bridge.failPushes = true
    await expect(open.push(Y.encodeStateAsUpdate(doc))).rejects.toThrow()
    expect(bridge.body('a')).toBe('x')
    bridge.failPushes = false
    await open.push(Y.encodeStateAsUpdate(doc))
    expect(bridge.body('a')).toBe('xy')
  })

  it('stops delivering after close', async () => {
    const bridge = memoryBridge({ a: 'x' })
    const { open, text } = await attach(bridge, 'a')
    await open.close()
    expect(bridge.attached('a')).toBe(0)
    bridge.remoteEdit('a', (body) => body.insert(0, '!'))
    expect(text.toString()).toBe('x')
    await expect(open.push(new Uint8Array([0, 0]))).rejects.toThrow()
  })

  it('reads and writes only the body root', async () => {
    const bridge = memoryBridge({ a: 'x' })
    const { open, doc, text } = await attach(bridge, 'a')
    text.insert(1, 'y')
    await open.push(Y.encodeStateAsUpdate(doc))
    bridge.remoteEdit('a', (body) => body.insert(0, '!'))
    expect([...bridge.doc('a').share.keys()]).toEqual(['body'])
    expect([...doc.share.keys()]).toEqual(['body'])
  })

  it('rejects unknown notes', async () => {
    await expect(memoryBridge().open('missing')).rejects.toThrow()
  })
})
