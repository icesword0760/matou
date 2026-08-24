import { createHash } from 'node:crypto'

import type { RuntimeDatabase } from './database'

export interface Migration {
  version: number
  name: string
  sql: string
}

export interface MigrationResult {
  appliedVersions: number[]
  currentVersion: number
  backupPath: string | undefined
}

interface AppliedMigration {
  version: number
  name: string
  checksum: string
}

export class MigrationRunner {
  readonly #database: RuntimeDatabase
  readonly #migrations: readonly Migration[]

  constructor(database: RuntimeDatabase, migrations: readonly Migration[]) {
    this.#database = database
    this.#migrations = [...migrations].sort((left, right) => left.version - right.version)
    validateMigrationSequence(this.#migrations)
  }

  async migrate(): Promise<MigrationResult> {
    this.#ensureHistoryTable()
    const applied = this.#database.all<AppliedMigration>(
      'SELECT version, name, checksum FROM schema_migrations ORDER BY version'
    )
    const supportedVersion = this.#migrations.at(-1)?.version ?? 0
    const currentVersion = applied.at(-1)?.version ?? 0
    if (currentVersion > supportedVersion) {
      throw new Error(
        `database schema version ${currentVersion} is newer than supported version ${supportedVersion}`
      )
    }

    for (const stored of applied) {
      const migration = this.#migrations.find(({ version }) => version === stored.version)
      if (!migration || checksum(migration) !== stored.checksum) {
        throw new Error(`checksum mismatch for applied migration ${stored.version}`)
      }
    }

    const pending = this.#migrations.filter(({ version }) => version > currentVersion)
    let backupPath: string | undefined
    if (currentVersion > 0 && pending.length > 0) {
      backupPath = `${this.#database.path}.pre-v${pending[0]!.version}-${Date.now()}.sqlite`
      await this.#database.backupTo(backupPath)
    }

    const appliedVersions: number[] = []
    for (const migration of pending) {
      this.#database.transaction((transaction) => {
        transaction.exec(migration.sql)
        transaction.run(
          'INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)',
          migration.version,
          migration.name,
          checksum(migration),
          Date.now()
        )
      })
      appliedVersions.push(migration.version)
    }

    return {
      appliedVersions,
      currentVersion: pending.at(-1)?.version ?? currentVersion,
      backupPath
    }
  }

  #ensureHistoryTable(): void {
    this.#database.exec(`
      CREATE TABLE IF NOT EXISTS schema_migrations (
        version INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        checksum TEXT NOT NULL,
        applied_at INTEGER NOT NULL
      ) STRICT;
    `)
  }
}

function checksum(migration: Migration): string {
  return createHash('sha256')
    .update(`${migration.version}\0${migration.name}\0${migration.sql}`)
    .digest('hex')
}

function validateMigrationSequence(migrations: readonly Migration[]): void {
  let previous = 0
  for (const migration of migrations) {
    if (!Number.isSafeInteger(migration.version) || migration.version <= 0) {
      throw new Error(`invalid migration version ${migration.version}`)
    }
    if (migration.version <= previous) {
      throw new Error(`duplicate or unordered migration version ${migration.version}`)
    }
    if (migration.name.trim() === '' || migration.sql.trim() === '') {
      throw new Error(`migration ${migration.version} must have a name and SQL`)
    }
    previous = migration.version
  }
}
