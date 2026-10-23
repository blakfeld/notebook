import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

const css = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8')

/** The declarations of the first rule whose selector list is `selector`. */
function block(source: string, selector: string): Record<string, string> {
  const start = source.indexOf(`${selector} {`)
  if (start < 0) throw new Error(`no rule for ${selector}`)
  const open = source.indexOf('{', start)
  const body = source.slice(open + 1, source.indexOf('}', open))
  return Object.fromEntries(
    [...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2]]),
  )
}

const light = block(css, ":root[data-theme='light']")
const dark = block(css, ":root[data-theme='dark']")
// The first rule is `:root:not([data-theme]),\n:root[data-theme='light']`.
const systemDark = block(
  css.slice(css.indexOf('@media (prefers-color-scheme: dark)')),
  ':root:not([data-theme])',
)

/** WCAG relative luminance of a `#rrggbb` color. */
function luminance(hex: string): number {
  const match = /^#([0-9a-f]{6})$/i.exec(hex)
  if (!match) throw new Error(`not a hex color: ${hex}`)
  const [r, g, b] = [0, 2, 4].map((i) => {
    const c = parseInt(match[1].slice(i, i + 2), 16) / 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

/** Text that must be readable: 4.5:1. */
const TEXT_PAIRS: [string, string][] = [
  ['--text', '--bg'],
  ['--text', '--surface'],
  ['--text', '--hover'],
  ['--text', '--selected'],
  ['--muted', '--bg'],
  ['--muted', '--surface'],
  ['--link', '--bg'],
  ['--accent', '--bg'],
  ['--accent-text', '--accent'],
  ['--danger', '--bg'],
  ['--warning', '--bg'],
  ['--tag-fg', '--tag-bg'],
  ['--code-text', '--code-bg'],
  ['--hl-keyword', '--code-bg'],
  ['--hl-string', '--code-bg'],
  ['--hl-comment', '--code-bg'],
  ['--hl-number', '--code-bg'],
  ['--hl-function', '--code-bg'],
  ['--hl-type', '--code-bg'],
]

/** Controls, focus rings, revealed syntax, and dimmed text: 3:1. */
const UI_PAIRS: [string, string][] = [
  ['--control', '--bg'],
  ['--control', '--surface'],
  ['--focus', '--bg'],
  ['--focus', '--surface'],
  ['--syntax-marker', '--bg'],
  ['--syntax-marker', '--code-bg'],
  ['--dim', '--bg'],
]

describe.each([
  ['light', light],
  ['dark', dark],
])('%s theme', (_name, tokens) => {
  it.each(TEXT_PAIRS)('%s on %s is at least 4.5:1', (fg, bg) => {
    expect(contrast(tokens[fg], tokens[bg])).toBeGreaterThanOrEqual(4.5)
  })

  it.each(UI_PAIRS)('%s on %s is at least 3:1', (fg, bg) => {
    expect(contrast(tokens[fg], tokens[bg])).toBeGreaterThanOrEqual(3)
  })
})

describe('token sets', () => {
  it('define the same tokens in both themes', () => {
    expect(Object.keys(dark).sort()).toEqual(Object.keys(light).sort())
  })

  it('use the dark set for System on a dark OS', () => {
    expect(systemDark).toEqual(dark)
  })
})
