import { describe, expect, it, vi } from 'vitest'

import { configureUpdateFeed, parseUpdateBaseUrl } from './update-feed'

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

  it('configures the production feed even when app-update.yml is missing', () => {
    const updater = { channel: '', setFeedURL: vi.fn() }

    const url = configureUpdateFeed(updater, {
      packagedConfigPath: '/definitely-missing/app-update.yml', channel: 'stable'
    })

    expect(url).toBe('https://updates.baize-ailabs.com/stable')
    expect(updater.channel).toBe('stable')
    expect(updater.setFeedURL).toHaveBeenCalledWith({
      provider: 'generic', url: 'https://updates.baize-ailabs.com/stable', channel: 'stable'
    })
  })

  it('prefers an explicit feed override for local release verification', () => {
    const updater = { channel: '', setFeedURL: vi.fn() }

    expect(configureUpdateFeed(updater, {
      overrideUrl: 'https://updates.example.com/test',
      packagedConfigPath: '/missing/app-update.yml', channel: 'preview'
    })).toBe('https://updates.example.com/test')
  })
})
