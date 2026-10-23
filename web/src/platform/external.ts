import { isTauri } from '@tauri-apps/api/core'

/** The only schemes a link in a note may open. */
const ALLOWED_PROTOCOLS = new Set(['http:', 'https:', 'mailto:'])

/**
 * The URL to open for a link's target, or `null` if it must not open: it
 * doesn't parse, or its scheme isn't `http`, `https`, or `mailto`.
 */
export function openableUrl(url: string): string | null {
  let parsed: URL
  try {
    parsed = new URL(url.trim())
  } catch {
    return null
  }
  return ALLOWED_PROTOCOLS.has(parsed.protocol) ? parsed.href : null
}

/** Hands a URL to the system browser (or a new tab, in the browser). */
export type Opener = (url: string) => Promise<void> | void

async function platformOpener(url: string): Promise<void> {
  if (isTauri()) {
    // The host scopes the opener to the same schemes (`default.json`).
    const { openUrl } = await import('@tauri-apps/plugin-opener')
    await openUrl(url)
  } else {
    window.open(url, '_blank', 'noopener,noreferrer')
  }
}

/**
 * Opens a link from a note outside the app. Anything that isn't an `http`,
 * `https`, or `mailto` URL is refused before reaching the platform. Returns
 * whether it was handed over.
 */
export async function openExternal(
  url: string,
  opener: Opener = platformOpener,
): Promise<boolean> {
  const allowed = openableUrl(url)
  if (allowed === null) return false
  await opener(allowed)
  return true
}
