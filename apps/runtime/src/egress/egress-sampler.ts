import { readFile } from 'node:fs/promises'
import os from 'node:os'
import { join } from 'node:path'

import type { EgressFamily, EgressHostError, EgressHostSample } from '@matou/contracts'

import { EGRESS_HOSTS, type EgressSample } from './egress-evaluator'
import {
  canReachIpv6Directly,
  detectTun,
  hasGlobalIpv6,
  readSystemProxy,
  type Interfaces
} from './local-network'
import { hasProxyEnvironment, resolveProxyForUrl, type Environment } from './proxy-route'
import { probeTrace } from './trace-probe'

const DEFAULT_CLAUDE_USER_SETTINGS_PATH = join(os.homedir(), '.claude', 'settings.json')

const CLAUDE_PROXY_ENV_KEYS = [
  'HTTPS_PROXY',
  'https_proxy',
  'HTTP_PROXY',
  'http_proxy',
  'ALL_PROXY',
  'all_proxy',
  'NO_PROXY',
  'no_proxy'
] as const

export interface EgressSamplerInput {
  baseEnv: Environment
  claudeUserSettingsPath?: string
  agentTimeZoneMainland(): boolean
  probeTrace?: typeof probeTrace
  readSystemProxy?: typeof readSystemProxy
  networkInterfaces?: () => Interfaces
  canReachIpv6Directly?: typeof canReachIpv6Directly
}

export interface EgressSampler {
  sampleFull(): Promise<EgressSample>
  sampleLight(family: EgressFamily): Promise<EgressHostSample>
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isHostError(value: unknown): value is EgressHostError {
  return value === 'timeout' || value === 'unreachable' || value === 'invalid'
}

function resolveEgressFixturePath(env: Environment): string | undefined {
  if (env.MATOU_E2E !== '1') return undefined
  const fixturePath = env.MATOU_E2E_EGRESS_FIXTURE?.trim()
  return fixturePath ? fixturePath : undefined
}

function parseFixtureHostSample(host: string, raw: unknown): EgressHostSample {
  if (!isPlainObject(raw)) return { host, error: 'timeout' }
  if (isHostError(raw.error)) return { host, error: raw.error }
  const ip = typeof raw.ip === 'string' && raw.ip.length > 0 ? raw.ip : undefined
  const country = typeof raw.country === 'string' && raw.country.length > 0 ? raw.country : undefined
  if (!ip && !country) return { host, error: 'timeout' }
  return {
    host,
    ...(ip ? { ip } : {}),
    ...(country ? { country } : {})
  }
}

async function readFixtureData(fixturePath: string): Promise<Record<string, unknown> | undefined> {
  try {
    const raw = await readFile(fixturePath, 'utf8')
    const parsed: unknown = JSON.parse(raw)
    return isPlainObject(parsed) ? parsed : undefined
  } catch {
    return undefined
  }
}

async function sampleFullFromFixture(
  fixturePath: string,
  baseEnv: Environment,
  settingsPath: string,
  agentTimeZoneMainland: () => boolean
): Promise<EgressSample> {
  const [fixture, anthropicEnv] = await Promise.all([
    readFixtureData(fixturePath),
    agentProxyEnvironment('anthropic', baseEnv, settingsPath)
  ])
  const anthropicRaw = fixture && isPlainObject(fixture.anthropic) ? fixture.anthropic : undefined
  const openaiRaw = fixture && isPlainObject(fixture.openai) ? fixture.openai : undefined
  const browserLogin =
    fixture && fixture.browserLogin !== null && fixture.browserLogin !== undefined
      ? parseFixtureHostSample(EGRESS_HOSTS.anthropic.login, fixture.browserLogin)
      : undefined

  return {
    hosts: {
      anthropic: {
        api: parseFixtureHostSample(EGRESS_HOSTS.anthropic.api, anthropicRaw?.api),
        login: parseFixtureHostSample(EGRESS_HOSTS.anthropic.login, anthropicRaw?.login)
      },
      openai: {
        api: parseFixtureHostSample(EGRESS_HOSTS.openai.api, openaiRaw?.api),
        login: parseFixtureHostSample(EGRESS_HOSTS.openai.login, openaiRaw?.login)
      }
    },
    ...(browserLogin ? { browserLogin } : {}),
    agentHasProxyEnv: hasProxyEnvironment(anthropicEnv),
    systemProxyEnabled: fixture?.systemProxyEnabled === true,
    tunDetected: fixture?.tunDetected === true,
    ipv6Bypass: fixture?.ipv6Bypass === true,
    agentTimeZoneMainland: agentTimeZoneMainland()
  }
}

async function sampleLightFromFixture(
  fixturePath: string,
  family: EgressFamily
): Promise<EgressHostSample> {
  const fixture = await readFixtureData(fixturePath)
  const familyRaw = fixture && isPlainObject(fixture[family]) ? fixture[family] : undefined
  return parseFixtureHostSample(EGRESS_HOSTS[family].api, familyRaw?.api)
}

/** Resolves the effective proxy environment for a provider family, merging Claude user settings when applicable. */
export async function agentProxyEnvironment(
  family: EgressFamily,
  base: Environment,
  claudeUserSettingsPath = DEFAULT_CLAUDE_USER_SETTINGS_PATH
): Promise<Environment> {
  const merged: Environment = { ...base }
  if (family !== 'anthropic') return merged

  let raw: string
  try {
    raw = await readFile(claudeUserSettingsPath, 'utf8')
  } catch {
    return merged
  }

  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return merged
  }

  if (!isPlainObject(parsed) || !isPlainObject(parsed.env)) {
    return merged
  }

  for (const key of CLAUDE_PROXY_ENV_KEYS) {
    const value = parsed.env[key]
    if (typeof value === 'string') {
      merged[key] = value
    }
  }

  return merged
}

/** Creates the full and light egress samplers used by EgressMonitor. */
export function createEgressSampler(input: EgressSamplerInput): EgressSampler {
  const settingsPath = input.claudeUserSettingsPath ?? DEFAULT_CLAUDE_USER_SETTINGS_PATH
  const probe = input.probeTrace ?? probeTrace
  const readSysProxy = input.readSystemProxy ?? readSystemProxy
  const getInterfaces = input.networkInterfaces ?? os.networkInterfaces
  const checkIpv6Directly = input.canReachIpv6Directly ?? canReachIpv6Directly
  const fixturePath = resolveEgressFixturePath(input.baseEnv)

  if (fixturePath !== undefined) {
    return {
      sampleFull: () =>
        sampleFullFromFixture(fixturePath, input.baseEnv, settingsPath, input.agentTimeZoneMainland),
      sampleLight: (family: EgressFamily) => sampleLightFromFixture(fixturePath, family)
    }
  }

  return {
    async sampleFull(): Promise<EgressSample> {
      const interfaces = getInterfaces()
      const ipv6Promise = hasGlobalIpv6(interfaces)
        ? checkIpv6Directly()
        : Promise.resolve(false)

      const [anthropicEnv, openaiEnv] = await Promise.all([
        agentProxyEnvironment('anthropic', input.baseEnv, settingsPath),
        agentProxyEnvironment('openai', input.baseEnv, settingsPath)
      ])

      const probeHost = (host: string, env: Environment) =>
        probe(host, resolveProxyForUrl(env, `https://${host}/cdn-cgi/trace`))

      const [
        anthropicApi,
        anthropicLogin,
        openaiApi,
        openaiLogin,
        browserResult,
        ipv6Bypass
      ] = await Promise.all([
        probeHost(EGRESS_HOSTS.anthropic.api, anthropicEnv),
        probeHost(EGRESS_HOSTS.anthropic.login, anthropicEnv),
        probeHost(EGRESS_HOSTS.openai.api, openaiEnv),
        probeHost(EGRESS_HOSTS.openai.login, openaiEnv),
        readSysProxy().then(async (systemProxy) => ({
          systemProxy,
          browserLogin: systemProxy
            ? await probe(EGRESS_HOSTS.anthropic.login, systemProxy)
            : undefined
        })),
        ipv6Promise
      ])

      return {
        hosts: {
          anthropic: { api: anthropicApi, login: anthropicLogin },
          openai: { api: openaiApi, login: openaiLogin }
        },
        ...(browserResult.browserLogin ? { browserLogin: browserResult.browserLogin } : {}),
        agentHasProxyEnv: hasProxyEnvironment(anthropicEnv),
        systemProxyEnabled: browserResult.systemProxy !== undefined,
        tunDetected: detectTun(interfaces),
        ipv6Bypass,
        agentTimeZoneMainland: input.agentTimeZoneMainland()
      }
    },

    async sampleLight(family: EgressFamily): Promise<EgressHostSample> {
      const env = await agentProxyEnvironment(family, input.baseEnv, settingsPath)
      const host = EGRESS_HOSTS[family].api
      const proxyUrl = resolveProxyForUrl(env, `https://${host}/cdn-cgi/trace`)
      return probe(host, proxyUrl)
    }
  }
}
