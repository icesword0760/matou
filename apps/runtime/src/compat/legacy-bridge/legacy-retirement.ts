import type { RuntimeDatabase } from '../../storage/database'

export type MigrationPhase = 'shadow' | 'sqlite-read' | 'retired'

export class LegacyRetirementGuard {
  readonly #database: RuntimeDatabase
  readonly #backupWindowMs: number

  constructor(database: RuntimeDatabase, options: { backupWindowMs: number }) {
    this.#database = database
    this.#backupWindowMs = options.backupWindowMs
    if (!Number.isSafeInteger(this.#backupWindowMs) || this.#backupWindowMs < 0) {
      throw new Error('Invalid legacy backup window')
    }
  }

  phase(): MigrationPhase {
    return this.#value<MigrationPhase>('migration-phase') ?? 'shadow'
  }

  readAuthority(): 'legacy' | 'sqlite' {
    return this.#value<'legacy' | 'sqlite'>('read-authority') ?? 'legacy'
  }

  retire(now = Date.now()): void {
    this.#database.transaction((tx) => {
      setValue(tx, 'migration-phase', 'retired', now)
      setValue(tx, 'read-authority', 'sqlite', now)
      setValue(tx, 'legacy-retired-at', now, now)
      setValue(tx, 'legacy-backup-window-ms', this.#backupWindowMs, now)
    })
  }

  assertShadowWriteAllowed(): void {
    if (this.phase() === 'retired') throw new Error('legacy shadow writes are retired')
  }

  canReadLegacyBackup(now = Date.now()): boolean {
    const retiredAt = this.#value<number>('legacy-retired-at')
    return retiredAt === undefined || now <= retiredAt + this.#backupWindowMs
  }

  #value<T>(key: string): T | undefined {
    const row = this.#database.get<{ value_json: string }>(
      'SELECT value_json FROM migration_authority WHERE key = ?', key
    )
    if (!row) return undefined
    try { return JSON.parse(row.value_json) as T } catch { return undefined }
  }
}

function setValue(
  tx: Pick<RuntimeDatabase, 'run'>,
  key: string,
  value: unknown,
  now: number
): void {
  tx.run(
    `INSERT INTO migration_authority (key, value_json, updated_at) VALUES (?, ?, ?)
     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`,
    key, JSON.stringify(value), now
  )
}
