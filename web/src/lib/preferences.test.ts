import { get } from 'svelte/store'
import { describe, expect, it } from 'vitest'
import { memorySettings } from '../platform/settings'
import { createPreferences, DEFAULT_PREFERENCES } from './preferences'

describe('preferences', () => {
  it('starts at the defaults: System theme, live preview, no focus mode', async () => {
    const prefs = createPreferences(memorySettings())
    expect(get(prefs)).toEqual(DEFAULT_PREFERENCES)
    await prefs.load()
    expect(prefs.current).toEqual({
      theme: 'system',
      sourceView: false,
      focusMode: false,
    })
  })

  it('saves choices and reads them back on the next load', async () => {
    const settings = memorySettings()
    const prefs = createPreferences(settings)
    prefs.set('theme', 'dark')
    prefs.set('sourceView', true)
    prefs.set('focusMode', true)
    expect(prefs.current).toEqual({
      theme: 'dark',
      sourceView: true,
      focusMode: true,
    })
    await Promise.resolve()
    expect(await settings.get('appearance.theme')).toBe('dark')
    expect(await settings.get('editor.sourceView')).toBe('true')

    const restarted = createPreferences(settings)
    await restarted.load()
    expect(get(restarted)).toEqual({
      theme: 'dark',
      sourceView: true,
      focusMode: true,
    })
  })

  it('uses the defaults when storage can’t be read', async () => {
    const settings = memorySettings()
    await settings.set('appearance.theme', 'light')
    settings.options.failReads = true
    const prefs = createPreferences(settings)
    await prefs.load()
    expect(prefs.current).toEqual(DEFAULT_PREFERENCES)
  })

  it('ignores a malformed stored theme', async () => {
    const settings = memorySettings()
    await settings.set('appearance.theme', 'sepia')
    const prefs = createPreferences(settings)
    await prefs.load()
    expect(prefs.current.theme).toBe('system')
  })

  it('keeps a value set while a load was in flight', async () => {
    const settings = memorySettings()
    await settings.set('appearance.theme', 'light')
    const prefs = createPreferences(settings)
    const loading = prefs.load()
    prefs.set('theme', 'dark')
    await loading
    expect(prefs.current.theme).toBe('dark')
  })

  it('reports a failed save and keeps the value for the session', async () => {
    const prefs = createPreferences(memorySettings({ failWrites: true }))
    prefs.set('focusMode', true)
    await new Promise((resolve) => setTimeout(resolve))
    expect(prefs.current.focusMode).toBe(true)
    expect(get(prefs.saveError)).toBeInstanceOf(Error)
    prefs.clearSaveError()
    expect(get(prefs.saveError)).toBeNull()
  })
})
