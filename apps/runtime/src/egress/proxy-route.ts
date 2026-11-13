export type Environment = Record<string, string | undefined>

const PROXY_ENV_KEYS = [
  'https_proxy',
  'HTTPS_PROXY',
  'http_proxy',
  'HTTP_PROXY',
  'all_proxy',
  'ALL_PROXY'
] as const

export function hasProxyEnvironment(env: Environment): boolean {
  return firstNonEmpty(env, PROXY_ENV_KEYS) !== undefined
}

export function resolveProxyForUrl(env: Environment, url: string): string | undefined {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return undefined
  }

  const noProxy = env.no_proxy ?? env.NO_PROXY
  if (noProxy && matchesNoProxy(parsed, noProxy)) {
    return undefined
  }

  const raw = parsed.protocol === 'https:'
    ? firstNonEmpty(env, ['https_proxy', 'HTTPS_PROXY', 'all_proxy', 'ALL_PROXY'])
    : parsed.protocol === 'http:'
      ? firstNonEmpty(env, ['http_proxy', 'HTTP_PROXY', 'all_proxy', 'ALL_PROXY'])
      : firstNonEmpty(env, ['all_proxy', 'ALL_PROXY'])

  if (!raw) return undefined
  return normalizeProxyUrl(raw)
}

function firstNonEmpty(env: Environment, keys: readonly string[]): string | undefined {
  for (const key of keys) {
    const value = env[key]?.trim()
    if (value) return value
  }
  return undefined
}

function normalizeProxyUrl(raw: string): string {
  return /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw) ? raw : `http://${raw}`
}

function stripBrackets(host: string): string {
  return host.startsWith('[') && host.endsWith(']') ? host.slice(1, -1) : host
}

function parseNoProxyEntry(entry: string): { host: string; port?: string } | undefined {
  if (entry.startsWith('[')) {
    const closing = entry.indexOf(']')
    if (closing === -1) return undefined
    const host = entry.slice(1, closing)
    const rest = entry.slice(closing + 1)
    if (rest.startsWith(':')) {
      return { host, port: rest.slice(1) }
    }
    return { host }
  }

  const firstColon = entry.indexOf(':')
  const lastColon = entry.lastIndexOf(':')
  if (firstColon !== -1 && firstColon === lastColon) {
    return { host: entry.slice(0, firstColon), port: entry.slice(firstColon + 1) }
  }

  return { host: entry }
}

function matchesNoProxy(parsed: URL, noProxy: string): boolean {
  const hostname = stripBrackets(parsed.hostname).replace(/\.$/, '').toLowerCase()
  const port = parsed.port || (parsed.protocol === 'https:' ? '443' : parsed.protocol === 'http:' ? '80' : '')

  for (const rawEntry of noProxy.split(',')) {
    const entry = rawEntry.trim()
    if (!entry) continue
    if (entry === '*') return true

    const target = parseNoProxyEntry(entry)
    if (!target) continue
    if (target.port !== undefined && target.port !== port) continue

    const entryHost = target.host.replace(/\.$/, '').toLowerCase()
    if (!entryHost) continue

    if (entryHost.startsWith('*.')) {
      const suffix = entryHost.slice(1)
      if (hostname.endsWith(suffix)) return true
      continue
    }

    const domain = entryHost.replace(/^\.+/, '')
    if (!domain) continue
    if (hostname === domain || hostname.endsWith(`.${domain}`)) return true
  }

  return false
}
