import { randomUUID } from 'node:crypto'
import {
  accessSync,
  constants,
  existsSync,
  openSync,
  closeSync,
  readFileSync,
  statSync
} from 'node:fs'
import { link, open, readFile, rename, rm } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'

import {
  RuntimeDatabase,
  type RuntimeDatabaseOwnership
} from './database'
import {
  isDatabaseOwnershipRecoveryError,
  type DatabaseOwnershipRecoveryIssue
} from './database-owner'
import {
  DatabaseBackupService,
  type DatabaseBackupDescriptor
} from './database-backup-service'
import { MigrationRunner, type Migration } from './migration-runner'

export interface RuntimeDatabaseRecoveryError {
  code: 'BACKUP_LIST_FAILED' | 'RECOVERY_MARKER_FAILED' | 'RECOVERY_MOVE_FAILED'
  message: string
  retryable: true
}

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
      reason: 'physical-corruption' | 'wal-recovery-required' | 'ownership-recovery-required'
      durableDatabasePath: string
      quarantinedPath: string
      markerPath: string
      backups: DatabaseBackupDescriptor[]
      ownershipIssue?: DatabaseOwnershipRecoveryIssue
      backupListError?: RuntimeDatabaseRecoveryError
      markerError?: RuntimeDatabaseRecoveryError
      moveError?: RuntimeDatabaseRecoveryError
    }

interface RuntimeDatabaseRecoveryMarker {
  version: 1
  reason: 'physical-corruption' | 'wal-recovery-required' | 'ownership-recovery-required'
  durableDatabasePath: string
  quarantinedPath: string
  markerPath: string
  createdAt: number
  ownershipIssue?: DatabaseOwnershipRecoveryIssue
}

export interface RuntimeDatabaseBootstrapObserver {
  onDatabaseOpened?(
    database: RuntimeDatabase,
    dataRoot: string,
    backups: DatabaseBackupService
  ): void
  onDatabaseClosed?(database: RuntimeDatabase): void
  onRecoveryMarkerPublished?(
    marker: RuntimeDatabaseRecoveryMarker
  ): void | Promise<void>
  isShutdownRequested?(): boolean
}

export async function openRecoverableRuntimeDatabase(
  dataRoot: string,
  migrations: readonly Migration[],
  observer: RuntimeDatabaseBootstrapObserver = {}
): Promise<RuntimeDatabaseBootstrapResult> {
  const databasePath = join(dataRoot, 'matou.sqlite')
  const backups = new DatabaseBackupService(dataRoot)
  const persistedRecovery = readRecoveryMarker(databasePath)
  if (persistedRecovery) return recoveryResult(persistedRecovery, backups)

  let database: RuntimeDatabase | undefined
  let ownership: RuntimeDatabaseOwnership | undefined
  try {
    if (existsSync(databasePath)) {
      if (isWritable(dataRoot)) {
        try {
          ownership = RuntimeDatabase.acquireOwnership(databasePath)
        } catch (error) {
          const marker = await waitForRecoveryMarker(databasePath)
          if (marker) return recoveryResult(marker, backups)
          throw error
        }
      } else {
        RuntimeDatabase.assertNoLiveOwner(databasePath)
      }

      assertWalBundleReady(databasePath)
      if (!isWritable(dataRoot) || !isWritable(databasePath)) {
        database = ownership?.openReadOnly() ?? RuntimeDatabase.openReadOnly(databasePath)
        assertFullIntegrity(database)
        observer.onDatabaseOpened?.(database, dataRoot, backups)
        ownership?.release()
        ownership = undefined
        return {
          kind: 'read-only',
          database,
          dataRoot,
          reason: 'filesystem-read-only'
        }
      }

      const inspection = ownership!.openReadOnly()
      let currentVersion: number
      try {
        assertFullIntegrity(inspection)
        currentVersion = readSchemaVersion(inspection)
      } finally {
        inspection.close()
      }
      const supportedVersion = migrations.reduce(
        (highest, migration) => Math.max(highest, migration.version),
        0
      )
      if (currentVersion > supportedVersion) {
        database = ownership!.openReadOnly()
        assertFullIntegrity(database)
        observer.onDatabaseOpened?.(database, dataRoot, backups)
        ownership!.release()
        ownership = undefined
        return { kind: 'read-only', database, dataRoot, reason: 'newer-schema' }
      }
      database = ownership!.openWritable()
      ownership = undefined
    } else {
      ownership = RuntimeDatabase.acquireOwnership(databasePath)
      const recoveryAfterFence = readRecoveryMarker(databasePath)
      if (recoveryAfterFence) {
        ownership.release()
        ownership = undefined
        return recoveryResult(recoveryAfterFence, backups)
      }
      if (existsSync(databasePath)) {
        ownership.release()
        ownership = undefined
        return openRecoverableRuntimeDatabase(dataRoot, migrations, observer)
      }
      database = ownership.openWritable()
      ownership = undefined
    }

    observer.onDatabaseOpened?.(database, dataRoot, backups)
    assertFullIntegrity(database)
    await new MigrationRunner(database, migrations, backups).migrate()
    return { kind: 'writable', database, dataRoot }
  } catch (error) {
    if (database && observer.isShutdownRequested?.()) throw error

    if (isDatabaseOwnershipRecoveryError(error)) {
      ownership?.release()
      return preserveDatabaseForOwnershipRecovery(databasePath, backups, error.issue, observer)
    }

    const recoveryReason = recoveryReasonFor(error)
    if (database) {
      if (database.readOnly) {
        closeObservedDatabase(database, observer)
      } else if (recoveryReason || isWriteDenied(error) || isNewerSchema(error)) {
        ownership = database.closeRetainingOwnership()
        observer.onDatabaseClosed?.(database)
      } else {
        closeObservedDatabase(database, observer)
      }
      database = undefined
    }

    if (isWriteDenied(error) && !recoveryReason) {
      try {
        const readOnly = ownership?.openReadOnly() ?? RuntimeDatabase.openReadOnly(databasePath)
        assertFullIntegrity(readOnly)
        observer.onDatabaseOpened?.(readOnly, dataRoot, backups)
        ownership?.release()
        ownership = undefined
        return {
          kind: 'read-only',
          database: readOnly,
          dataRoot,
          reason: 'filesystem-read-only'
        }
      } catch (readOnlyError) {
        const readOnlyRecoveryReason = recoveryReasonFor(readOnlyError)
        if (!readOnlyRecoveryReason) {
          ownership?.release()
          throw readOnlyError
        }
        return quarantineDatabaseBundle(
          databasePath,
          backups,
          readOnlyRecoveryReason,
          ownership,
          observer
        )
      }
    }
    if (isNewerSchema(error)) {
      const readOnly = ownership?.openReadOnly() ?? RuntimeDatabase.openReadOnly(databasePath)
      assertFullIntegrity(readOnly)
      observer.onDatabaseOpened?.(readOnly, dataRoot, backups)
      ownership?.release()
      ownership = undefined
      return { kind: 'read-only', database: readOnly, dataRoot, reason: 'newer-schema' }
    }
    if (recoveryReason) {
      return quarantineDatabaseBundle(
        databasePath,
        backups,
        recoveryReason,
        ownership,
        observer
      )
    }
    ownership?.release()
    throw error
  }
}

async function preserveDatabaseForOwnershipRecovery(
  databasePath: string,
  backups: DatabaseBackupService,
  ownershipIssue: DatabaseOwnershipRecoveryIssue,
  observer: RuntimeDatabaseBootstrapObserver
): Promise<Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }>> {
  const marker = newRecoveryMarker(
    databasePath,
    'ownership-recovery-required',
    ownershipIssue
  )
  try {
    const published = await publishRecoveryMarker(marker)
    await observer.onRecoveryMarkerPublished?.(published)
    return recoveryResult(published, backups)
  } catch (error) {
    return recoveryResult(marker, backups, {
      markerError: recoveryError('RECOVERY_MARKER_FAILED', error),
      quarantinedPath: databasePath
    })
  }
}

async function quarantineDatabaseBundle(
  databasePath: string,
  backups: DatabaseBackupService,
  reason: RuntimeDatabaseRecoveryMarker['reason'],
  ownership: RuntimeDatabaseOwnership | undefined,
  observer: RuntimeDatabaseBootstrapObserver
): Promise<RuntimeDatabaseBootstrapResult> {
  const marker = newRecoveryMarker(databasePath, reason)
  let published: RuntimeDatabaseRecoveryMarker
  try {
    published = await publishRecoveryMarker(marker)
    await observer.onRecoveryMarkerPublished?.(published)
  } catch (error) {
    ownership?.release()
    return recoveryResult(marker, backups, {
      markerError: recoveryError('RECOVERY_MARKER_FAILED', error),
      quarantinedPath: databasePath
    })
  }

  let moveError: RuntimeDatabaseRecoveryError | undefined
  try {
    await moveDatabaseBundle(databasePath, published.quarantinedPath)
  } catch (error) {
    moveError = recoveryError('RECOVERY_MOVE_FAILED', error)
  } finally {
    ownership?.release()
  }
  return recoveryResult(published, backups, moveError ? { moveError } : {})
}

async function moveDatabaseBundle(databasePath: string, quarantinedPath: string): Promise<void> {
  if (existsSync(databasePath)) await rename(databasePath, quarantinedPath)
  for (const suffix of ['-wal', '-shm'] as const) {
    const source = `${databasePath}${suffix}`
    const target = `${quarantinedPath}${suffix}`
    if (existsSync(source)) await rename(source, target)
  }
}

function newRecoveryMarker(
  databasePath: string,
  reason: RuntimeDatabaseRecoveryMarker['reason'],
  ownershipIssue?: DatabaseOwnershipRecoveryIssue
): RuntimeDatabaseRecoveryMarker {
  const createdAt = Date.now()
  return {
    version: 1,
    reason,
    durableDatabasePath: databasePath,
    quarantinedPath: reason === 'ownership-recovery-required'
      ? databasePath
      : `${databasePath}.corrupt-${createdAt}`,
    markerPath: `${databasePath}.recovery.json`,
    createdAt,
    ...(ownershipIssue ? { ownershipIssue } : {})
  }
}

async function publishRecoveryMarker(
  marker: RuntimeDatabaseRecoveryMarker
): Promise<RuntimeDatabaseRecoveryMarker> {
  const existing = readRecoveryMarker(marker.durableDatabasePath)
  if (existing) return existing
  const partialPath = `${marker.markerPath}.partial-${randomUUID()}`
  const handle = await open(partialPath, 'wx', 0o600)
  try {
    await handle.writeFile(JSON.stringify(marker), 'utf8')
    await handle.sync()
  } finally {
    await handle.close()
  }
  try {
    await link(partialPath, marker.markerPath)
    syncDirectory(dirname(marker.markerPath))
    return marker
  } catch (error) {
    if (errorCode(error) === 'EEXIST') {
      const competing = readRecoveryMarker(marker.durableDatabasePath)
      if (competing) return competing
    }
    throw error
  } finally {
    await rm(partialPath, { force: true }).catch(() => undefined)
  }
}

function syncDirectory(path: string): void {
  let descriptor: number | undefined
  try {
    descriptor = openSync(path, constants.O_RDONLY)
    const fs = process.getBuiltinModule('node:fs') as typeof import('node:fs')
    fs.fsyncSync(descriptor)
  } finally {
    if (descriptor !== undefined) closeSync(descriptor)
  }
}

function readRecoveryMarker(databasePath: string): RuntimeDatabaseRecoveryMarker | undefined {
  const markerPath = `${databasePath}.recovery.json`
  let value: unknown
  try {
    value = JSON.parse(readFileSync(markerPath, 'utf8')) as unknown
  } catch (error) {
    if (errorCode(error) === 'ENOENT') return undefined
    throw error
  }
  if (typeof value !== 'object' || value === null) {
    throw new Error('database recovery marker is invalid')
  }
  const marker = value as Partial<RuntimeDatabaseRecoveryMarker>
  const durablePath = resolve(databasePath)
  if (
    marker.version !== 1 ||
    ![
      'physical-corruption',
      'wal-recovery-required',
      'ownership-recovery-required'
    ].includes(String(marker.reason)) ||
    marker.durableDatabasePath !== durablePath ||
    marker.markerPath !== markerPath ||
    typeof marker.quarantinedPath !== 'string' ||
    dirname(marker.quarantinedPath) !== dirname(durablePath) ||
    (marker.reason === 'ownership-recovery-required'
      ? marker.quarantinedPath !== durablePath ||
        !['owner-record-malformed', 'takeover-sidecar-unusable'].includes(
          String(marker.ownershipIssue)
        )
      : !marker.quarantinedPath.startsWith(`${durablePath}.corrupt-`) ||
        marker.ownershipIssue !== undefined) ||
    !Number.isSafeInteger(marker.createdAt)
  ) {
    throw new Error('database recovery marker is invalid')
  }
  return marker as RuntimeDatabaseRecoveryMarker
}

async function waitForRecoveryMarker(
  databasePath: string,
  timeoutMs = 250
): Promise<RuntimeDatabaseRecoveryMarker | undefined> {
  const deadline = Date.now() + timeoutMs
  do {
    const marker = readRecoveryMarker(databasePath)
    if (marker) return marker
    await new Promise((resolve) => setTimeout(resolve, 10))
  } while (Date.now() < deadline)
  return readRecoveryMarker(databasePath)
}

async function recoveryResult(
  marker: RuntimeDatabaseRecoveryMarker,
  backups: DatabaseBackupService,
  overrides: {
    quarantinedPath?: string
    markerError?: RuntimeDatabaseRecoveryError
    moveError?: RuntimeDatabaseRecoveryError
  } = {}
): Promise<Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }>> {
  let validBackups: DatabaseBackupDescriptor[] = []
  let backupListError: RuntimeDatabaseRecoveryError | undefined
  try {
    validBackups = await backups.listValid()
  } catch (error) {
    backupListError = recoveryError('BACKUP_LIST_FAILED', error)
  }
  return {
    kind: 'recovery-required',
    reason: marker.reason,
    durableDatabasePath: marker.durableDatabasePath,
    quarantinedPath: overrides.quarantinedPath ?? marker.quarantinedPath,
    markerPath: marker.markerPath,
    backups: validBackups,
    ...(marker.ownershipIssue ? { ownershipIssue: marker.ownershipIssue } : {}),
    ...(backupListError ? { backupListError } : {}),
    ...(overrides.markerError ? { markerError: overrides.markerError } : {}),
    ...(overrides.moveError ? { moveError: overrides.moveError } : {})
  }
}

function recoveryError(
  code: RuntimeDatabaseRecoveryError['code'],
  error: unknown
): RuntimeDatabaseRecoveryError {
  return { code, message: errorMessage(error), retryable: true }
}

function assertWalBundleReady(databasePath: string): void {
  const walPath = `${databasePath}-wal`
  if (!existsSync(walPath) || statSync(walPath).size <= 32) return
  const shmPath = `${databasePath}-shm`
  let bytes: Buffer
  try {
    accessSync(shmPath, constants.R_OK)
    const metadata = statSync(shmPath)
    if (!metadata.isFile() || metadata.size < 32_768) throw new Error('SHM is truncated')
    bytes = readFileSync(shmPath)
  } catch (error) {
    throw walRecoveryError(`committed WAL has no readable SHM: ${errorMessage(error)}`)
  }
  const firstHeader = bytes.subarray(0, 48)
  const secondHeader = bytes.subarray(48, 96)
  const version = firstHeader.length === 48 ? firstHeader.readUInt32LE(0) : 0
  if (version !== 3_007_000 || !firstHeader.equals(secondHeader)) {
    throw walRecoveryError('committed WAL has a corrupt SHM header')
  }
}

function walRecoveryError(message: string): Error & { code: 'WAL_RECOVERY_REQUIRED' } {
  return Object.assign(new Error(message), { code: 'WAL_RECOVERY_REQUIRED' as const })
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

function recoveryReasonFor(
  error: unknown
): RuntimeDatabaseRecoveryMarker['reason'] | undefined {
  if (errorCode(error) === 'WAL_RECOVERY_REQUIRED' || /WAL requires recovery/i.test(errorMessage(error))) {
    return 'wal-recovery-required'
  }
  return isPhysicalDatabaseCorruption(error) ? 'physical-corruption' : undefined
}

function isPhysicalDatabaseCorruption(error: unknown): boolean {
  return /file is not a database|database disk image is malformed|database corrupt|integrity check failed/i.test(
    errorMessage(error)
  )
}

function isWriteDenied(error: unknown): boolean {
  const code = errorCode(error)
  return code === 'EACCES' || code === 'EPERM' || /readonly|read-only|permission denied/i.test(
    errorMessage(error)
  )
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
  return /database schema version \d+ is newer than supported version \d+/i.test(
    errorMessage(error)
  )
}

function errorCode(error: unknown): string | undefined {
  return typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : undefined
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
