import { describe, expect, it } from 'vitest'

import { parseUpdateBaseUrl } from './update-feed'

describe('parseUpdateBaseUrl', () => {
  it('reads the generic provider URL from packaged app-update.yml content', () => {
    expect(parseUpdateBaseUrl([
      'provider: generic',
      'url: https://updates.example.com/stable',
      'updaterCacheDirName: matou-updater'
    ].join('\n'))).toBe('https://updates.example.com/stable')
  })

  it('returns undefined when the package does not contain a generic update URL', () => {
    expect(parseUpdateBaseUrl('provider: github\nowner: matou')).toBeUndefined()
  })
})
