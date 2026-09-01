import { chmod, copyFile, mkdtemp, rename } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { RuntimeDatabase } from './database'
import { DatabaseBackupService } from './database-backup-service'
import { MigrationRunner, type Migration } from './migration-runner'

export interface RuntimeDatabaseBootstrapResult {
  database: RuntimeDatabase
  recoveredFromCorruption: boolean
  effectiveDataRoot: string
  ephemeral: boolean
  quarantinedPath?: string
}

export interface RuntimeDatabaseBootstrapObserver {
  onDatabaseOpened?(
    database: RuntimeDatabase,
    effectiveDataRoot: string,
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
  let database: RuntimeDatabase | undefined
  try {
    database = RuntimeDatabase.open(databasePath)
    const backups = new DatabaseBackupService(dataRoot)
    observer.onDatabaseOpened?.(database, dataRoot, backups)
    await new MigrationRunner(
      database,
      migrations,
      backups
    ).migrate()
    return {
      database,
      recoveredFromCorruption: false,
      effectiveDataRoot: dataRoot,
      ephemeral: false
    }
  } catch (error) {
    if (database && observer.isShutdownRequested?.()) throw error
    if (database) closeObservedDatabase(database, observer)
    if (isWriteDenied(error)) {
      return openEphemeralCopy(databasePath, migrations, true, observer)
    }
    if (isNewerSchema(error)) {
      return openEphemeralCopy(databasePath, migrations, false, observer)
    }
    if (!isPhysicalDatabaseCorruption(error)) throw error
    const quarantinedPath = `${databasePath}.corrupt-${Date.now()}`
    await rename(databasePath, quarantinedPath)
    await rename(`${databasePath}-wal`, `${quarantinedPath}-wal`).catch(() => undefined)
    await rename(`${databasePath}-shm`, `${quarantinedPath}-shm`).catch(() => undefined)
    const clean = RuntimeDatabase.open(databasePath)
    const backups = new DatabaseBackupService(dataRoot)
    observer.onDatabaseOpened?.(clean, dataRoot, backups)
    try {
      await new MigrationRunner(
        clean,
        migrations,
        backups
      ).migrate()
      return {
        database: clean,
        recoveredFromCorruption: true,
        effectiveDataRoot: dataRoot,
        ephemeral: false,
        quarantinedPath
      }
    } catch (migrationError) {
      closeObservedDatabase(clean, observer)
      throw migrationError
    }
  }
}

async function openEphemeralCopy(
  durableDatabasePath: string,
  migrations: readonly Migration[],
  migrate = true,
  observer: RuntimeDatabaseBootstrapObserver = {}
): Promise<RuntimeDatabaseBootstrapResult> {
  const effectiveDataRoot = await mkdtemp(join(tmpdir(), 'matou-ephemeral-'))
  const ephemeralDatabasePath = join(effectiveDataRoot, 'matou.sqlite')
  await copyFile(durableDatabasePath, ephemeralDatabasePath).catch(() => undefined)
  await chmod(ephemeralDatabasePath, 0o600).catch(() => undefined)
  await copyFile(`${durableDatabasePath}-wal`, `${ephemeralDatabasePath}-wal`).catch(() => undefined)
  await copyFile(`${durableDatabasePath}-shm`, `${ephemeralDatabasePath}-shm`).catch(() => undefined)
  const database = RuntimeDatabase.open(ephemeralDatabasePath)
  const backups = new DatabaseBackupService(effectiveDataRoot)
  observer.onDatabaseOpened?.(database, effectiveDataRoot, backups)
  try {
    if (migrate) {
      await new MigrationRunner(
        database,
        migrations,
        backups
      ).migrate()
    }
    return {
      database,
      recoveredFromCorruption: false,
      effectiveDataRoot,
      ephemeral: true
    }
  } catch (error) {
    closeObservedDatabase(database, observer)
    throw error
  }
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
  return /file is not a database|database disk image is malformed|database corrupt/i.test(message)
}

function isWriteDenied(error: unknown): boolean {
  const code = typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : ''
  const message = error instanceof Error ? error.message : String(error)
  return code === 'EACCES' || code === 'EPERM' || /readonly|read-only|permission denied/i.test(message)
}

function isNewerSchema(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error)
  return /database schema version \d+ is newer than supported version \d+/i.test(message)
}
