import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import type { NetworkInterfaceInfo } from 'node:os'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import type { EgressHostSample } from '@matou/contracts'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { agentProxyEnvironment, createEgressSampler } from './egress-sampler'

let root: string

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-egress-sampler-'))
})

afterEach(async () => {
  await rm(root, { recursive: true, force: true })
})

describe('agentProxyEnvironment', () => {
  it('merges proxy keys from Claude user settings onto base for anthropic but not openai', async () => {
    const settingsPath = join(root, 'settings.json')
    await writeFile(settingsPath, JSON.stringify({
      env: {
        HTTPS_PROXY: 'http://127.0.0.1:7890',
        NO_PROXY: 'localhost,.internal',
        UNRELATED_KEY: 'ignored'
      }
    }))
    const base = {
      HTTP_PROXY: 'http://127.0.0.1:1080',
      HTTPS_PROXY: 'http://127.0.0.1:1080',
      PATH: '/usr/bin'
    }

    const anthropicEnv = await agentProxyEnvironment('anthropic', base, settingsPath)
    const openaiEnv = await agentProxyEnvironment('openai', base, settingsPath)

    expect(anthropicEnv).toEqual({
      HTTP_PROXY: 'http://127.0.0.1:1080',
      HTTPS_PROXY: 'http://127.0.0.1:7890',
      NO_PROXY: 'localhost,.internal',
      PATH: '/usr/bin'
    })
    expect(anthropicEnv.UNRELATED_KEY).toBeUndefined()
    expect(openaiEnv).toEqual(base)
    expect(base.HTTPS_PROXY).toBe('http://127.0.0.1:1080')
  })

  it('falls back to base when settings file is missing, malformed, or has non-object env', async () => {
    const base = { HTTPS_PROXY: 'http://127.0.0.1:8888' }

    await expect(
      agentProxyEnvironment('anthropic', base, join(root, 'missing.json'))
    ).resolves.toEqual(base)

    const invalidJsonPath = join(root, 'invalid.json')
    await writeFile(invalidJsonPath, '{not valid json')
    await expect(
      agentProxyEnvironment('anthropic', base, invalidJsonPath)
    ).resolves.toEqual(base)

    const arrayEnvPath = join(root, 'array-env.json')
    await writeFile(arrayEnvPath, JSON.stringify({ env: ['HTTPS_PROXY=bad'] }))
    await expect(
      agentProxyEnvironment('anthropic', base, arrayEnvPath)
    ).resolves.toEqual(base)
  })
})

describe('createEgressSampler', () => {
  it('probes all four provider hosts concurrently and adds browserLogin when system proxy is enabled', async () => {
    const settingsPath = join(root, 'settings.json')
    await writeFile(settingsPath, JSON.stringify({
      env: {
        HTTPS_PROXY: 'http://127.0.0.1:7890'
      }
    }))

    const probeCalls: Array<{ host: string; proxyUrl: string | undefined }> = []
    const probeTrace = vi.fn(async (host: string, proxyUrl: string | undefined): Promise<EgressHostSample> => {
      probeCalls.push({ host, proxyUrl })
      if (proxyUrl === 'http://127.0.0.1:9090') {
        return { host, ip: '198.51.100.99', country: 'JP' }
      }
      if (host.includes('anthropic') || host === 'claude.ai') {
        return { host, ip: '203.0.113.10', country: 'US' }
      }
      return { host, ip: '198.51.100.20', country: 'SG' }
    })
    const readSystemProxy = vi.fn(async () => 'http://127.0.0.1:9090')
    const canReachIpv6Directly = vi.fn(async () => false)
    const interfaces: NodeJS.Dict<NetworkInterfaceInfo[]> = {
      utun3: [ipv4Info('198.18.0.1', '255.254.0.0', false)]
    }

    const sampler = createEgressSampler({
      baseEnv: {},
      claudeUserSettingsPath: settingsPath,
      agentTimeZoneMainland: () => true,
      probeTrace,
      readSystemProxy,
      networkInterfaces: () => interfaces,
      canReachIpv6Directly
    })

    const sample = await sampler.sampleFull()

    expect(probeCalls).toEqual([
      { host: 'api.anthropic.com', proxyUrl: 'http://127.0.0.1:7890' },
      { host: 'claude.ai', proxyUrl: 'http://127.0.0.1:7890' },
      { host: 'api.openai.com', proxyUrl: undefined },
      { host: 'chatgpt.com', proxyUrl: undefined },
      { host: 'claude.ai', proxyUrl: 'http://127.0.0.1:9090' }
    ])
    expect(sample).toEqual({
      hosts: {
        anthropic: {
          api: { host: 'api.anthropic.com', ip: '203.0.113.10', country: 'US' },
          login: { host: 'claude.ai', ip: '203.0.113.10', country: 'US' }
        },
        openai: {
          api: { host: 'api.openai.com', ip: '198.51.100.20', country: 'SG' },
          login: { host: 'chatgpt.com', ip: '198.51.100.20', country: 'SG' }
        }
      },
      browserLogin: { host: 'claude.ai', ip: '198.51.100.99', country: 'JP' },
      agentHasProxyEnv: true,
      systemProxyEnabled: true,
      tunDetected: true,
      ipv6Bypass: false,
      agentTimeZoneMainland: true
    })
    expect(canReachIpv6Directly).not.toHaveBeenCalled()
  })

  it('skips browserLogin when system proxy is disabled and checks IPv6 bypass only with global IPv6', async () => {
    const probeTrace = vi.fn(async (host: string): Promise<EgressHostSample> => ({
      host,
      ip: '203.0.113.1',
      country: 'US'
    }))
    const canReachIpv6Directly = vi.fn(async () => true)
    const interfaces: NodeJS.Dict<NetworkInterfaceInfo[]> = {
      en0: [ipv6Info('2408:8207:1::10', false)]
    }

    const sampler = createEgressSampler({
      baseEnv: {},
      claudeUserSettingsPath: join(root, 'missing.json'),
      agentTimeZoneMainland: () => false,
      probeTrace,
      readSystemProxy: async () => undefined,
      networkInterfaces: () => interfaces,
      canReachIpv6Directly
    })

    const sample = await sampler.sampleFull()

    expect(probeTrace).toHaveBeenCalledTimes(4)
    expect(sample.browserLogin).toBeUndefined()
    expect(sample.systemProxyEnabled).toBe(false)
    expect(sample.agentHasProxyEnv).toBe(false)
    expect(sample.tunDetected).toBe(false)
    expect(canReachIpv6Directly).toHaveBeenCalledTimes(1)
    expect(sample.ipv6Bypass).toBe(true)
    expect(sample.agentTimeZoneMainland).toBe(false)
  })

  it('sampleLight probes only the requested family API host using that family proxy environment', async () => {
    const settingsPath = join(root, 'settings.json')
    await writeFile(settingsPath, JSON.stringify({
      env: {
        HTTPS_PROXY: 'socks5://127.0.0.1:7891'
      }
    }))

    const probeTrace = vi.fn(async (host: string, proxyUrl: string | undefined): Promise<EgressHostSample> => ({
      host,
      ip: proxyUrl === 'socks5://127.0.0.1:7891' ? '198.51.100.1' : '203.0.113.2',
      country: 'US'
    }))

    const sampler = createEgressSampler({
      baseEnv: { HTTPS_PROXY: 'http://127.0.0.1:8080' },
      claudeUserSettingsPath: settingsPath,
      agentTimeZoneMainland: () => false,
      probeTrace
    })

    const anthropicSample = await sampler.sampleLight('anthropic')
    const openaiSample = await sampler.sampleLight('openai')

    expect(probeTrace).toHaveBeenNthCalledWith(1, 'api.anthropic.com', 'socks5://127.0.0.1:7891')
    expect(probeTrace).toHaveBeenNthCalledWith(2, 'api.openai.com', 'http://127.0.0.1:8080')
    expect(anthropicSample).toEqual({ host: 'api.anthropic.com', ip: '198.51.100.1', country: 'US' })
    expect(openaiSample).toEqual({ host: 'api.openai.com', ip: '203.0.113.2', country: 'US' })
  })

  it('keeps real probing behavior when MATOU_E2E_EGRESS_FIXTURE is unset or MATOU_E2E is not 1', async () => {
    const fixturePath = join(root, 'ignored-fixture.json')
    await writeFile(fixturePath, JSON.stringify({
      anthropic: {
        api: { ip: '223.5.5.5', country: 'CN' },
        login: { ip: '223.5.5.5', country: 'CN' }
      },
      openai: {
        api: { ip: '223.5.5.5', country: 'CN' },
        login: { ip: '223.5.5.5', country: 'CN' }
      },
      browserLogin: null,
      systemProxyEnabled: false,
      tunDetected: true,
      ipv6Bypass: false
    }))

    const probeTrace = vi.fn(async (host: string): Promise<EgressHostSample> => ({
      host,
      ip: '169.128.10.1',
      country: 'US'
    }))

    for (const baseEnv of [
      {},
      { MATOU_E2E: '0', MATOU_E2E_EGRESS_FIXTURE: fixturePath },
      { MATOU_E2E_EGRESS_FIXTURE: fixturePath }
    ]) {
      probeTrace.mockClear()
      const sampler = createEgressSampler({
        baseEnv,
        claudeUserSettingsPath: join(root, 'missing.json'),
        agentTimeZoneMainland: () => false,
        probeTrace,
        readSystemProxy: async () => undefined,
        networkInterfaces: () => ({}),
        canReachIpv6Directly: async () => false
      })

      const full = await sampler.sampleFull()
      const light = await sampler.sampleLight('anthropic')

      expect(probeTrace).toHaveBeenCalledTimes(5)
      expect(full.hosts.anthropic.api).toEqual({ host: 'api.anthropic.com', ip: '169.128.10.1', country: 'US' })
      expect(light).toEqual({ host: 'api.anthropic.com', ip: '169.128.10.1', country: 'US' })
    }
  })

  it('reads MATOU_E2E_EGRESS_FIXTURE on every call without probing when MATOU_E2E=1', async () => {
    const fixturePath = join(root, 'egress-fixture.json')
    const settingsPath = join(root, 'settings.json')
    await writeFile(settingsPath, JSON.stringify({
      env: { HTTPS_PROXY: 'http://127.0.0.1:7890' }
    }))
    await writeFile(fixturePath, JSON.stringify({
      anthropic: {
        api: { ip: '169.128.10.1', country: 'US' },
        login: { ip: '169.128.10.1', country: 'US' }
      },
      openai: {
        api: { ip: '169.128.10.1', country: 'US' },
        login: { ip: '169.128.10.1', country: 'US' }
      },
      browserLogin: null,
      systemProxyEnabled: false,
      tunDetected: true,
      ipv6Bypass: false
    }))

    let mainlandTz = false
    const probeTrace = vi.fn()
    const readSystemProxy = vi.fn()
    const networkInterfaces = vi.fn()
    const canReachIpv6Directly = vi.fn()

    const sampler = createEgressSampler({
      baseEnv: { MATOU_E2E: '1', MATOU_E2E_EGRESS_FIXTURE: fixturePath },
      claudeUserSettingsPath: settingsPath,
      agentTimeZoneMainland: () => mainlandTz,
      probeTrace,
      readSystemProxy,
      networkInterfaces,
      canReachIpv6Directly
    })

    const initial = await sampler.sampleFull()
    expect(initial).toEqual({
      hosts: {
        anthropic: {
          api: { host: 'api.anthropic.com', ip: '169.128.10.1', country: 'US' },
          login: { host: 'claude.ai', ip: '169.128.10.1', country: 'US' }
        },
        openai: {
          api: { host: 'api.openai.com', ip: '169.128.10.1', country: 'US' },
          login: { host: 'chatgpt.com', ip: '169.128.10.1', country: 'US' }
        }
      },
      agentHasProxyEnv: true,
      systemProxyEnabled: false,
      tunDetected: true,
      ipv6Bypass: false,
      agentTimeZoneMainland: false
    })
    expect(await sampler.sampleLight('anthropic')).toEqual({
      host: 'api.anthropic.com',
      ip: '169.128.10.1',
      country: 'US'
    })

    mainlandTz = true
    await writeFile(fixturePath, JSON.stringify({
      anthropic: {
        api: { ip: '223.5.5.5', country: 'CN' },
        login: { error: 'timeout' }
      },
      openai: {
        api: { ip: '169.128.10.2', country: 'JP' },
        login: { ip: '169.128.10.2', country: 'JP' }
      },
      browserLogin: { ip: '198.51.100.7', country: 'SG' },
      systemProxyEnabled: true,
      tunDetected: false,
      ipv6Bypass: true
    }))

    const updated = await sampler.sampleFull()
    expect(updated).toEqual({
      hosts: {
        anthropic: {
          api: { host: 'api.anthropic.com', ip: '223.5.5.5', country: 'CN' },
          login: { host: 'claude.ai', error: 'timeout' }
        },
        openai: {
          api: { host: 'api.openai.com', ip: '169.128.10.2', country: 'JP' },
          login: { host: 'chatgpt.com', ip: '169.128.10.2', country: 'JP' }
        }
      },
      browserLogin: { host: 'claude.ai', ip: '198.51.100.7', country: 'SG' },
      agentHasProxyEnv: true,
      systemProxyEnabled: true,
      tunDetected: false,
      ipv6Bypass: true,
      agentTimeZoneMainland: true
    })
    expect(await sampler.sampleLight('anthropic')).toEqual({
      host: 'api.anthropic.com',
      ip: '223.5.5.5',
      country: 'CN'
    })
    expect(probeTrace).not.toHaveBeenCalled()
    expect(readSystemProxy).not.toHaveBeenCalled()
    expect(networkInterfaces).not.toHaveBeenCalled()
    expect(canReachIpv6Directly).not.toHaveBeenCalled()
  })

  it('returns timeout for all hosts when MATOU_E2E_EGRESS_FIXTURE is missing or invalid JSON', async () => {
    const missingPath = join(root, 'does-not-exist.json')
    const missingSampler = createEgressSampler({
      baseEnv: { MATOU_E2E: '1', MATOU_E2E_EGRESS_FIXTURE: missingPath },
      claudeUserSettingsPath: join(root, 'missing-settings.json'),
      agentTimeZoneMainland: () => false
    })

    const missingFull = await missingSampler.sampleFull()
    expect(missingFull).toEqual({
      hosts: {
        anthropic: {
          api: { host: 'api.anthropic.com', error: 'timeout' },
          login: { host: 'claude.ai', error: 'timeout' }
        },
        openai: {
          api: { host: 'api.openai.com', error: 'timeout' },
          login: { host: 'chatgpt.com', error: 'timeout' }
        }
      },
      agentHasProxyEnv: false,
      systemProxyEnabled: false,
      tunDetected: false,
      ipv6Bypass: false,
      agentTimeZoneMainland: false
    })
    expect(await missingSampler.sampleLight('openai')).toEqual({
      host: 'api.openai.com',
      error: 'timeout'
    })

    const invalidPath = join(root, 'invalid-fixture.json')
    await writeFile(invalidPath, '{broken json')
    const invalidSampler = createEgressSampler({
      baseEnv: { MATOU_E2E: '1', MATOU_E2E_EGRESS_FIXTURE: invalidPath },
      claudeUserSettingsPath: join(root, 'missing-settings.json'),
      agentTimeZoneMainland: () => false
    })

    expect((await invalidSampler.sampleFull()).hosts).toEqual({
      anthropic: {
        api: { host: 'api.anthropic.com', error: 'timeout' },
        login: { host: 'claude.ai', error: 'timeout' }
      },
      openai: {
        api: { host: 'api.openai.com', error: 'timeout' },
        login: { host: 'chatgpt.com', error: 'timeout' }
      }
    })
    expect(await invalidSampler.sampleLight('anthropic')).toEqual({
      host: 'api.anthropic.com',
      error: 'timeout'
    })
  })
})

function ipv4Info(address: string, netmask: string, internal: boolean): NetworkInterfaceInfo {
  return {
    address,
    netmask,
    family: 'IPv4',
    mac: '00:00:00:00:00:00',
    internal,
    cidr: `${address}/15`
  }
}

function ipv6Info(address: string, internal: boolean): NetworkInterfaceInfo {
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
