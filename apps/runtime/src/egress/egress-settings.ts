import { isEgressFamily, type EgressExit, type EgressFamily } from '@matou/contracts'
import type { RuntimeDatabase } from '../storage/database'
import type { EgressRule } from './egress-evaluator'

const PROTECTION_KEY = 'egress.protection'
const RULE_KEY = 'egress.rule'

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isValidExit(value: unknown): value is EgressExit {
  if (!isPlainRecord(value)) return false
  return (
    typeof value.ip === 'string' &&
    value.ip.length > 0 &&
    typeof value.country === 'string' &&
    value.country.length > 0
  )
}

function parseRule(raw: unknown): EgressRule {
  if (!isPlainRecord(raw)) {
    return { mode: 'country', baselines: {} }
  }
  const mode: EgressRule['mode'] = raw.mode === 'country' || raw.mode === 'ip' ? raw.mode : 'country'
  const baselines: Partial<Record<EgressFamily, EgressExit>> = {}
  if (isPlainRecord(raw.baselines)) {
    for (const [key, entry] of Object.entries(raw.baselines)) {
      if (isEgressFamily(key) && isValidExit(entry)) {
        baselines[key] = { ip: entry.ip, country: entry.country }
      }
    }
  }
  return { mode, baselines }
}

/** Persists network egress protection and rule preferences in the runtime database. */
export class EgressSettings {
  constructor(private readonly database: RuntimeDatabase) {}

  protectionEnabled(): boolean {
    const row = this.#readPreference(PROTECTION_KEY)
    if (row === undefined) return true
    try {
      const value: unknown = JSON.parse(row.value_json)
      return typeof value === 'boolean' ? value : true
    } catch {
      return true
    }
  }

  setProtection(enabled: boolean): void {
    this.#writePreference(PROTECTION_KEY, Boolean(enabled))
  }

  rule(): EgressRule {
    const row = this.#readPreference(RULE_KEY)
    if (row === undefined) return { mode: 'country', baselines: {} }
    try {
      const value: unknown = JSON.parse(row.value_json)
      return parseRule(value)
    } catch {
      return { mode: 'country', baselines: {} }
    }
  }

  setMode(mode: 'country' | 'ip'): void {
    const current = this.rule()
    this.#writePreference(RULE_KEY, {
      mode,
      baselines: current.baselines
    })
  }

  setBaseline(family: EgressFamily, exit: EgressExit): void {
    const current = this.rule()
    this.#writePreference(RULE_KEY, {
      mode: current.mode,
      baselines: {
        ...current.baselines,
        [family]: { ip: exit.ip, country: exit.country }
      }
    })
  }

  #readPreference(key: string): { value_json: string } | undefined {
    return this.database.get<{ value_json: string }>(
      'SELECT value_json FROM preferences WHERE key = ?',
      key
    )
  }

  #writePreference(key: string, value: unknown): void {
    this.database.run(
      `INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)
       ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`,
      key,
      JSON.stringify(value),
      Date.now()
    )
  }
}
