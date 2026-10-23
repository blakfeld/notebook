import type { CoreApi } from './core'
import type { Settings } from './types'

/**
 * Device-local preferences: non-sensitive UI settings stored on this device
 * only and never synced (vim mode, and later theme and view options).
 *
 * Keys are namespaced strings such as `editor.vimMode`; values are strings.
 * A missing key, or a read that fails, means "use the default". A write that
 * fails is reported by the caller to the host, and the new value still
 * applies for the rest of the session.
 *
 * Native shells back this with the local store's device settings; the
 * browser backs it with `localStorage`.
 */
export interface DeviceSettings {
  /** The stored value, or `null` if there is none. */
  get(key: string): Promise<string | null>
  set(key: string, value: string): Promise<void>
}

export interface MemorySettingsOptions {
  /** Make every `get` reject. */
  failReads?: boolean
  /** Make every `set` reject. */
  failWrites?: boolean
}

/** An in-memory {@link DeviceSettings} for tests and the placeholder page. */
export function memorySettings(
  options: MemorySettingsOptions = {},
): DeviceSettings & { options: MemorySettingsOptions } {
  const values = new Map<string, string>()
  return {
    options,
    async get(key) {
      if (options.failReads) {
        throw new Error('settings unavailable')
      }
      return values.get(key) ?? null
    },
    async set(key, value) {
      if (options.failWrites) {
        throw new Error('settings unavailable')
      }
      values.set(key, value)
    },
  }
}

// `VIM_MODE_KEY` in the editor, repeated so this layer doesn't load it.
const VIM_MODE = 'editor.vimMode'

/** The keys the core stores, and how each reads from its settings. */
const CORE_KEYS: Record<string, (settings: Settings) => string> = {
  [VIM_MODE]: (s) => String(s.vim_mode),
  'appearance.theme': (s) => s.theme,
  'editor.sourceView': (s) => String(s.source_view),
  'editor.focusMode': (s) => String(s.focus_mode),
}

/**
 * The {@link DeviceSettings} of a native shell, over the core's device
 * settings. The core has typed settings rather than a key-value store, so
 * only the keys it knows are supported: others read as unset and reject on
 * write.
 *
 * A write equal to the last value seen for its key is skipped, so the
 * settings screen and the open editor setting the same value write it once.
 */
export function coreSettings(core: CoreApi): DeviceSettings {
  const last = new Map<string, string>()
  return {
    async get(key) {
      const read = CORE_KEYS[key]
      if (!read) return null
      const value = read(await core.getSettings())
      last.set(key, value)
      return value
    },
    async set(key, value) {
      if (!CORE_KEYS[key]) {
        throw new Error(`unsupported device setting: ${key}`)
      }
      const previous = last.get(key)
      if (value === previous) return
      // Recorded first, so a second caller during the write skips it.
      last.set(key, value)
      try {
        if (key === VIM_MODE) await core.setVimMode(value === 'true')
        else await core.setPreference(key, value)
      } catch (error) {
        if (previous === undefined) last.delete(key)
        else last.set(key, previous)
        throw error
      }
    },
  }
}
