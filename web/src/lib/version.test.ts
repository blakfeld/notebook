import { describe, expect, it } from 'vitest'
import { formatCoreVersion } from './version'

describe('formatCoreVersion', () => {
  it('shows the core version when the host provides one', () => {
    expect(formatCoreVersion('0.1.0')).toBe('core v0.1.0')
  })

  it('falls back to "browser build" without a host', () => {
    expect(formatCoreVersion(null)).toBe('browser build')
  })
})
