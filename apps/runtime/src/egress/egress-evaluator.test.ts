import { describe, expect, it } from 'vitest'

import { EGRESS_HOSTS, evaluateEgress, type EgressSample } from './egress-evaluator'

function makeBaseSample(overrides: Partial<EgressSample> = {}): EgressSample {
  return {
    hosts: {
      anthropic: {
        api: { host: EGRESS_HOSTS.anthropic.api, ip: '169.128.1.1', country: 'US' },
        login: { host: EGRESS_HOSTS.anthropic.login, ip: '169.128.1.1', country: 'US' }
      },
      openai: {
        api: { host: EGRESS_HOSTS.openai.api, ip: '169.128.1.1', country: 'US' },
        login: { host: EGRESS_HOSTS.openai.login, ip: '169.128.1.1', country: 'US' }
      }
    },
    agentHasProxyEnv: true,
    systemProxyEnabled: false,
    tunDetected: false,
    ipv6Bypass: false,
    agentTimeZoneMainland: false,
    ...overrides
  }
}

describe('EGRESS_HOSTS', () => {
  it('defines the expected provider API and login hosts', () => {
    expect(EGRESS_HOSTS).toEqual({
      anthropic: { api: 'api.anthropic.com', login: 'claude.ai' },
      openai: { api: 'api.openai.com', login: 'chatgpt.com' }
    })
  })
})

describe('evaluateEgress', () => {
  it('returns ok when all exits are non-mainland US and consistent', () => {
    const result = evaluateEgress(makeBaseSample(), { mode: 'country', baselines: {} })

    expect(result.state).toBe('ok')
    expect(result.families).toEqual([
      { family: 'anthropic', state: 'ok', exit: { ip: '169.128.1.1', country: 'US' } },
      { family: 'openai', state: 'ok', exit: { ip: '169.128.1.1', country: 'US' } }
    ])
  })

  it('marks only anthropic abnormal with mainland violation when anthropic api is CN and openai is US', () => {
    const sample = makeBaseSample()
    sample.hosts.anthropic.api = { host: 'api.anthropic.com', ip: '220.181.1.1', country: 'CN' }

    const result = evaluateEgress(sample, { mode: 'country', baselines: {} })

    expect(result.state).toBe('abnormal')
    expect(result.families.find((f) => f.family === 'anthropic')).toMatchObject({
      family: 'anthropic',
      state: 'abnormal',
      violation: 'mainland',
      exit: { ip: '220.181.1.1', country: 'CN' }
    })
    expect(result.families.find((f) => f.family === 'openai')).toMatchObject({
      family: 'openai',
      state: 'ok'
    })
  })

  it('does not treat HK, MO, or TW as mainland China', () => {
    const sample = makeBaseSample()
    sample.hosts.anthropic.api = { host: 'api.anthropic.com', ip: '1.2.3.4', country: 'HK' }
    sample.hosts.anthropic.login = { host: 'claude.ai', ip: '1.2.3.4', country: 'HK' }
    sample.hosts.openai.api = { host: 'api.openai.com', ip: '5.6.7.8', country: 'TW' }
    sample.hosts.openai.login = { host: 'chatgpt.com', ip: '5.6.7.8', country: 'TW' }

    const result = evaluateEgress(sample, { mode: 'country', baselines: {} })
    expect(result.state).toBe('ok')
  })

  it('warns on login-api-match without becoming abnormal when claude.ai and api.anthropic.com have different IPs', () => {
    const sample = makeBaseSample()
    sample.hosts.anthropic.login = { host: 'claude.ai', ip: '1.1.1.1', country: 'US' }
    sample.hosts.anthropic.api = { host: 'api.anthropic.com', ip: '2.2.2.2', country: 'US' }

    const result = evaluateEgress(sample, { mode: 'country', baselines: {} })

    expect(result.state).toBe('warning')
    const anthropic = result.families.find((f) => f.family === 'anthropic')
    expect(anthropic?.state).toBe('warning')
    expect(anthropic?.violation).toBeUndefined()
    expect(result.checks.find((c) => c.id === 'login-api-match' && c.family === 'anthropic')).toEqual({
      id: 'login-api-match',
      family: 'anthropic',
      result: 'warn',
      facts: { apiIp: '2.2.2.2', loginIp: '1.1.1.1' }
    })
  })

  it('treats different exits across providers as ok', () => {
    const sample = makeBaseSample()
    sample.hosts.openai.api = { host: 'api.openai.com', ip: '203.0.113.10', country: 'JP' }
    sample.hosts.openai.login = { host: 'chatgpt.com', ip: '203.0.113.10', country: 'JP' }

    const result = evaluateEgress(sample, { mode: 'country', baselines: {} })

    expect(result.state).toBe('ok')
    expect(result.families.every((f) => f.state === 'ok')).toBe(true)
  })

  it('flags exit-rule violations in country mode and ip mode', () => {
    const jpApiSample = makeBaseSample()
    jpApiSample.hosts.anthropic.api = { host: 'api.anthropic.com', ip: '203.0.113.20', country: 'JP' }
    jpApiSample.hosts.anthropic.login = { host: 'claude.ai', ip: '203.0.113.20', country: 'JP' }

    const countryResult = evaluateEgress(jpApiSample, {
      mode: 'country',
      baselines: { anthropic: { ip: '169.128.1.1', country: 'US' } }
    })
    expect(countryResult.state).toBe('abnormal')
    expect(countryResult.families.find((f) => f.family === 'anthropic')?.violation).toBe('rule')

    const sameCountryNewIpSample = makeBaseSample()
    sameCountryNewIpSample.hosts.anthropic.api = { host: 'api.anthropic.com', ip: '169.128.9.9', country: 'US' }
    sameCountryNewIpSample.hosts.anthropic.login = { host: 'claude.ai', ip: '169.128.9.9', country: 'US' }

    const ipResult = evaluateEgress(sameCountryNewIpSample, {
      mode: 'ip',
      baselines: { anthropic: { ip: '169.128.1.1', country: 'US' } }
    })
    expect(ipResult.state).toBe('abnormal')
    expect(ipResult.families.find((f) => f.family === 'anthropic')?.violation).toBe('rule')
  })

  it('marks both families unknown and overall state warning without violation when all hosts timeout', () => {
    const sample = makeBaseSample({
      hosts: {
        anthropic: {
          api: { host: 'api.anthropic.com', error: 'timeout' },
          login: { host: 'claude.ai', error: 'timeout' }
        },
        openai: {
          api: { host: 'api.openai.com', error: 'timeout' },
          login: { host: 'chatgpt.com', error: 'timeout' }
        }
      }
    })

    const result = evaluateEgress(sample, { mode: 'country', baselines: {} })

    expect(result.state).toBe('warning')
    expect(result.families).toEqual([
      { family: 'anthropic', state: 'unknown' },
      { family: 'openai', state: 'unknown' }
    ])
  })

  it('warns on agent-proxy-path when system proxy is enabled without proxy env or tun', () => {
    const sample = makeBaseSample({
      systemProxyEnabled: true,
      agentHasProxyEnv: false,
      tunDetected: false
    })

    const result = evaluateEgress(sample, { mode: 'country', baselines: {} })

    expect(result.state).toBe('warning')
    expect(result.checks.find((c) => c.id === 'agent-proxy-path')?.result).toBe('warn')

    const withTun = evaluateEgress(makeBaseSample({
      systemProxyEnabled: true,
      agentHasProxyEnv: false,
      tunDetected: true
    }), { mode: 'country', baselines: {} })
    expect(withTun.checks.find((c) => c.id === 'agent-proxy-path')?.result).toBe('pass')
  })

  it('reflects agentTimeZoneMainland in the time-zone check', () => {
    const warned = evaluateEgress(makeBaseSample({ agentTimeZoneMainland: true }), {
      mode: 'country',
      baselines: {}
    })
    expect(warned.state).toBe('warning')
    expect(warned.checks.find((c) => c.id === 'time-zone')?.result).toBe('warn')

    const passed = evaluateEgress(makeBaseSample({ agentTimeZoneMainland: false }), {
      mode: 'country',
      baselines: {}
    })
    expect(passed.checks.find((c) => c.id === 'time-zone')?.result).toBe('pass')
  })

  it('warns on browser-path and ipv6 when browser IP differs or IPv6 bypass is detected', () => {
    const sample = makeBaseSample({
      systemProxyEnabled: true,
      browserLogin: { host: 'claude.ai', ip: '198.51.100.2', country: 'US' },
      ipv6Bypass: true
    })

    const result = evaluateEgress(sample, { mode: 'country', baselines: {} })

    expect(result.state).toBe('warning')
    expect(result.checks.find((c) => c.id === 'browser-path')?.result).toBe('warn')
    expect(result.checks.find((c) => c.id === 'ipv6')?.result).toBe('warn')
  })

  it('ignores unknown login-api-match in family and overall state while preserving the unknown check', () => {
    const sample = makeBaseSample()
    sample.hosts.openai.login = { host: 'chatgpt.com', error: 'invalid' }

    const result = evaluateEgress(sample, {
      mode: 'country',
      baselines: {
        anthropic: { ip: '169.128.1.1', country: 'US' },
        openai: { ip: '169.128.1.1', country: 'US' }
      }
    })

    expect(result.state).toBe('ok')
    expect(result.families.find((f) => f.family === 'openai')?.state).toBe('ok')
    expect(
      result.checks.find((c) => c.id === 'login-api-match' && c.family === 'openai')
    ).toEqual({
      id: 'login-api-match',
      family: 'openai',
      result: 'unknown'
    })
  })
})
