import { readFileSync, rmSync, writeFileSync } from 'node:fs'

export interface DatabaseOwnerRecord {
  pid: number
  runtimeGeneration: string
}

export function acquireDatabaseOwner(ownerPath: string, runtimeGeneration: string): void {
  const owner: DatabaseOwnerRecord = { pid: process.pid, runtimeGeneration }
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      writeFileSync(ownerPath, JSON.stringify(owner), { flag: 'wx', mode: 0o600 })
      return
    } catch (error) {
      if (!isAlreadyExists(error)) throw error
      const existing = readDatabaseOwner(ownerPath)
      if (existing === undefined || isLiveProcess(existing.pid)) {
        throw new Error('database is already owned by a live Runtime')
      }
      releaseDatabaseOwner(ownerPath, existing.runtimeGeneration)
    }
  }
  throw new Error('database ownership could not be acquired')
}

export function assertNoLiveDatabaseOwner(ownerPath: string): void {
  const existing = readDatabaseOwner(ownerPath)
  if (existing === undefined) {
    throw new Error('database is already owned by a live Runtime')
  }
  if (isLiveProcess(existing.pid)) {
    throw new Error('database is already owned by a live Runtime')
  }
}

export function releaseDatabaseOwner(ownerPath: string, runtimeGeneration: string): void {
  const existing = readDatabaseOwner(ownerPath)
  if (existing?.runtimeGeneration === runtimeGeneration) {
    rmSync(ownerPath, { force: true })
  }
}

export function readDatabaseOwner(ownerPath: string): DatabaseOwnerRecord | undefined {
  try {
    const value = JSON.parse(readFileSync(ownerPath, 'utf8')) as Partial<DatabaseOwnerRecord>
    return Number.isSafeInteger(value.pid) && typeof value.runtimeGeneration === 'string'
      ? value as DatabaseOwnerRecord
      : undefined
  } catch {
    return undefined
  }
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
