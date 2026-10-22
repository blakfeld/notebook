import { describe, expect, it } from 'vitest'
import { memorySettings } from './settings'

describe('memorySettings', () => {
  it('stores and reads values', async () => {
    const settings = memorySettings()
    await settings.set('editor.vimMode', 'true')
    expect(await settings.get('editor.vimMode')).toBe('true')
    await settings.set('editor.vimMode', 'false')
    expect(await settings.get('editor.vimMode')).toBe('false')
  })

  it('reads a missing key as null', async () => {
    expect(await memorySettings().get('nothing.here')).toBeNull()
  })

  it('can fail reads and writes', async () => {
    const settings = memorySettings({ failReads: true, failWrites: true })
    await expect(settings.get('a')).rejects.toThrow()
    await expect(settings.set('a', 'b')).rejects.toThrow()
    settings.options.failWrites = false
    await settings.set('a', 'b')
    settings.options.failReads = false
    expect(await settings.get('a')).toBe('b')
  })
})
