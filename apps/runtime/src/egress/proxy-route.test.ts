import { describe, expect, it } from 'vitest'

import { hasProxyEnvironment, resolveProxyForUrl } from './proxy-route'

describe('resolveProxyForUrl', () => {
  it('prefers https_proxy over HTTPS_PROXY and falls back to all_proxy/ALL_PROXY for https URLs', () => {
    expect(resolveProxyForUrl({
      https_proxy: 'http://127.0.0.1:7890',
      HTTPS_PROXY: 'http://127.0.0.1:7891',
      all_proxy: 'socks5://127.0.0.1:7892',
      ALL_PROXY: 'socks5://127.0.0.1:7893'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBe('http://127.0.0.1:7890')

    expect(resolveProxyForUrl({
      HTTPS_PROXY: 'http://127.0.0.1:7891',
      all_proxy: 'socks5://127.0.0.1:7892'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBe('http://127.0.0.1:7891')

    expect(resolveProxyForUrl({
      all_proxy: 'socks5://127.0.0.1:7892',
      ALL_PROXY: 'socks5://127.0.0.1:7893'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBe('socks5://127.0.0.1:7892')

    expect(resolveProxyForUrl({
      ALL_PROXY: 'socks5://127.0.0.1:7893'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBe('socks5://127.0.0.1:7893')
  })

  it('uses http_proxy for http URLs and falls back to all_proxy', () => {
    expect(resolveProxyForUrl({
      http_proxy: 'http://127.0.0.1:8080',
      HTTPS_PROXY: 'http://127.0.0.1:7890'
    }, 'http://example.com/health')).toBe('http://127.0.0.1:8080')

    expect(resolveProxyForUrl({
      HTTP_PROXY: 'http://127.0.0.1:8081'
    }, 'http://example.com/health')).toBe('http://127.0.0.1:8081')

    expect(resolveProxyForUrl({
      ALL_PROXY: 'socks5://127.0.0.1:7891'
    }, 'http://example.com/health')).toBe('socks5://127.0.0.1:7891')
  })

  it('returns undefined when NO_PROXY is wildcard *', () => {
    expect(resolveProxyForUrl({
      HTTPS_PROXY: 'http://127.0.0.1:7890',
      NO_PROXY: '*'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBeUndefined()

    expect(resolveProxyForUrl({
      HTTPS_PROXY: 'http://127.0.0.1:7890',
      no_proxy: '*'
    }, 'https://api.openai.com/cdn-cgi/trace')).toBeUndefined()
  })

  it('matches .domain and bare domain in NO_PROXY without matching superstrings', () => {
    const withLeadingDot = {
      HTTPS_PROXY: 'http://127.0.0.1:7890',
      NO_PROXY: 'localhost, .anthropic.com'
    }
    expect(resolveProxyForUrl(withLeadingDot, 'https://api.anthropic.com/cdn-cgi/trace')).toBeUndefined()
    expect(resolveProxyForUrl(withLeadingDot, 'https://anthropic.com/cdn-cgi/trace')).toBeUndefined()
    expect(resolveProxyForUrl(withLeadingDot, 'https://notanthropic.com/cdn-cgi/trace')).toBe('http://127.0.0.1:7890')

    const withoutLeadingDot = {
      HTTPS_PROXY: 'http://127.0.0.1:7890',
      NO_PROXY: 'anthropic.com'
    }
    expect(resolveProxyForUrl(withoutLeadingDot, 'https://api.anthropic.com/cdn-cgi/trace')).toBeUndefined()
    expect(resolveProxyForUrl(withoutLeadingDot, 'https://anthropic.com/cdn-cgi/trace')).toBeUndefined()
    expect(resolveProxyForUrl(withoutLeadingDot, 'https://notanthropic.com/cdn-cgi/trace')).toBe('http://127.0.0.1:7890')
  })

  it('respects optional port filters in NO_PROXY and case-insensitive hosts', () => {
    expect(resolveProxyForUrl({
      HTTPS_PROXY: 'http://127.0.0.1:7890',
      NO_PROXY: 'API.ANTHROPIC.COM:443'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBeUndefined()

    expect(resolveProxyForUrl({
      HTTPS_PROXY: 'http://127.0.0.1:7890',
      NO_PROXY: 'api.anthropic.com:8443'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBe('http://127.0.0.1:7890')
  })

  it('prepends http:// when proxy value omits scheme', () => {
    expect(resolveProxyForUrl({
      HTTPS_PROXY: '127.0.0.1:7890'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBe('http://127.0.0.1:7890')
  })

  it('preserves socks5h:// proxy URLs as-is', () => {
    expect(resolveProxyForUrl({
      ALL_PROXY: 'socks5h://127.0.0.1:7891'
    }, 'https://api.anthropic.com/cdn-cgi/trace')).toBe('socks5h://127.0.0.1:7891')
  })
})

describe('hasProxyEnvironment', () => {
  it('returns true when any proxy environment variable is set', () => {
    expect(hasProxyEnvironment({ ALL_PROXY: 'x' })).toBe(true)
    expect(hasProxyEnvironment({ https_proxy: 'http://127.0.0.1:7890' })).toBe(true)
    expect(hasProxyEnvironment({ HTTP_PROXY: 'http://127.0.0.1:7890' })).toBe(true)
  })

  it('returns false when no proxy environment variable is set', () => {
    expect(hasProxyEnvironment({})).toBe(false)
    expect(hasProxyEnvironment({ HTTPS_PROXY: '   ', NO_PROXY: '*' })).toBe(false)
  })
})
