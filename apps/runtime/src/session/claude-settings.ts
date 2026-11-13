import type { ClaudeSettingsSnapshot } from '@matou/contracts'
import { agentTimeZone, isMainlandTimeZone, nonMainlandUtc8Zone } from '../egress/time-zone-policy'
import type { RuntimeDatabase } from '../storage/database'

const KEY = 'claude.timeZone'

export interface ClaudeSettingsOptions {
  systemTimeZone?: () => string
}

export interface ClaudeEnvironmentOptions {
  exitCountry?: string
  official?: boolean
}

export function isClaudeTimeZone(value: unknown): value is string {
  if (typeof value !== 'string') return false
  if (value === 'auto' || value === 'system') return true
  // Named zones preserve daylight-saving rules and work with both Intl and TZ.
  if (!/^[A-Za-z_]+(?:\/[A-Za-z0-9_+\-]+)*$/.test(value) || value.length > 100) return false
  try {
    new Intl.DateTimeFormat('en', { timeZone: value }).format(0)
    return true
  } catch { return false }
}

/** Read at launch, without a per-window cache: detached windows share this preference. */
export class ClaudeSettings {
  readonly #systemTimeZone: () => string

  constructor(private readonly database: RuntimeDatabase, options?: ClaudeSettingsOptions) {
    this.#systemTimeZone = options?.systemTimeZone ?? (() => Intl.DateTimeFormat().resolvedOptions().timeZone)
  }

  snapshot(): ClaudeSettingsSnapshot {
    const row = this.database.get<{ value_json: string }>(
      'SELECT value_json FROM preferences WHERE key = ?', KEY
    )
    let timeZone = 'auto'
    try {
      const value: unknown = row && JSON.parse(row.value_json)
      if (isClaudeTimeZone(value)) timeZone = value
    } catch { /* Invalid stored settings fall back to the default automatic policy. */ }
    const systemTimeZone = this.#systemTimeZone()
    const autoTimeZone = isMainlandTimeZone(systemTimeZone) ? nonMainlandUtc8Zone() : undefined
    return {
      timeZone,
      systemTimeZone,
      ...(autoTimeZone === undefined ? {} : { autoTimeZone })
    }
  }

  set(timeZone: string): ClaudeSettingsSnapshot {
    if (!isClaudeTimeZone(timeZone)) throw new Error('Invalid Claude Code time zone')
    this.database.run(
      `INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)
       ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`,
      KEY, JSON.stringify(timeZone), Date.now()
    )
    return this.snapshot()
  }

  environment(profile: string, options?: ClaudeEnvironmentOptions): Record<string, string> {
    if (profile !== 'claude-code' && profile !== 'codex') return {}
    const { timeZone, systemTimeZone } = this.snapshot()
    const official = options?.official ?? true
    if (!official) {
      if (profile !== 'claude-code' || timeZone === 'auto' || timeZone === 'system') return {}
      return { TZ: timeZone }
    }
    const resolved = agentTimeZone({
      profile,
      setting: timeZone,
      systemTimeZone,
      ...(options?.exitCountry === undefined ? {} : { exitCountry: options.exitCountry })
    })
    return resolved === undefined ? {} : { TZ: resolved }
  }
}
