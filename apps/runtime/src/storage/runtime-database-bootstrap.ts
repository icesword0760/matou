import { accessSync, constants, existsSync } from 'node:fs'
import { rename } from 'node:fs/promises'
import { join } from 'node:path'

import { RuntimeDatabase } from './database'
import {
  DatabaseBackupService,
  type DatabaseBackupDescriptor
} from './database-backup-service'
import { MigrationRunner, type Migration } from './migration-runner'

export type RuntimeDatabaseBootstrapResult =
  | { kind: 'writable'; database: RuntimeDatabase; dataRoot: string }
  | {
      kind: 'read-only'
      database: RuntimeDatabase
      dataRoot: string
      reason: 'filesystem-read-only' | 'newer-schema'
    }
  | {
      kind: 'recovery-required'
      reason: 'physical-corruption'
      durableDatabasePath: string
      quarantinedPath: string
      backups: DatabaseBackupDescriptor[]
    }

export interface RuntimeDatabaseBootstrapObserver {
  onDatabaseOpened?(
    database: RuntimeDatabase,
    dataRoot: string,
    backups: DatabaseBackupService
  ): void
  onDatabaseClosed?(database: RuntimeDatabase): void
  isShutdownRequested?(): boolean
}

export async function openRecoverableRuntimeDatabase(
  dataRoot: string,
  migrations: readonly Migration[],
  observer: RuntimeDatabaseBootstrapObserver = {}
): Promise<RuntimeDatabaseBootstrapResult> {
  const databasePath = join(dataRoot, 'matou.sqlite')
  const backups = new DatabaseBackupService(dataRoot)
  let database: RuntimeDatabase | undefined

  try {
    if (existsSync(databasePath)) {
      if (!isWritable(dataRoot) || !isWritable(databasePath)) {
        return openReadOnlyDatabase(
          databasePath,
          dataRoot,
          backups,
          'filesystem-read-only',
          observer
        )
      }
      const supportedVersion = migrations.reduce(
        (highest, migration) => Math.max(highest, migration.version),
        0
      )
      database = RuntimeDatabase.openWritableValidated(databasePath, (inspection) => {
        assertFullIntegrity(inspection)
        const currentVersion = readSchemaVersion(inspection)
        if (currentVersion > supportedVersion) {
          throw new Error(
            `database schema version ${currentVersion} is newer than supported version ${supportedVersion}`
          )
        }
      })
    } else {
      database = RuntimeDatabase.open(databasePath)
    }

    observer.onDatabaseOpened?.(database, dataRoot, backups)
    assertFullIntegrity(database)
    await new MigrationRunner(database, migrations, backups).migrate()
    return { kind: 'writable', database, dataRoot }
  } catch (error) {
    if (database && observer.isShutdownRequested?.()) throw error
    if (database) closeObservedDatabase(database, observer)

    if (isWriteDenied(error)) {
      try {
        return openReadOnlyDatabase(
          databasePath,
          dataRoot,
          backups,
          'filesystem-read-only',
          observer
        )
      } catch (readOnlyError) {
        if (!isPhysicalDatabaseCorruption(readOnlyError)) throw readOnlyError
        return quarantineCorruptDatabase(databasePath, backups)
      }
    }
    if (isNewerSchema(error)) {
      return openReadOnlyDatabase(databasePath, dataRoot, backups, 'newer-schema', observer)
    }
    if (!isPhysicalDatabaseCorruption(error)) throw error
    return quarantineCorruptDatabase(databasePath, backups)
  }
}

function openReadOnlyDatabase(
  databasePath: string,
  dataRoot: string,
  backups: DatabaseBackupService,
  reason: 'filesystem-read-only' | 'newer-schema',
  observer: RuntimeDatabaseBootstrapObserver
): RuntimeDatabaseBootstrapResult {
  const database = RuntimeDatabase.openReadOnly(databasePath)
  try {
    assertFullIntegrity(database)
    observer.onDatabaseOpened?.(database, dataRoot, backups)
    return { kind: 'read-only', database, dataRoot, reason }
  } catch (error) {
    closeObservedDatabase(database, observer)
    throw error
  }
}

async function quarantineCorruptDatabase(
  databasePath: string,
  backups: DatabaseBackupService
): Promise<RuntimeDatabaseBootstrapResult> {
  const quarantinedPath = `${databasePath}.corrupt-${Date.now()}`
  await rename(databasePath, quarantinedPath)
  await rename(`${databasePath}-wal`, `${quarantinedPath}-wal`).catch(() => undefined)
  await rename(`${databasePath}-shm`, `${quarantinedPath}-shm`).catch(() => undefined)
  return {
    kind: 'recovery-required',
    reason: 'physical-corruption',
    durableDatabasePath: databasePath,
    quarantinedPath,
    backups: await backups.listValid()
  }
}

function assertFullIntegrity(database: RuntimeDatabase): void {
  const rows = database.all<Record<string, unknown>>('PRAGMA integrity_check')
  const result = rows.map((row) => String(Object.values(row)[0] ?? ''))
  if (result.length !== 1 || result[0]?.toLowerCase() !== 'ok') {
    throw new Error(`database corrupt: integrity_check failed: ${result.slice(0, 3).join('; ')}`)
  }
}

function readSchemaVersion(database: RuntimeDatabase): number {
  const historyExists = database.get<{ present: number }>(
    `SELECT 1 AS present FROM sqlite_master
     WHERE type = 'table' AND name = 'schema_migrations'`
  ) !== undefined
  if (!historyExists) return 0
  return database.get<{ version: number | null }>(
    'SELECT MAX(version) AS version FROM schema_migrations'
  )?.version ?? 0
}

function closeObservedDatabase(
  database: RuntimeDatabase,
  observer: RuntimeDatabaseBootstrapObserver
): void {
  database.close()
  observer.onDatabaseClosed?.(database)
}

function isPhysicalDatabaseCorruption(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error)
  return /file is not a database|database disk image is malformed|database corrupt|integrity check failed/i.test(
    message
  )
}

function isWriteDenied(error: unknown): boolean {
  const code = typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : ''
  const message = error instanceof Error ? error.message : String(error)
  return code === 'EACCES' || code === 'EPERM' || /readonly|read-only|permission denied/i.test(message)
}

function isWritable(path: string): boolean {
  try {
    accessSync(path, constants.W_OK)
    return true
  } catch {
    return false
  }
}

function isNewerSchema(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error)
  return /database schema version \d+ is newer than supported version \d+/i.test(message)
}
