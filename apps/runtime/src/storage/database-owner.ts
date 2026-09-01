import { randomUUID } from 'node:crypto'
import {
  chmodSync,
  closeSync,
  constants,
  fsyncSync,
  linkSync,
  lstatSync,
  openSync,
  readFileSync,
  rmSync,
  writeFileSync
} from 'node:fs'
import type { Stats } from 'node:fs'
import { dirname, join } from 'node:path'
import type { DatabaseSync as DatabaseSyncType } from 'node:sqlite'

const { DatabaseSync } = process.getBuiltinModule(
  'node:sqlite'
) as typeof import('node:sqlite')

export interface DatabaseOwnerRecord {
  pid: number
  runtimeGeneration: string
}

export type DatabaseOwnershipRecoveryIssue =
  | 'owner-record-malformed'
  | 'takeover-sidecar-unusable'

export class DatabaseOwnershipRecoveryError extends Error {
  readonly code = 'OWNERSHIP_RECOVERY_REQUIRED'
  readonly issue: DatabaseOwnershipRecoveryIssue
  readonly statePath: string

  constructor(
    issue: DatabaseOwnershipRecoveryIssue,
    statePath: string,
    cause?: unknown
  ) {
    super(
      `database ownership state requires recovery: ${issue} at ${statePath}` +
      (cause === undefined ? '' : `: ${errorMessage(cause)}`),
      cause === undefined ? undefined : { cause }
    )
    this.name = 'DatabaseOwnershipRecoveryError'
    this.issue = issue
    this.statePath = statePath
  }
}

export interface DatabaseOwnerPublicationObserver {
  onPrepared?(): void
  onPublished?(): void
}

type OwnerInspection =
  | { state: 'absent' }
  | {
      state: 'valid'
      format: 'file' | 'legacy-directory'
      record: DatabaseOwnerRecord
    }
  | { state: 'malformed'; error?: unknown }

export function acquireDatabaseOwner(ownerPath: string, runtimeGeneration: string): void {
  const owner: DatabaseOwnerRecord = { pid: process.pid, runtimeGeneration }
  try {
    publishDatabaseOwnerRecord(ownerPath, owner)
    assertCanonicalGeneration(ownerPath, runtimeGeneration)
    return
  } catch (error) {
    if (!isAlreadyExists(error)) throw error
  }

  const existing = inspectDatabaseOwner(ownerPath)
  if (existing.state === 'absent' || (
    existing.state === 'valid' && isLiveProcess(existing.record.pid)
  )) {
    throw ownershipConflict()
  }
  // A malformed observation may be the brief legacy-directory removal window.
  // Re-inspect it only after joining the same takeover fence used by stale owners.
  takeOverStaleOwner(ownerPath, owner)
}

export function assertNoLiveDatabaseOwner(ownerPath: string): void {
  const existing = inspectDatabaseOwner(ownerPath)
  if (existing.state === 'malformed') throw malformedOwner(ownerPath, existing.error)
  if (existing.state === 'absent' || isLiveProcess(existing.record.pid)) {
    throw ownershipConflict()
  }
}

export function releaseDatabaseOwner(ownerPath: string, runtimeGeneration: string): void {
  const existing = inspectDatabaseOwner(ownerPath)
  if (
    existing.state === 'valid' &&
    existing.format === 'file' &&
    existing.record.runtimeGeneration === runtimeGeneration
  ) {
    rmSync(ownerPath, { force: true })
    syncDirectory(dirname(ownerPath))
  }
}

export function readDatabaseOwner(ownerPath: string): DatabaseOwnerRecord | undefined {
  const inspection = inspectDatabaseOwner(ownerPath)
  return inspection.state === 'valid' ? inspection.record : undefined
}

export interface DatabaseRecoveryActionFenceObserver {
  beforeCommit?(): void
  beforeClose?(): void
}

export async function withDatabaseRecoveryActionFence<T>(
  databasePath: string,
  operation: () => Promise<T>,
  observer: DatabaseRecoveryActionFenceObserver = {}
): Promise<T> {
  const path = `${databasePath}.recovery-action.sqlite`
  prepareTakeoverSidecar(path)
  let lock: DatabaseSyncType | undefined
  let transactionOpen = false
  let outcome!: T
  let completed = false
  let failure: unknown
  try {
    lock = new DatabaseSync(path) as DatabaseSyncType
    repairSameUserMode(path, lstatSync(path), 0o600)
    lock.exec('PRAGMA busy_timeout = 5000; BEGIN EXCLUSIVE;')
    transactionOpen = true
    outcome = await operation()
    observer.beforeCommit?.()
    lock.exec('COMMIT')
    transactionOpen = false
    completed = true
  } catch (error) {
    failure = error
    if (transactionOpen) {
      try {
        lock?.exec('ROLLBACK')
      } catch {
        // Closing the SQLite handle releases the OS lock after a failed rollback.
      }
    }
  }
  try {
    observer.beforeClose?.()
  } catch (error) {
    failure ??= error
  }
  try {
    lock?.close()
  } catch (error) {
    failure ??= error
  }
  if (failure !== undefined) throw failure
  if (!completed) throw new Error('database recovery action fence did not complete')
  return outcome
}

/**
 * Publishes a complete owner payload with one atomic namespace operation.
 * A crash before link leaves only a non-canonical temporary inode; a crash
 * after link leaves a fully written canonical record.
 */
export function publishDatabaseOwnerRecord(
  ownerPath: string,
  owner: DatabaseOwnerRecord,
  observer: DatabaseOwnerPublicationObserver = {}
): void {
  const partialPath = `${ownerPath}.claim-${owner.runtimeGeneration}-${randomUUID()}`
  let descriptor: number | undefined
  try {
    descriptor = openSync(
      partialPath,
      constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY,
      0o600
    )
    writeFileSync(descriptor, JSON.stringify(owner), 'utf8')
    fsyncSync(descriptor)
    closeSync(descriptor)
    descriptor = undefined
    observer.onPrepared?.()
    linkSync(partialPath, ownerPath)
    syncDirectory(dirname(ownerPath))
    observer.onPublished?.()
  } finally {
    if (descriptor !== undefined) closeSync(descriptor)
    rmSync(partialPath, { force: true })
  }
}

export function isDatabaseOwnershipRecoveryError(
  error: unknown
): error is DatabaseOwnershipRecoveryError {
  return error instanceof DatabaseOwnershipRecoveryError || (
    typeof error === 'object' && error !== null &&
    'code' in error && error.code === 'OWNERSHIP_RECOVERY_REQUIRED' &&
    'issue' in error &&
    ['owner-record-malformed', 'takeover-sidecar-unusable'].includes(String(error.issue))
  )
}

function takeOverStaleOwner(ownerPath: string, owner: DatabaseOwnerRecord): void {
  try {
    withTakeoverLock(`${ownerPath}.takeover.sqlite`, () => {
      rmSync(`${ownerPath}.takeover`, { recursive: true, force: true })
      const stale = inspectDatabaseOwner(ownerPath)
      if (stale.state === 'malformed') throw malformedOwner(ownerPath, stale.error)
      if (stale.state === 'valid' && isLiveProcess(stale.record.pid)) throw ownershipConflict()
      if (stale.state === 'valid') {
        rmSync(ownerPath, {
          recursive: stale.format === 'legacy-directory',
          force: true
        })
      }
      try {
        publishDatabaseOwnerRecord(ownerPath, owner)
      } catch (error) {
        if (isAlreadyExists(error)) throw ownershipConflict()
        throw error
      }
      assertCanonicalGeneration(ownerPath, owner.runtimeGeneration)
    })
  } catch (error) {
    if (isDatabaseOwnershipRecoveryError(error)) {
      releaseDatabaseOwner(ownerPath, owner.runtimeGeneration)
    }
    throw error
  }
}

function inspectDatabaseOwner(ownerPath: string): OwnerInspection {
  let metadata: Stats
  try {
    metadata = lstatSync(ownerPath)
  } catch (error) {
    return errorCode(error) === 'ENOENT'
      ? { state: 'absent' }
      : { state: 'malformed', error }
  }

  const format = metadata.isDirectory() ? 'legacy-directory' : 'file'
  if (format === 'file' && !metadata.isFile()) {
    return { state: 'malformed', error: new Error('owner path is not a regular file') }
  }
  const recordPath = format === 'legacy-directory' ? join(ownerPath, 'owner.json') : ownerPath
  if (format === 'file') repairSameUserMode(recordPath, metadata, 0o600)
  try {
    const value = JSON.parse(readFileSync(recordPath, 'utf8')) as Partial<DatabaseOwnerRecord>
    return isOwnerRecord(value)
      ? { state: 'valid', format, record: value }
      : { state: 'malformed', error: new Error('owner payload is invalid') }
  } catch (error) {
    return { state: 'malformed', error }
  }
}

function isOwnerRecord(value: Partial<DatabaseOwnerRecord>): value is DatabaseOwnerRecord {
  return Number.isSafeInteger(value.pid) && Number(value.pid) > 0 &&
    typeof value.runtimeGeneration === 'string' && value.runtimeGeneration.length > 0
}

function assertCanonicalGeneration(ownerPath: string, runtimeGeneration: string): void {
  const canonical = inspectDatabaseOwner(ownerPath)
  if (
    canonical.state !== 'valid' ||
    canonical.format !== 'file' ||
    canonical.record.pid !== process.pid ||
    canonical.record.runtimeGeneration !== runtimeGeneration
  ) {
    throw new Error('database ownership claim lost before publication')
  }
}

function withTakeoverLock(path: string, operation: () => void): void {
  prepareTakeoverSidecar(path)
  let lock: DatabaseSyncType | undefined
  let transactionOpen = false
  try {
    lock = new DatabaseSync(path) as DatabaseSyncType
    repairSameUserMode(path, lstatSync(path), 0o600)
    lock.exec('PRAGMA busy_timeout = 5000; BEGIN EXCLUSIVE;')
    transactionOpen = true
  } catch (error) {
    try { lock?.close() } catch { /* the OS releases any acquired lock on process exit */ }
    if (/database is locked/i.test(errorMessage(error))) throw error
    throw unusableTakeoverSidecar(path, error)
  }

  let operationFailed = false
  try {
    operation()
    lock.exec('COMMIT')
    transactionOpen = false
  } catch (error) {
    operationFailed = true
    if (transactionOpen) {
      try {
        lock.exec('ROLLBACK')
      } catch {
        // Closing the SQLite handle still releases the OS lock after a failed rollback.
      }
    }
    if (isDatabaseOwnershipRecoveryError(error) || errorMessage(error).includes('already owned')) {
      throw error
    }
    throw unusableTakeoverSidecar(path, error)
  } finally {
    try {
      lock.close()
    } catch (error) {
      if (!operationFailed) throw unusableTakeoverSidecar(path, error)
    }
  }
}

function prepareTakeoverSidecar(path: string): void {
  let metadata: Stats
  try {
    metadata = lstatSync(path)
  } catch (error) {
    if (errorCode(error) === 'ENOENT') return
    throw unusableTakeoverSidecar(path, error)
  }
  if (!metadata.isFile()) {
    throw unusableTakeoverSidecar(path, new Error('takeover sidecar is not a regular file'))
  }
  repairSameUserMode(path, metadata, 0o600)
}

function repairSameUserMode(
  path: string,
  metadata: Stats,
  desiredMode: number
): void {
  const currentUid = process.getuid?.()
  if (currentUid === undefined || metadata.uid !== currentUid) return
  if ((metadata.mode & 0o777) === desiredMode) return
  try {
    chmodSync(path, desiredMode)
  } catch {
    // The subsequent read/open classifies a still-unusable auxiliary state.
  }
}

function syncDirectory(path: string): void {
  let descriptor: number | undefined
  try {
    descriptor = openSync(path, constants.O_RDONLY)
    fsyncSync(descriptor)
  } finally {
    if (descriptor !== undefined) closeSync(descriptor)
  }
}

function malformedOwner(path: string, cause?: unknown): DatabaseOwnershipRecoveryError {
  return new DatabaseOwnershipRecoveryError('owner-record-malformed', path, cause)
}

function unusableTakeoverSidecar(
  path: string,
  cause?: unknown
): DatabaseOwnershipRecoveryError {
  return new DatabaseOwnershipRecoveryError('takeover-sidecar-unusable', path, cause)
}

function ownershipConflict(): Error {
  return new Error('database is already owned by a live Runtime')
}

function isLiveProcess(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === 'EPERM'
  }
}

function isAlreadyExists(error: unknown): boolean {
  return errorCode(error) === 'EEXIST'
}

function errorCode(error: unknown): string | undefined {
  return typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : undefined
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
