import { execFile } from 'node:child_process'
import net from 'node:net'
import type { NetworkInterfaceInfo } from 'node:os'

export type Interfaces = NodeJS.Dict<NetworkInterfaceInfo[]>

const SCUTIL_TIMEOUT_MS = 3000
const IPV6_PROBE_HOST = '2606:4700:4700::1111'
const IPV6_PROBE_PORT = 443
const DEFAULT_IPV6_TIMEOUT_MS = 3000

export function parseScutilProxy(text: string): string | undefined {
  const entries = new Map<string, string>()

  for (const line of text.split(/\r?\n/)) {
    const match = /^\s*([A-Za-z0-9_]+)\s*:\s*(.+?)\s*$/.exec(line)
    if (!match) continue
    const [, key, value] = match
    if (!key || !value || value.startsWith('<')) continue
    entries.set(key, value)
  }

  if (entries.get('HTTPSEnable') === '1') {
    const url = formatProxyUrl('http', entries.get('HTTPSProxy'), entries.get('HTTPSPort'))
    if (url) return url
  }

  if (entries.get('HTTPEnable') === '1') {
    const url = formatProxyUrl('http', entries.get('HTTPProxy'), entries.get('HTTPPort'))
    if (url) return url
  }

  if (entries.get('SOCKSEnable') === '1') {
    const url = formatProxyUrl('socks5', entries.get('SOCKSProxy'), entries.get('SOCKSPort'))
    if (url) return url
  }

  return undefined
}

export function readSystemProxy(platform: NodeJS.Platform = process.platform): Promise<string | undefined> {
  if (platform !== 'darwin') {
    return Promise.resolve(undefined)
  }

  return new Promise((resolve) => {
    execFile('scutil', ['--proxy'], { timeout: SCUTIL_TIMEOUT_MS }, (error, stdout) => {
      if (error || !stdout) {
        resolve(undefined)
        return
      }
      resolve(parseScutilProxy(stdout))
    })
  })
}

export function detectTun(interfaces: Interfaces): boolean {
  for (const entries of Object.values(interfaces)) {
    if (!entries) continue
    for (const info of entries) {
      if (info.internal) continue
      if (isIpv4Family(info.family) && isBenchmarkCidr198_18(info.address)) {
        return true
      }
    }
  }
  return false
}

export function hasGlobalIpv6(interfaces: Interfaces): boolean {
  for (const entries of Object.values(interfaces)) {
    if (!entries) continue
    for (const info of entries) {
      if (info.internal) continue
      if (isIpv6Family(info.family) && isGlobalIpv6Address(info.address)) {
        return true
      }
    }
  }
  return false
}

export function canReachIpv6Directly(timeoutMs = DEFAULT_IPV6_TIMEOUT_MS): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false
    const socket = net.connect({ host: IPV6_PROBE_HOST, port: IPV6_PROBE_PORT })

    const finish = (reachable: boolean) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      socket.destroy()
      resolve(reachable)
    }

    const timer = setTimeout(() => finish(false), timeoutMs)
    socket.setTimeout(timeoutMs)
    socket.once('connect', () => finish(true))
    socket.once('timeout', () => finish(false))
    socket.once('error', () => finish(false))
  })
}

export function networkSignature(interfaces: Interfaces): string {
  const records: string[] = []

  for (const [name, entries] of Object.entries(interfaces)) {
    if (!entries) continue
    for (const info of entries) {
      records.push(`${name}|${info.family}|${info.address}|${info.netmask}|${info.internal ? 1 : 0}`)
    }
  }

  records.sort()
  return records.join('\n')
}

function formatProxyUrl(
  scheme: 'http' | 'socks5',
  host: string | undefined,
  port: string | undefined
): string | undefined {
  const trimmedHost = host?.trim()
  if (!trimmedHost) return undefined

  const hostPart = trimmedHost.includes(':') && !trimmedHost.startsWith('[')
    ? `[${trimmedHost}]`
    : trimmedHost
  const trimmedPort = port?.trim()
  if (trimmedPort && /^\d+$/.test(trimmedPort) && Number(trimmedPort) > 0) {
    return `${scheme}://${hostPart}:${trimmedPort}`
  }
  return `${scheme}://${hostPart}`
}

function isIpv4Family(family: NetworkInterfaceInfo['family']): boolean {
  return family === 'IPv4' || (family as unknown) === 4
}

function isIpv6Family(family: NetworkInterfaceInfo['family']): boolean {
  return family === 'IPv6' || (family as unknown) === 6
}

function isBenchmarkCidr198_18(address: string): boolean {
  if (!net.isIPv4(address)) return false
  const parts = address.split('.')
  if (parts.length !== 4) return false
  const first = Number(parts[0])
  const second = Number(parts[1])
  return first === 198 && (second === 18 || second === 19)
}

function isGlobalIpv6Address(address: string): boolean {
  const clean = address.split('%')[0] ?? ''
  if (!net.isIPv6(clean)) return false
  if (clean.startsWith('::')) return false

  const firstGroup = clean.split(':')[0]
  if (!firstGroup) return false
  const firstHextet = Number.parseInt(firstGroup, 16)
  if (Number.isNaN(firstHextet)) return false

  // Global unicast IPv6 addresses fall within 2000::/3 (first hextet 0x2000..0x3fff).
  return firstHextet >= 0x2000 && firstHextet <= 0x3fff
}
