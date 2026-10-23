import { describe, expect, it } from 'vitest'
import { FakeCoreApi } from './fake'
import { coreSettings, memorySettings } from './settings'

describe('coreSettings', () => {
  it('maps editor.vimMode to the core’s vim mode setting', async () => {
    const core = new FakeCoreApi()
    const settings = coreSettings(core)
    expect(await settings.get('editor.vimMode')).toBe('false')
    await settings.set('editor.vimMode', 'true')
    expect(core.settings.vim_mode).toBe(true)
    expect(core.callsTo('setVimMode')).toEqual([[true]])
    expect(await settings.get('editor.vimMode')).toBe('true')
  })

  it('writes a repeated value once', async () => {
    const core = new FakeCoreApi()
    const settings = coreSettings(core)
    await settings.set('editor.vimMode', 'true')
    await settings.set('editor.vimMode', 'true')
    expect(core.callsTo('setVimMode')).toEqual([[true]])
  })

  it('has no other keys', async () => {
    const settings = coreSettings(new FakeCoreApi())
    expect(await settings.get('appearance.theme')).toBeNull()
    await expect(settings.set('appearance.theme', 'dark')).rejects.toThrow()
  })
})

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
