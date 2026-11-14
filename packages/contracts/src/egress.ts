export const EGRESS_FAMILIES = ['anthropic', 'openai', 'chatgpt-subscription'] as const
export type EgressFamily = (typeof EGRESS_FAMILIES)[number]
export type EgressCheckId =
  | 'mainland'
  | 'exit-rule'
  | 'login-api-match'
  | 'agent-proxy-path'
  | 'browser-path'
  | 'time-zone'
  | 'ipv6'
export type EgressCheckResult = 'pass' | 'warn' | 'fail' | 'unknown'
export type EgressHostError = 'timeout' | 'unreachable' | 'invalid'

export interface EgressExit {
  ip: string
  /** ISO 3166-1 alpha-2 uppercase country/region code (e.g. "US", "CN"). */
  country: string
}

export interface EgressHostSample {
  host: string
  ip?: string
  country?: string
  error?: EgressHostError
}

export interface EgressCheckWire {
  id: EgressCheckId
  family?: EgressFamily
  result: EgressCheckResult
  facts?: Record<string, string>
}

export interface EgressFamilyWire {
  family: EgressFamily
  state: 'ok' | 'warning' | 'abnormal' | 'unknown'
  exit?: EgressExit
  hosts: EgressHostSample[]
  violation?: 'mainland' | 'rule'
  baseline?: EgressExit
  overridden: boolean
  frozenSessionIds: string[]
  recovering: boolean
}

export interface EgressStatusWire {
  state: 'checking' | 'ok' | 'warning' | 'abnormal'
  families: EgressFamilyWire[]
  checks: EgressCheckWire[]
  ruleMode: 'country' | 'ip'
  protectionEnabled: boolean
  sessionFamilies: Record<string, EgressFamily>
  checkedAt?: number
  lastTransition?: {
    id: string
    kind: 'frozen' | 'resumed'
    family: EgressFamily
    sessionCount: number
    at: number
    reason?: 'mainland' | 'rule'
  }
}

export function isEgressFamily(value: unknown): value is EgressFamily {
  return typeof value === 'string' && (EGRESS_FAMILIES as readonly string[]).includes(value)
}
