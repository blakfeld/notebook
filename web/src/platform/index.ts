import { invoke, isTauri } from '@tauri-apps/api/core'

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
