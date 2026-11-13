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
  return parsed.protocol === 'https:' && ALLOWED_EXTERNAL_HOSTS.has(parsed.hostname)
}
