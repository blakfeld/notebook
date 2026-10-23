import { invoke, isTauri } from '@tauri-apps/api/core'
import type { CoreApi } from './core'
import { tauriCore } from './tauri'

export type { CoreApi } from './core'
export { coreBridge } from './noteBridge'
export { coreSettings, type DeviceSettings } from './settings'
export * from './types'

/**
 * Returns the `notebook-core` version from the Tauri host, or `null` when
 * running as a plain browser build.
 */
export async function coreVersion(): Promise<string | null> {
  if (!isTauri()) {
    return null
  }
  return invoke<string>('core_version')
}

/**
 * The core for this platform: the Tauri host when running in the shell.
 * The browser has no core yet (`browser-web-client` adds the WASM one), so
 * a dev build there runs on the in-memory fake to make the UI viewable,
 * and a production build has none.
 */
export async function loadCore(): Promise<CoreApi> {
  if (isTauri()) return tauriCore()
  if (import.meta.env.DEV) {
    const { demoCore } = await import('./demo')
    return demoCore()
  }
  throw new Error('This build has no Notebook core.')
}
