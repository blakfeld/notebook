import { describe, expect, it } from 'vitest'
import { formatVersion } from './version'

describe('formatVersion', () => {
  it('shows the version the core reports', () => {
    expect(formatVersion('0.1.0')).toBe('v0.1.0')
  })

  it('says when the version couldn’t be read', () => {
    expect(formatVersion(null)).toBe('Unknown')
  })
})
