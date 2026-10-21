/** Formats the core-version line shown on the placeholder page. */
export function formatCoreVersion(version: string | null): string {
  return version === null ? 'browser build' : `core v${version}`
}
