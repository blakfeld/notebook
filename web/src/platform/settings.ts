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
