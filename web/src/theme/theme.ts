import type { Readable } from 'svelte/store'
import type { ThemePreference } from '../platform/types'

export type Theme = 'light' | 'dark'

const DARK_QUERY = '(prefers-color-scheme: dark)'

/** The theme a preference means, given whether the OS prefers dark. */
export function resolveTheme(pref: ThemePreference, osDark: boolean): Theme {
  if (pref === 'system') return osDark ? 'dark' : 'light'
  return pref
}

/**
 * Keeps `data-theme` on `root` in step with the preference and, under
 * System, with the OS. Applies the current value synchronously, so calling
 * this before mounting the app means the first frame has the right theme.
 * Returns a function that stops following both.
 */
export function applyTheme(
  preference: Readable<{ theme: ThemePreference }>,
  root: HTMLElement = document.documentElement,
  media: MediaQueryList = window.matchMedia(DARK_QUERY),
): () => void {
  let pref: ThemePreference = 'system'
  const apply = () => {
    root.dataset.theme = resolveTheme(pref, media.matches)
  }
  const unsubscribe = preference.subscribe(({ theme }) => {
    pref = theme
    apply()
  })
  media.addEventListener('change', apply)
  return () => {
    unsubscribe()
    media.removeEventListener('change', apply)
  }
}
