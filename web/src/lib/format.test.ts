// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { dayTime, displayTitle, relativeTime, shortcutOf } from './format'

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

describe('dayTime', () => {
  const now = new Date(2026, 9, 22, 15, 0).getTime()
  const at = (day: number, hour: number) =>
    new Date(2026, 9, day, hour, 5).getTime()

  it('names the day for yesterday, today, and tomorrow', () => {
    expect(dayTime(at(22, 9), now)).toMatch(/^Today at 9:05/)
    expect(dayTime(at(21, 23), now)).toMatch(/^Yesterday at 11:05/)
    expect(dayTime(at(23, 1), now)).toMatch(/^Tomorrow at 1:05/)
  })

  it('gives the date further out', () => {
    expect(dayTime(at(29, 9), now)).toMatch(/29.* at 9:05/)
    expect(dayTime(new Date(2025, 0, 3, 9, 5).getTime(), now)).toMatch(/2025/)
  })
})

describe('displayTitle', () => {
  it('shows Untitled for an empty title', () => {
    expect(displayTitle('')).toBe('Untitled')
    expect(displayTitle('  ')).toBe('Untitled')
    expect(displayTitle('Plan')).toBe('Plan')
  })
})

describe('shortcutOf', () => {
  const key = (init: KeyboardEventInit) => new KeyboardEvent('keydown', init)

  it('uses Cmd on a Mac and Ctrl elsewhere', () => {
    expect(shortcutOf(key({ key: 'n', metaKey: true }), true)).toBe('new-note')
    expect(shortcutOf(key({ key: 'n', ctrlKey: true }), true)).toBeNull()
    expect(shortcutOf(key({ key: 'n', ctrlKey: true }), false)).toBe('new-note')
  })

  it('knows search, and nothing else', () => {
    expect(
      shortcutOf(key({ key: 'F', metaKey: true, shiftKey: true }), true),
    ).toBe('search')
    expect(shortcutOf(key({ key: 'f', metaKey: true }), true)).toBeNull()
    // There is no lock any more, so Mod+L is left to the system.
    expect(shortcutOf(key({ key: 'l', metaKey: true }), true)).toBeNull()
  })
})
