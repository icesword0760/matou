import type { ClaudeSettingsSnapshot } from '@matou/contracts'
import type { RuntimeDatabase } from '../storage/database'

const KEY = 'claude.timeZone'

export function isClaudeTimeZone(value: unknown): value is string {
  if (typeof value !== 'string') return false
  if (value === 'system') return true
  // Named zones preserve daylight-saving rules and work with both Intl and TZ.
  if (!/^[A-Za-z_]+(?:\/[A-Za-z0-9_+\-]+)*$/.test(value) || value.length > 100) return false
  try {
    new Intl.DateTimeFormat('en', { timeZone: value }).format(0)
    return true
  } catch { return false }
}

/** Read at launch, without a per-window cache: detached windows share this preference. */
export class ClaudeSettings {
  constructor(private readonly database: RuntimeDatabase) {}

  snapshot(): ClaudeSettingsSnapshot {
    const row = this.database.get<{ value_json: string }>(
      'SELECT value_json FROM preferences WHERE key = ?', KEY
    )
    let timeZone = 'system'
    try {
      const value: unknown = row && JSON.parse(row.value_json)
      if (isClaudeTimeZone(value)) timeZone = value
    } catch { /* Invalid stored settings fall back to the launch environment. */ }
    return { timeZone, systemTimeZone: Intl.DateTimeFormat().resolvedOptions().timeZone }
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

  environment(profile: string): Record<string, string> {
    if (profile !== 'claude-code') return {}
    const { timeZone } = this.snapshot()
    return timeZone === 'system' ? {} : { TZ: timeZone }
  }
}
