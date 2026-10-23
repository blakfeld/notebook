// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { displayTitle, relativeTime, shortcutOf, syncSummary } from './format'

describe('relativeTime', () => {
  const now = new Date(2026, 9, 22, 15, 0).getTime()

  it('reads naturally for recent times', () => {
    expect(relativeTime(now - 10_000, now)).toBe('Just now')
    expect(relativeTime(now - 5 * 60_000, now)).toBe('5 min ago')
    expect(relativeTime(now - 3 * 3_600_000, now)).toBe('3 h ago')
    expect(relativeTime(new Date(2026, 9, 21, 9).getTime(), now)).toBe(
      'Yesterday',
    )
  })

  it('falls back to a date', () => {
    expect(relativeTime(new Date(2026, 0, 3).getTime(), now)).toMatch(/3/)
  })
})

describe('displayTitle', () => {
  it('shows Untitled for an empty title', () => {
    expect(displayTitle('')).toBe('Untitled')
    expect(displayTitle('  ')).toBe('Untitled')
    expect(displayTitle('Plan')).toBe('Plan')
  })
})

describe('syncSummary', () => {
  it('hides a zero pending count', () => {
    expect(
      syncSummary({ state: 'synced', pending: 0, attention: [] }).pending,
    ).toBeNull()
    expect(
      syncSummary({ state: 'offline', pending: 2, attention: [] }).pending,
    ).toBe('2 pending')
  })
})

describe('shortcutOf', () => {
  const key = (init: KeyboardEventInit) => new KeyboardEvent('keydown', init)

  it('uses Cmd on a Mac and Ctrl elsewhere', () => {
    expect(shortcutOf(key({ key: 'n', metaKey: true }), true)).toBe('new-note')
    expect(shortcutOf(key({ key: 'n', ctrlKey: true }), true)).toBeNull()
    expect(shortcutOf(key({ key: 'n', ctrlKey: true }), false)).toBe('new-note')
  })

  it('knows search and lock', () => {
    expect(
      shortcutOf(key({ key: 'F', metaKey: true, shiftKey: true }), true),
    ).toBe('search')
    expect(shortcutOf(key({ key: 'f', metaKey: true }), true)).toBeNull()
    expect(shortcutOf(key({ key: 'l', metaKey: true }), true)).toBe('lock')
  })
})
