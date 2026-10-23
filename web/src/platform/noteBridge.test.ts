import { describe, expect, it } from 'vitest'
import * as Y from 'yjs'
import { NoteClosedError } from '../editor/bridge'
import { fromBase64, toBase64 } from './base64'
import { FakeCoreApi } from './fake'
import { coreBridge } from './noteBridge'

function unlockedCore(notes: { id: string; body: string }[]) {
  const core = new FakeCoreApi(notes)
  core.phase = {
    phase: 'unlocked',
    account: null,
    credentials_remembered: true,
  }
  return core
}

describe('base64', () => {
  it('round-trips bytes', () => {
    const bytes = new Uint8Array(70_000).map((_, i) => i % 256)
    expect(fromBase64(toBase64(bytes))).toEqual(bytes)
    expect(toBase64(new Uint8Array([0, 1, 255]))).toBe('AAH/')
  })
})

describe('coreBridge', () => {
  it('opens with the note’s state and pushes local edits', async () => {
    const core = unlockedCore([{ id: 'a', body: 'hello' }])
    const doc = await coreBridge(core).open('a')

    const ydoc = new Y.Doc()
    Y.applyUpdate(ydoc, doc.initialState)
    expect(ydoc.getText('body').toString()).toBe('hello')

    const before = Y.encodeStateVector(ydoc)
    ydoc.getText('body').insert(5, ' world')
    await doc.push(Y.encodeStateAsUpdate(ydoc, before))
    expect(core.body('a')).toBe('hello world')
    expect(core.attached('a')).toBe(1)

    await doc.close()
    expect(core.attached('a')).toBe(0)
    expect(core.listenerCount('editor-remote-update')).toBe(0)
    await expect(doc.push(new Uint8Array())).rejects.toThrow()
  })

  it('delivers remote updates for its own session only', async () => {
    const core = unlockedCore([
      { id: 'a', body: 'a' },
      { id: 'b', body: 'b' },
    ])
    const bridge = coreBridge(core)
    const docA = await bridge.open('a')
    const docB = await bridge.open('b')
    const ydoc = new Y.Doc()
    Y.applyUpdate(ydoc, docA.initialState)
    let received = 0
    docA.onRemote((update) => {
      received++
      Y.applyUpdate(ydoc, update)
    })
    docB.onRemote(() => {})

    core.remoteEdit('a', (body) => body.insert(0, 'remote '))
    expect(ydoc.getText('body').toString()).toBe('remote a')
    core.remoteEdit('b', (body) => body.insert(0, 'x'))
    expect(received).toBe(1)
  })

  it('keeps updates that arrive before the editor subscribes', async () => {
    const core = unlockedCore([{ id: 'a', body: 'a' }])
    const doc = await coreBridge(core).open('a')
    core.remoteEdit('a', (body) => body.insert(1, 'b'))

    const ydoc = new Y.Doc()
    Y.applyUpdate(ydoc, doc.initialState)
    doc.onRemote((update) => Y.applyUpdate(ydoc, update))
    expect(ydoc.getText('body').toString()).toBe('ab')
  })

  it('attaches only once the update subscription is live', async () => {
    const core = unlockedCore([{ id: 'a', body: 'a' }])
    core.holdSubscriptions = true
    const opening = coreBridge(core).open('a')
    await Promise.resolve()
    await Promise.resolve()
    expect(core.callsTo('editorOpen')).toEqual([])

    core.releaseSubscriptions()
    const doc = await opening
    expect(core.callsTo('editorOpen')).toEqual([['a']])
    // An edit right after the attach reaches the editor.
    core.remoteEdit('a', (body) => body.insert(1, 'b'))
    const ydoc = new Y.Doc()
    Y.applyUpdate(ydoc, doc.initialState)
    doc.onRemote((update) => Y.applyUpdate(ydoc, update))
    expect(ydoc.getText('body').toString()).toBe('ab')
  })

  it('reports pushes the core will never take as NoteClosedError', async () => {
    const core = unlockedCore([{ id: 'a', body: 'a' }])
    const doc = await coreBridge(core).open('a')
    await core.lock()
    const push = doc.push(new Uint8Array([0, 0]))
    await expect(push).rejects.toBeInstanceOf(NoteClosedError)
    await expect(push).rejects.toMatchObject({
      cause: { code: 'locked' },
    })
    // Closing after a lock detaches without an error.
    await doc.close()
    expect(core.listenerCount('editor-remote-update')).toBe(0)
  })

  it('keeps other push failures retryable', async () => {
    const core = unlockedCore([{ id: 'a', body: 'a' }])
    const doc = await coreBridge(core).open('a')
    core.failNext('editorPush', 'internal')
    const push = doc.push(new Uint8Array([0, 0]))
    await expect(push).rejects.not.toBeInstanceOf(NoteClosedError)
  })

  it('rejects and unsubscribes when the note can’t be opened', async () => {
    const core = unlockedCore([])
    await expect(coreBridge(core).open('missing')).rejects.toMatchObject({
      code: 'not_found',
    })
    expect(core.listenerCount('editor-remote-update')).toBe(0)
  })
})
