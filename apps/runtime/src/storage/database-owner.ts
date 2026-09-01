import { existsSync, lstatSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { DatabaseSync as DatabaseSyncType } from 'node:sqlite'

const { DatabaseSync } = process.getBuiltinModule(
  'node:sqlite'
) as typeof import('node:sqlite')

export interface DatabaseOwnerRecord {
  pid: number
  runtimeGeneration: string
}

export function acquireDatabaseOwner(ownerPath: string, runtimeGeneration: string): void {
  const owner: DatabaseOwnerRecord = { pid: process.pid, runtimeGeneration }
  try {
    writeOwnerExclusive(ownerPath, owner)
    assertCanonicalGeneration(ownerPath, runtimeGeneration)
    return
  } catch (error) {
    if (!isAlreadyExists(error)) throw error
  }

  const existing = inspectDatabaseOwner(ownerPath)
  if (existing === undefined || isLiveProcess(existing.record.pid)) {
    throw ownershipConflict()
  }
  takeOverStaleOwner(ownerPath, owner)
}

export function assertNoLiveDatabaseOwner(ownerPath: string): void {
  const existing = inspectDatabaseOwner(ownerPath)?.record
  if (existing === undefined) {
    throw ownershipConflict()
  }
  if (isLiveProcess(existing.pid)) {
    throw ownershipConflict()
  }
}

export function releaseDatabaseOwner(ownerPath: string, runtimeGeneration: string): void {
  const existing = inspectDatabaseOwner(ownerPath)
  if (existing?.format === 'file' && existing.record.runtimeGeneration === runtimeGeneration) {
    rmSync(ownerPath, { force: true })
  }
}

export function readDatabaseOwner(ownerPath: string): DatabaseOwnerRecord | undefined {
  return inspectDatabaseOwner(ownerPath)?.record
}

function takeOverStaleOwner(ownerPath: string, owner: DatabaseOwnerRecord): void {
  withTakeoverLock(`${ownerPath}.takeover.sqlite`, () => {
    rmSync(`${ownerPath}.takeover`, { recursive: true, force: true })
    const stale = inspectDatabaseOwner(ownerPath)
    if (existsSync(ownerPath) && stale === undefined) throw ownershipConflict()
    if (stale && isLiveProcess(stale.record.pid)) throw ownershipConflict()
    if (stale) {
      rmSync(ownerPath, { recursive: stale.format === 'legacy-directory', force: true })
    }
    try {
      writeOwnerExclusive(ownerPath, owner)
    } catch (error) {
      if (isAlreadyExists(error)) throw ownershipConflict()
      throw error
    }
    assertCanonicalGeneration(ownerPath, owner.runtimeGeneration)
  })
}

function inspectDatabaseOwner(ownerPath: string): {
  format: 'file' | 'legacy-directory'
  record: DatabaseOwnerRecord
} | undefined {
  try {
    const metadata = lstatSync(ownerPath)
    const format = metadata.isDirectory() ? 'legacy-directory' : 'file'
    if (format === 'file' && !metadata.isFile()) return undefined
    const recordPath = format === 'legacy-directory' ? join(ownerPath, 'owner.json') : ownerPath
    const value = JSON.parse(readFileSync(recordPath, 'utf8')) as Partial<DatabaseOwnerRecord>
    return isOwnerRecord(value) ? { format, record: value } : undefined
  } catch {
    return undefined
  }
}

function isOwnerRecord(value: Partial<DatabaseOwnerRecord>): value is DatabaseOwnerRecord {
  return Number.isSafeInteger(value.pid) && Number(value.pid) > 0 &&
    typeof value.runtimeGeneration === 'string'
}

function writeOwnerExclusive(ownerPath: string, owner: DatabaseOwnerRecord): void {
  writeFileSync(ownerPath, JSON.stringify(owner), { flag: 'wx', mode: 0o600 })
}

function assertCanonicalGeneration(ownerPath: string, runtimeGeneration: string): void {
  const canonical = inspectDatabaseOwner(ownerPath)
  if (
    canonical?.format !== 'file' ||
    canonical.record.pid !== process.pid ||
    canonical.record.runtimeGeneration !== runtimeGeneration
  ) {
    throw new Error('database ownership claim lost before publication')
  }
}

function withTakeoverLock(path: string, operation: () => void): void {
  const lock = new DatabaseSync(path) as DatabaseSyncType
  let transactionOpen = false
  try {
    lock.exec('PRAGMA busy_timeout = 5000; BEGIN EXCLUSIVE;')
    transactionOpen = true
    operation()
    lock.exec('COMMIT')
    transactionOpen = false
  } catch (error) {
    if (transactionOpen) {
      try {
        lock.exec('ROLLBACK')
      } catch {
        // Closing the SQLite handle still releases the OS lock after a failed rollback.
      }
    }
    throw error
  } finally {
    lock.close()
  }
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
  return (error as NodeJS.ErrnoException).code === 'EEXIST'
}
