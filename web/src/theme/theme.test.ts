// @vitest-environment jsdom
import { writable } from 'svelte/store'
import { describe, expect, it } from 'vitest'
import type { ThemePreference } from '../platform/types'
import { applyTheme, resolveTheme } from './theme'

/** A `matchMedia` result whose answer the test controls. */
function fakeMedia(matches: boolean) {
  const listeners = new Set<() => void>()
  const media = {
    matches,
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
  }
  return {
    media: media as unknown as MediaQueryList,
    listeners,
    /** The OS switches its color scheme. */
    set(dark: boolean) {
      media.matches = dark
      for (const fn of listeners) fn()
    },
  }
}

function setup(theme: ThemePreference, osDark: boolean) {
  const os = fakeMedia(osDark)
  const pref = writable({ theme })
  const root = document.createElement('html')
  const stop = applyTheme(pref, root, os.media)
  return { os, pref, root, stop }
}

describe('resolveTheme', () => {
  it.each([
    ['system', false, 'light'],
    ['system', true, 'dark'],
    ['light', true, 'light'],
    ['light', false, 'light'],
    ['dark', false, 'dark'],
    ['dark', true, 'dark'],
  ] as const)('%s with OS dark=%s is %s', (pref, osDark, expected) => {
    expect(resolveTheme(pref, osDark)).toBe(expected)
  })
})

describe('applyTheme', () => {
  it('applies at once, before returning', () => {
    expect(setup('dark', false).root.dataset.theme).toBe('dark')
    expect(setup('system', true).root.dataset.theme).toBe('dark')
    expect(setup('system', false).root.dataset.theme).toBe('light')
  })

  it('follows the OS live under System', () => {
    const { os, root } = setup('system', false)
    os.set(true)
    expect(root.dataset.theme).toBe('dark')
    os.set(false)
    expect(root.dataset.theme).toBe('light')
  })

  it('ignores the OS under Light or Dark', () => {
    const { os, pref, root } = setup('light', true)
    expect(root.dataset.theme).toBe('light')
    os.set(false)
    os.set(true)
    expect(root.dataset.theme).toBe('light')
    pref.set({ theme: 'dark' })
    os.set(false)
    expect(root.dataset.theme).toBe('dark')
  })

  it('switches when the preference changes, and back to System', () => {
    const { pref, root } = setup('dark', false)
    pref.set({ theme: 'system' })
    expect(root.dataset.theme).toBe('light')
  })

  it('stops following when stopped', () => {
    const { os, pref, root, stop } = setup('system', false)
    stop()
    os.set(true)
    pref.set({ theme: 'dark' })
    expect(root.dataset.theme).toBe('light')
    expect(os.listeners.size).toBe(0)
  })
})
