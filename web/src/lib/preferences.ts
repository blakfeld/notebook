import { writable, type Readable } from 'svelte/store'
import type { DeviceSettings } from '../platform/settings'
import type { ThemePreference } from '../platform/types'

/**
 * The UI preferences live preview adds: theme, source view, and focus mode.
 * Device-local and non-sensitive, like vim mode, and never synced.
 */
export interface Preferences {
  theme: ThemePreference
  sourceView: boolean
  focusMode: boolean
}

export const DEFAULT_PREFERENCES: Readonly<Preferences> = {
  theme: 'system',
  sourceView: false,
  focusMode: false,
}

/** The device-setting key behind each preference. */
export const PREFERENCE_KEYS: Readonly<Record<keyof Preferences, string>> = {
  theme: 'appearance.theme',
  sourceView: 'editor.sourceView',
  focusMode: 'editor.focusMode',
}

const THEMES: readonly string[] = ['system', 'light', 'dark']

function parse<K extends keyof Preferences>(
  key: K,
  raw: string | null,
): Preferences[K] {
  if (key === 'theme') {
    return (
      raw !== null && THEMES.includes(raw) ? raw : DEFAULT_PREFERENCES.theme
    ) as Preferences[K]
  }
  return (raw === 'true') as Preferences[K]
}

export interface PreferencesStore extends Readable<Preferences> {
  /** The values right now. */
  readonly current: Preferences
  /**
   * Reads every preference from the device. Anything unreadable or
   * malformed keeps its default; this never rejects.
   */
  load(): Promise<void>
  /**
   * Applies `value` at once and saves it on this device. A failed save is
   * reported to `onError`; the value still applies for this session.
   */
  set<K extends keyof Preferences>(key: K, value: Preferences[K]): void
}

export function createPreferences(
  settings: DeviceSettings,
  onError: (error: unknown) => void = () => {},
): PreferencesStore {
  let current: Preferences = { ...DEFAULT_PREFERENCES }
  const store = writable(current)
  /** Preferences set this session, which a slower `load` must not undo. */
  const chosen = new Set<keyof Preferences>()

  const update = (next: Preferences) => {
    current = next
    store.set(next)
  }

  return {
    subscribe: store.subscribe,
    get current() {
      return current
    },

    async load() {
      const keys = Object.keys(PREFERENCE_KEYS) as (keyof Preferences)[]
      const values = await Promise.all(
        keys.map((key) =>
          settings.get(PREFERENCE_KEYS[key]).then(
            (raw) => parse(key, raw),
            () => DEFAULT_PREFERENCES[key],
          ),
        ),
      )
      const next = { ...current }
      keys.forEach((key, i) => {
        if (!chosen.has(key)) (next as Record<string, unknown>)[key] = values[i]
      })
      update(next)
    },

    set(key, value) {
      chosen.add(key)
      if (current[key] !== value) update({ ...current, [key]: value })
      settings.set(PREFERENCE_KEYS[key], String(value)).catch(onError)
    },
  }
}
