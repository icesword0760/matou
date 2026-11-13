import type { NetworkInterfaceInfo } from 'node:os'
import { describe, expect, it } from 'vitest'

import {
  detectTun,
  hasGlobalIpv6,
  networkSignature,
  parseScutilProxy,
  readSystemProxy,
  type Interfaces
} from './local-network'

function ipv4(address: string, internal = false): NetworkInterfaceInfo {
  return {
    address,
    netmask: '255.255.255.0',
    family: 'IPv4',
    mac: '00:00:00:00:00:00',
    internal,
    cidr: `${address}/24`
  }
}

function ipv6(address: string, internal = false): NetworkInterfaceInfo {
  return {
    address,
    netmask: 'ffff:ffff:ffff:ffff::',
    family: 'IPv6',
    mac: '00:00:00:00:00:00',
    internal,
    cidr: `${address}/64`,
    scopeid: 0
  }
}

describe('parseScutilProxy', () => {
  it('parses enabled HTTPS proxy', () => {
    const text = `<dictionary> {
  HTTPEnable : 0
  HTTPSEnable : 1
  HTTPSPort : 7890
  HTTPSProxy : 127.0.0.1
  SOCKSEnable : 0
}`
    expect(parseScutilProxy(text)).toBe('http://127.0.0.1:7890')
  })

  it('parses enabled SOCKS proxy when only SOCKS is enabled', () => {
    const text = `<dictionary> {
  HTTPEnable : 0
  HTTPSEnable : 0
  SOCKSEnable : 1
  SOCKSPort : 7891
  SOCKSProxy : 127.0.0.1
}`
    expect(parseScutilProxy(text)).toBe('socks5://127.0.0.1:7891')
  })

  it('parses enabled HTTP proxy when only HTTP is enabled', () => {
    const text = `<dictionary> {
  HTTPEnable : 1
  HTTPPort : 8080
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 0
  SOCKSEnable : 0
}`
    expect(parseScutilProxy(text)).toBe('http://127.0.0.1:8080')
  })

  it('returns undefined when all proxy flags are 0', () => {
    const text = `<dictionary> {
  HTTPEnable : 0
  HTTPSEnable : 0
  ProxyAutoConfigEnable : 0
  SOCKSEnable : 0
}`
    expect(parseScutilProxy(text)).toBeUndefined()
  })
})

describe('readSystemProxy', () => {
  it('returns undefined on non-darwin platforms', async () => {
    await expect(readSystemProxy('linux')).resolves.toBeUndefined()
    await expect(readSystemProxy('win32')).resolves.toBeUndefined()
  })
})

describe('detectTun', () => {
  it('returns true when a non-internal interface has an IPv4 in 198.18.0.0/15', () => {
    const interfaces: Interfaces = {
      en0: [ipv4('192.168.1.5')],
      utun4: [ipv4('198.18.0.1')]
    }
    expect(detectTun(interfaces)).toBe(true)

    expect(detectTun({
      utun4: [ipv4('198.19.255.254')]
    })).toBe(true)
  })

  it('returns false when only private LAN or internal 198.18.x.x addresses exist', () => {
    expect(detectTun({
      en0: [ipv4('192.168.1.5')],
      lo0: [ipv4('127.0.0.1', true), ipv4('198.18.0.1', true)]
    })).toBe(false)

    expect(detectTun({
      en0: [ipv4('198.20.0.1')]
    })).toBe(false)
  })
})

describe('hasGlobalIpv6', () => {
  it('ignores link-local, ULA, and internal IPv6 addresses', () => {
    expect(hasGlobalIpv6({
      lo0: [ipv6('::1', true), ipv6('2408:8207::1', true)],
      en0: [ipv6('fe80::1'), ipv6('fd00::1'), ipv6('fc00::1')]
    })).toBe(false)
  })

  it('returns true when a non-internal global IPv6 address is present', () => {
    expect(hasGlobalIpv6({
      en0: [ipv6('fe80::1'), ipv6('2408:8207:1234::1')]
    })).toBe(true)
  })
})

describe('networkSignature', () => {
  it('produces identical signatures regardless of interface or address order', () => {
    const a: Interfaces = {
      en0: [ipv4('192.168.1.5'), ipv6('fe80::1')],
      utun4: [ipv4('198.18.0.1')]
    }
    const b: Interfaces = {
      utun4: [ipv4('198.18.0.1')],
      en0: [ipv6('fe80::1'), ipv4('192.168.1.5')]
    }
    expect(networkSignature(a)).toBe(networkSignature(b))
  })

  it('changes signature when addresses are added or removed', () => {
    const base: Interfaces = {
      en0: [ipv4('192.168.1.5')]
    }
    const added: Interfaces = {
      en0: [ipv4('192.168.1.5'), ipv6('2408:8207::1')]
    }
    const removed: Interfaces = {
      en0: []
    }
    expect(networkSignature(added)).not.toBe(networkSignature(base))
    expect(networkSignature(removed)).not.toBe(networkSignature(base))
  })
})
