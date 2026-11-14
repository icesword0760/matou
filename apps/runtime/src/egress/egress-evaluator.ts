import {
  EGRESS_FAMILIES,
  type EgressCheckResult,
  type EgressCheckWire,
  type EgressExit,
  type EgressFamily,
  type EgressHostSample
} from '@matou/contracts'

export const EGRESS_HOSTS: Record<EgressFamily, { api: string; login: string }> = {
  anthropic: { api: 'api.anthropic.com', login: 'claude.ai' },
  openai: { api: 'api.openai.com', login: 'chatgpt.com' },
  'chatgpt-subscription': { api: 'chatgpt.com', login: 'auth.openai.com' }
}

export interface EgressSample {
  hosts: Partial<Record<EgressFamily, { api: EgressHostSample; login: EgressHostSample }>>
  browserLogin?: EgressHostSample
  agentHasProxyEnv: boolean
  systemProxyEnabled: boolean
  tunDetected: boolean
  ipv6Bypass: boolean
  agentTimeZoneMainland: boolean
}

export interface FamilyEvaluation {
  family: EgressFamily
  state: 'ok' | 'warning' | 'abnormal' | 'unknown'
  exit?: EgressExit
  violation?: 'mainland' | 'rule'
}

export interface EgressEvaluation {
  families: FamilyEvaluation[]
  checks: EgressCheckWire[]
  state: 'ok' | 'warning' | 'abnormal'
}

export interface EgressRule {
  mode: 'country' | 'ip'
  baselines: Partial<Record<EgressFamily, EgressExit>>
}

export function evaluateEgress(sample: EgressSample, rule: EgressRule): EgressEvaluation {
  const families: FamilyEvaluation[] = []
  const checks: EgressCheckWire[] = []

  for (const family of EGRESS_FAMILIES) {
    const pair = sample.hosts[family]
    if (!pair) continue
    const { evaluation, familyChecks } = evaluateFamily(family, pair, rule)
    families.push(evaluation)
    checks.push(...familyChecks)
  }

  checks.push(evaluateAgentProxyPath(sample))
  checks.push(evaluateBrowserPath(sample))
  checks.push({
    id: 'time-zone',
    result: sample.agentTimeZoneMainland ? 'warn' : 'pass'
  })
  checks.push({
    id: 'ipv6',
    result: sample.ipv6Bypass ? 'warn' : 'pass'
  })

  const state: EgressEvaluation['state'] = families.some((f) => f.state === 'abnormal')
    ? 'abnormal'
    : families.some((f) => f.state === 'warning' || f.state === 'unknown') ||
        checks.some(
          (c) => c.result === 'warn' || (c.result === 'unknown' && c.id !== 'login-api-match')
        )
      ? 'warning'
      : 'ok'

  return { families, checks, state }
}

function evaluateFamily(
  family: EgressFamily,
  pair: { api: EgressHostSample; login: EgressHostSample },
  rule: EgressRule
): { evaluation: FamilyEvaluation; familyChecks: EgressCheckWire[] } {
  const { api, login } = pair
  const exit = resolveExit(api, login)

  const mainlandCheck = evaluateMainland(family, api, login)
  const exitRuleCheck = evaluateExitRule(family, api, rule)
  const loginApiMatchCheck = evaluateLoginApiMatch(family, api, login)

  let violation: 'mainland' | 'rule' | undefined
  let state: FamilyEvaluation['state']

  if (mainlandCheck.result === 'fail') {
    violation = 'mainland'
    state = 'abnormal'
  } else if (exitRuleCheck.result === 'fail') {
    violation = 'rule'
    state = 'abnormal'
  } else if (mainlandCheck.result === 'unknown' || exitRuleCheck.result === 'unknown') {
    state = 'unknown'
  } else if (loginApiMatchCheck.result === 'warn') {
    state = 'warning'
  } else {
    state = 'ok'
  }

  const evaluation: FamilyEvaluation = { family, state }
  if (exit) evaluation.exit = exit
  if (violation) evaluation.violation = violation

  return {
    evaluation,
    familyChecks: [mainlandCheck, exitRuleCheck, loginApiMatchCheck]
  }
}

function resolveExit(api: EgressHostSample, login: EgressHostSample): EgressExit | undefined {
  if (hasResolvedExit(api)) return { ip: api.ip, country: api.country }
  if (hasResolvedExit(login)) return { ip: login.ip, country: login.country }
  return undefined
}

function hasResolvedExit(
  sample: EgressHostSample
): sample is EgressHostSample & { ip: string; country: string } {
  return Boolean(sample.ip && sample.country)
}

function evaluateMainland(
  family: EgressFamily,
  api: EgressHostSample,
  login: EgressHostSample
): EgressCheckWire {
  const cnHost = api.country === 'CN'
    ? api.host
    : login.country === 'CN'
      ? login.host
      : undefined
  if (cnHost) {
    return { id: 'mainland', family, result: 'fail', facts: { host: cnHost } }
  }

  if (!hasResolvedExit(api) && !hasResolvedExit(login)) {
    return { id: 'mainland', family, result: 'unknown' }
  }

  return { id: 'mainland', family, result: 'pass' }
}

function evaluateExitRule(
  family: EgressFamily,
  api: EgressHostSample,
  rule: EgressRule
): EgressCheckWire {
  const baseline = rule.baselines[family]
  if (!baseline) {
    return { id: 'exit-rule', family, result: 'pass' }
  }

  if (!hasResolvedExit(api)) {
    return { id: 'exit-rule', family, result: 'unknown' }
  }

  const mismatch = rule.mode === 'country'
    ? api.country !== baseline.country
    : api.ip !== baseline.ip

  if (mismatch) {
    return {
      id: 'exit-rule',
      family,
      result: 'fail',
      facts: {
        baselineCountry: baseline.country,
        baselineIp: baseline.ip,
        currentCountry: api.country,
        currentIp: api.ip
      }
    }
  }

  return { id: 'exit-rule', family, result: 'pass' }
}

function evaluateLoginApiMatch(
  family: EgressFamily,
  api: EgressHostSample,
  login: EgressHostSample
): EgressCheckWire {
  if (!hasResolvedExit(api) || !hasResolvedExit(login)) {
    return { id: 'login-api-match', family, result: 'unknown' }
  }

  if (api.ip !== login.ip) {
    return {
      id: 'login-api-match',
      family,
      result: 'warn',
      facts: { apiIp: api.ip, loginIp: login.ip }
    }
  }

  return { id: 'login-api-match', family, result: 'pass' }
}

function evaluateAgentProxyPath(sample: EgressSample): EgressCheckWire {
  const result: EgressCheckResult =
    sample.systemProxyEnabled && !sample.agentHasProxyEnv && !sample.tunDetected
      ? 'warn'
      : 'pass'
  return { id: 'agent-proxy-path', result }
}

function evaluateBrowserPath(sample: EgressSample): EgressCheckWire {
  if (!sample.systemProxyEnabled || !sample.browserLogin) {
    return { id: 'browser-path', result: 'pass' }
  }

  const agentIp = sample.hosts.anthropic?.login.ip
  const browserIp = sample.browserLogin.ip
  if (!browserIp || !agentIp) {
    return { id: 'browser-path', result: 'unknown' }
  }

  if (browserIp !== agentIp) {
    return {
      id: 'browser-path',
      result: 'warn',
      facts: { browserIp, agentIp }
    }
  }

  return { id: 'browser-path', result: 'pass' }
}
