export const SAFE_BROWSER_REPOSITORY = 'icesword0760/mantle-browser'
export const SAFE_BROWSER_RELEASES_URL = `https://github.com/${SAFE_BROWSER_REPOSITORY}/releases/latest`

export const ALLOWED_EXTERNAL_HOSTS: ReadonlySet<string> = new Set([
  'ip.net.coffee',
  'fuck-claude.vercel.app'
])

/** Check whether a candidate external URL uses HTTPS on an explicitly allowlisted host. */
export function isAllowedExternalUrl(url: unknown): url is string {
  if (typeof url !== 'string' || url.length === 0) return false
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return false
  }
  if (parsed.protocol !== 'https:' || parsed.username || parsed.password || parsed.port) return false
  if (ALLOWED_EXTERNAL_HOSTS.has(parsed.hostname)) return true
  const releasesPrefix = `/${SAFE_BROWSER_REPOSITORY}/releases`
  return parsed.hostname === 'github.com' &&
    (parsed.pathname === releasesPrefix || parsed.pathname.startsWith(`${releasesPrefix}/`))
}
