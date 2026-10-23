import type { CoreApi } from './core'

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

/**
 * The {@link DeviceSettings} of a native shell, over the core's device
 * settings. The core has typed commands rather than a key-value store, so
 * only `editor.vimMode` is supported: other keys read as unset and reject
 * on write.
 *
 * A write equal to the last value seen is skipped, so the settings screen
 * and the open editor setting the same value write it once.
 */
export function coreSettings(core: CoreApi): DeviceSettings {
  let last: string | null = null
  return {
    async get(key) {
      if (key !== VIM_MODE) return null
      last = (await core.getSettings()).vim_mode ? 'true' : 'false'
      return last
    },
    async set(key, value) {
      if (key !== VIM_MODE) {
        throw new Error(`unsupported device setting: ${key}`)
      }
      if (value === last) return
      // Recorded first, so a second caller during the write skips it.
      const previous = last
      last = value
      try {
        await core.setVimMode(value === 'true')
      } catch (error) {
        last = previous
        throw error
      }
    },
  }
}
