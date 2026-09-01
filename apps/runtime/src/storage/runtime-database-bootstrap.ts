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

export async function openRecoverableRuntimeDatabase(
  dataRoot: string,
  migrations: readonly Migration[]
): Promise<RuntimeDatabaseBootstrapResult> {
  const databasePath = join(dataRoot, 'matou.sqlite')
  let database: RuntimeDatabase | undefined
  try {
    database = RuntimeDatabase.open(databasePath)
    await new MigrationRunner(
      database,
      migrations,
      new DatabaseBackupService(dataRoot)
    ).migrate()
    return {
      database,
      recoveredFromCorruption: false,
      effectiveDataRoot: dataRoot,
      ephemeral: false
    }
  } catch (error) {
    database?.close()
    if (isWriteDenied(error)) {
      return openEphemeralCopy(databasePath, migrations)
    }
    if (isNewerSchema(error)) {
      return openEphemeralCopy(databasePath, migrations, false)
    }
    if (!isPhysicalDatabaseCorruption(error)) throw error
    const quarantinedPath = `${databasePath}.corrupt-${Date.now()}`
    await rename(databasePath, quarantinedPath)
    await rename(`${databasePath}-wal`, `${quarantinedPath}-wal`).catch(() => undefined)
    await rename(`${databasePath}-shm`, `${quarantinedPath}-shm`).catch(() => undefined)
    const clean = RuntimeDatabase.open(databasePath)
    try {
      await new MigrationRunner(
        clean,
        migrations,
        new DatabaseBackupService(dataRoot)
      ).migrate()
      return {
        database: clean,
        recoveredFromCorruption: true,
        effectiveDataRoot: dataRoot,
        ephemeral: false,
        quarantinedPath
      }
    } catch (migrationError) {
      clean.close()
      throw migrationError
    }
  }
}

async function openEphemeralCopy(
  durableDatabasePath: string,
  migrations: readonly Migration[],
  migrate = true
): Promise<RuntimeDatabaseBootstrapResult> {
  const effectiveDataRoot = await mkdtemp(join(tmpdir(), 'matou-ephemeral-'))
  const ephemeralDatabasePath = join(effectiveDataRoot, 'matou.sqlite')
  await copyFile(durableDatabasePath, ephemeralDatabasePath).catch(() => undefined)
  await chmod(ephemeralDatabasePath, 0o600).catch(() => undefined)
  await copyFile(`${durableDatabasePath}-wal`, `${ephemeralDatabasePath}-wal`).catch(() => undefined)
  await copyFile(`${durableDatabasePath}-shm`, `${ephemeralDatabasePath}-shm`).catch(() => undefined)
  const database = RuntimeDatabase.open(ephemeralDatabasePath)
  try {
    if (migrate) {
      await new MigrationRunner(
        database,
        migrations,
        new DatabaseBackupService(effectiveDataRoot)
      ).migrate()
    }
    return {
      database,
      recoveredFromCorruption: false,
      effectiveDataRoot,
      ephemeral: true
    }
  } catch (error) {
    database.close()
    throw error
  }
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
