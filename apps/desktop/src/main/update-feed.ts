import { readFileSync } from 'node:fs'

export const DEFAULT_UPDATE_BASE_URL = 'https://updates.baize-ailabs.com/stable'

interface UpdateFeedAdapter {
  channel?: string | null
  allowDowngrade?: boolean
  setFeedURL(options: { provider: 'generic'; url: string; channel?: string }): void
}

interface ConfigureUpdateFeedOptions {
  overrideUrl?: string
  packagedConfigPath: string
  channel: string
}

export function parseUpdateBaseUrl(content: string): string | undefined {
  if (!/^provider:\s*generic\s*$/m.test(content)) return undefined
  const value = content.match(/^url:\s*(.+?)\s*$/m)?.[1]?.trim()
  return value || undefined
}

export function readUpdateBaseUrl(path: string): string | undefined {
  try {
    return parseUpdateBaseUrl(readFileSync(path, 'utf8'))
  } catch {
    return undefined
  }
}

export function configureUpdateFeed(
  updater: UpdateFeedAdapter, options: ConfigureUpdateFeedOptions
): string {
  const url = options.overrideUrl?.trim()
    || readUpdateBaseUrl(options.packagedConfigPath)
    || DEFAULT_UPDATE_BASE_URL
  updater.channel = options.channel
  // electron-updater enables downgrades when channel is assigned, even if unchanged.
  // A local acceptance build must not offer the older published release as an update.
  updater.allowDowngrade = false
  updater.setFeedURL({ provider: 'generic', url, channel: options.channel })
  return url
}
