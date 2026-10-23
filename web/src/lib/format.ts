const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/**
 * A note's updated time relative to `now`: "Just now", "5 min ago",
 * "3 h ago", "Yesterday", then a date.
 */
export function relativeTime(updated: number, now = Date.now()): string {
  const age = now - updated
  if (age < MINUTE) return 'Just now'
  if (age < HOUR) return `${Math.floor(age / MINUTE)} min ago`
  const then = new Date(updated)
  const today = new Date(now)
  if (age < DAY && then.getDate() === today.getDate()) {
    return `${Math.floor(age / HOUR)} h ago`
  }
  const yesterday = new Date(now - DAY)
  if (then.toDateString() === yesterday.toDateString()) return 'Yesterday'
  return then.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: then.getFullYear() === today.getFullYear() ? undefined : 'numeric',
  })
}

/** The title a list entry shows. */
export function displayTitle(title: string): string {
  return title.trim() === '' ? 'Untitled' : title
}

/** Mac keyboards use ⌘ where others use Ctrl. */
export function isMac(): boolean {
  return /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent)
}

export type Shortcut = 'new-note' | 'search'

/** The app shortcut a key press means, if any. */
export function shortcutOf(
  event: KeyboardEvent,
  mac = isMac(),
): Shortcut | null {
  const mod = mac
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey
  if (!mod || event.altKey) return null
  const key = event.key.toLowerCase()
  if (key === 'n' && !event.shiftKey) return 'new-note'
  if (key === 'f' && event.shiftKey) return 'search'
  return null
}

/** "⌘N" or "Ctrl+N", for labels. */
export function shortcutLabel(
  key: string,
  shift = false,
  mac = isMac(),
): string {
  if (mac) return `${shift ? '⇧' : ''}⌘${key}`
  return `Ctrl+${shift ? 'Shift+' : ''}${key}`
}
