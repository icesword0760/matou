import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { loadLoginShellProxyEnvironment, PROXY_ENVIRONMENT_KEYS, type ProxyEnvironment } from '../session/provider-command-environment'

const execFileAsync = promisify(execFile)
export interface RouterProxyResolution {
  source: 'shell' | 'system' | 'direct'
  proxyUrl?: string
  noProxy?: string
  providerBypass?: boolean
}
interface ProxyDependencies {
  loadShell?(environment: NodeJS.ProcessEnv): Promise<ProxyEnvironment>
  readSystem?(): Promise<string>
  platform?: NodeJS.Platform
}

/** Resolve here, before writing the private router config, not in renderer.
 * The pinned Codex uTLS path uses proxy.Direct when config proxy-url is empty;
 * forwarding HTTP(S)_PROXY alone therefore cannot cover both providers.
 */
export async function resolveRouterProxy(environment: NodeJS.ProcessEnv = process.env, dependencies: ProxyDependencies = {}): Promise<RouterProxyResolution> {
  const shell = await (dependencies.loadShell ?? loadLoginShellProxyEnvironment)(environment)
  const noProxy = shell.NO_PROXY || shell.no_proxy
  // Decision #7: one explicit proxy for the engine. If NO_PROXY covers a
  // provider, omit proxy-url for the whole engine; other exclusions are not
  // forwarded, since fixed v8's explicit proxy does not apply NO_PROXY.
  if (noProxy && bypassesProvider(noProxy)) return { source: 'direct', noProxy, providerBypass: true }
  for (const name of PROXY_ENVIRONMENT_KEYS.slice(0, 6)) {
    if (shell[name]) return { source: 'shell', proxyUrl: validatedProxyUrl(shell[name]!), ...(noProxy ? { noProxy } : {}) }
  }
  if ((dependencies.platform ?? process.platform) === 'darwin') {
    const system = parseSystemProxy(await (dependencies.readSystem ?? readSystemProxy)())
    if (system) return { source: 'system', proxyUrl: system, ...(noProxy ? { noProxy } : {}) }
  }
  return { source: 'direct', ...(noProxy ? { noProxy } : {}) }
}

export function bypassesProvider(noProxy: string): boolean {
  const fixedHosts = ['chatgpt.com', 'auth.openai.com', 'accounts.google.com']
  return noProxy.split(',').some(item => {
    let entry = item.trim().toLowerCase()
    if (entry === '*') return true
    const port = /:(\d+)$/.exec(entry)
    if (port) {
      if (port[1] !== '443') return false
      entry = entry.slice(0, port.index)
    }
    const subdomainsOnly = entry.startsWith('.') || entry.startsWith('*.')
    const domain = entry.replace(/^\*?\./, '')
    if (!domain || !/^[a-z0-9.-]+$/.test(domain)) return false
    return fixedHosts.some(host => (!subdomainsOnly && host === domain) || host.endsWith(`.${domain}`)) ||
      domain === 'googleapis.com' || domain.endsWith('.googleapis.com')
  })
}

async function readSystemProxy(): Promise<string> {
  try {
    return (await execFileAsync('/usr/sbin/scutil', ['--proxy'], { encoding: 'utf8', timeout: 2_000, maxBuffer: 64 * 1024 })).stdout
  } catch { return '' }
}

export function parseSystemProxy(output: string): string | undefined {
  // scutil can also report interface-scoped dictionaries. Only the effective
  // root dictionary is the fallback, never an arbitrary nested interface.
  const values: Record<string, string> = {}
  let depth = 0
  for (const line of output.split('\n')) {
    if (depth === 1) {
      const field = /^\s*(HTTPSEnable|HTTPSProxy|HTTPSPort|SOCKSEnable|SOCKSProxy|SOCKSPort)\s*:\s*(.*?)\s*$/.exec(line)
      if (field) values[field[1]!] = field[2]!
    }
    depth += (line.match(/\{/g)?.length ?? 0) - (line.match(/\}/g)?.length ?? 0)
  }
  for (const [prefix, scheme] of [['HTTPS', 'http'], ['SOCKS', 'socks5']] as const) {
    if (values[`${prefix}Enable`] !== '1') continue
    const host = values[`${prefix}Proxy`]
    const port = Number(values[`${prefix}Port`])
    if (!host || /[\s/@?#]/.test(host) || !Number.isInteger(port) || port < 1 || port > 65535) throw new Error('System network proxy configuration is invalid')
    const authority = host.includes(':') && !host.startsWith('[') ? `[${host}]` : host
    return validatedProxyUrl(`${scheme}://${authority}:${port}`)
  }
  return undefined
}

function validatedProxyUrl(raw: string): string {
  const value = raw.includes('://') ? raw : `http://${raw}`
  try {
    const url = new URL(value)
    if (['http:', 'https:', 'socks5:', 'socks5h:'].includes(url.protocol) && url.hostname && (!url.port || Number(url.port) > 0)) return value
  } catch { /* Do not include a potentially credential-bearing proxy URL in errors. */ }
  throw new Error('Network proxy configuration is invalid')
}
