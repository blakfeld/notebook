// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { openableUrl, openExternal } from './external'

afterEach(() => {
  vi.restoreAllMocks()
})

describe('openExternal', () => {
  it.each([
    'https://example.com/docs',
    'http://example.com',
    'mailto:sam@example.com',
    '  https://example.com  ',
  ])('opens %s', async (url) => {
    const opener = vi.fn()
    expect(await openExternal(url, opener)).toBe(true)
    expect(opener).toHaveBeenCalledOnce()
    expect(opener.mock.calls[0][0]).toBe(openableUrl(url))
  })

  it.each([
    'javascript:alert(1)',
    'JavaScript:alert(1)',
    'file:///etc/passwd',
    'data:text/html,<script>alert(1)</script>',
    'vbscript:x',
    'ftp://example.com',
    'not a url',
    '/relative/path',
    '',
  ])('refuses %s without calling the platform', async (url) => {
    const opener = vi.fn()
    expect(await openExternal(url, opener)).toBe(false)
    expect(opener).not.toHaveBeenCalled()
  })

  it('opens a new tab without opener or referrer in the browser', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null)
    expect(await openExternal('https://example.com')).toBe(true)
    expect(open).toHaveBeenCalledWith(
      'https://example.com/',
      '_blank',
      'noopener,noreferrer',
    )
  })
})
