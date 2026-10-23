/**
 * The app version as Settings › About shows it. The core and the host
 * share the workspace version, so the core's is the app's. `null` means it
 * couldn't be read.
 */
export function formatVersion(version: string | null): string {
  return version === null ? 'Unknown' : `v${version}`
}
