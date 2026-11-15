import { describe, expect, it } from 'vitest'
import { isAllowedExternalUrl } from './external-url-policy'

describe('isAllowedExternalUrl', () => {
  it('allows exact HTTPS URLs for reference check hosts', () => {
    expect(isAllowedExternalUrl('https://ip.net.coffee/')).toBe(true)
    expect(isAllowedExternalUrl('https://fuck-claude.vercel.app/')).toBe(true)
  })

  it('rejects non-HTTPS schemes, unlisted hosts, subdomains, and malformed inputs', () => {
    expect(isAllowedExternalUrl('http://ip.net.coffee/')).toBe(false)
    expect(isAllowedExternalUrl('file:///etc/passwd')).toBe(false)
    expect(isAllowedExternalUrl('javascript:alert(1)')).toBe(false)
    expect(isAllowedExternalUrl('https://sub.ip.net.coffee/')).toBe(false)
    expect(isAllowedExternalUrl('https://example.com/')).toBe(false)
    expect(isAllowedExternalUrl('')).toBe(false)
    expect(isAllowedExternalUrl(null)).toBe(false)
    expect(isAllowedExternalUrl('not a url')).toBe(false)
  })
})

it('allows only the exact safe-browser repository release path on GitHub', () => {
  expect(isAllowedExternalUrl('https://github.com/icesword0760/safe-browser/releases/latest')).toBe(true)
  expect(isAllowedExternalUrl('https://github.com/icesword0760/safe-browser/releases/tag/v148.1')).toBe(true)
  for (const url of [
    'http://github.com/icesword0760/safe-browser/releases/latest',
    'https://sub.github.com/icesword0760/safe-browser/releases/latest',
    'https://github.com.evil.invalid/icesword0760/safe-browser/releases/latest',
    'https://github.com/other/safe-browser/releases/latest',
    'https://github.com/icesword0760/safe-browser',
    'https://github.com/icesword0760/safe-browser/releases-evil/latest',
    'https://github.com/icesword0760/safe-browser/releases/../../other',
    'https://github.com:444/icesword0760/safe-browser/releases/latest',
    'https://user@github.com/icesword0760/safe-browser/releases/latest'
  ]) expect(isAllowedExternalUrl(url)).toBe(false)
})
