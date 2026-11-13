import { readFile } from 'node:fs/promises'
import os from 'node:os'
import { join } from 'node:path'

import type { EgressFamily, EgressHostSample } from '@matou/contracts'

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
