import { createHash, randomUUID } from 'node:crypto'
import {
  copyFile,
  link,
  mkdir,
  readFile,
  readdir,
  rename as renameFile,
  rm,
  stat,
  writeFile
} from 'node:fs/promises'
import { basename, dirname, join, resolve } from 'node:path'

import type { RuntimeDatabase } from './database'

const { DatabaseSync, backup } = process.getBuiltinModule(
  'node:sqlite'
) as typeof import('node:sqlite')

export type DatabaseBackupReason = 'pre-migration' | 'clean-exit'

export interface DatabaseBackupDescriptor {
  id: string
  path: string
  createdAt: number
  reason: DatabaseBackupReason
  schemaVersion: number
  size: number
  sha256: string
}

interface DatabaseBackupServiceOptions {
  now?: () => number
  rename?: (from: string, to: string) => Promise<void>
}

const BACKUP_NAME = /^matou-(\d+)-(pre-migration|clean-exit)-v(\d+)\.json$/

export class DatabaseBackupService {
  readonly #backupDirectory: string
  readonly #now: () => number
  readonly #rename: (from: string, to: string) => Promise<void>
  #lastTimestamp = 0

  constructor(dataRoot: string, options: DatabaseBackupServiceOptions = {}) {
    this.#backupDirectory = join(resolve(dataRoot), 'backups')
    this.#now = options.now ?? Date.now
    this.#rename = options.rename ?? renameFile
  }

  async create(
    database: RuntimeDatabase,
    reason: DatabaseBackupReason
  ): Promise<DatabaseBackupDescriptor> {
    await mkdir(this.#backupDirectory, { recursive: true })
    const createdAt = this.#nextTimestamp()
    const schemaVersion = readSchemaVersion(database)
    const id = `matou-${createdAt}-${reason}-v${schemaVersion}`
    const path = join(this.#backupDirectory, `${id}.sqlite`)
    const partialPath = `${path}.partial`
    const manifestPath = join(this.#backupDirectory, `${id}.json`)
    const partialManifestPath = `${manifestPath}.partial`

    try {
      await database.backupTo(partialPath)
      const { size, sha256 } = await inspectBackup(partialPath)
      const descriptor: DatabaseBackupDescriptor = {
        id,
        path,
        createdAt,
        reason,
        schemaVersion,
        size,
        sha256
      }
      await this.#rename(partialPath, path)
      await writeFile(partialManifestPath, JSON.stringify(descriptor), {
        encoding: 'utf8',
        mode: 0o600
      })
      await this.#rename(partialManifestPath, manifestPath)
      return descriptor
    } catch (error) {
      await rm(partialPath, { force: true }).catch(() => undefined)
      await rm(partialManifestPath, { force: true }).catch(() => undefined)
      throw error
    }
  }

  async listValid(): Promise<DatabaseBackupDescriptor[]> {
    const names = await readdir(this.#backupDirectory).catch((error: unknown) => {
      if (errorCode(error) === 'ENOENT') return []
      throw error
    })
    const descriptors: DatabaseBackupDescriptor[] = []
    for (const name of names) {
      const match = BACKUP_NAME.exec(name)
      if (!match) continue
      const manifestPath = join(this.#backupDirectory, name)
      try {
        const parsed = JSON.parse(await readFile(manifestPath, 'utf8')) as unknown
        const descriptor = validateManifest(parsed, this.#backupDirectory, match)
        await verifyBackup(descriptor.path, descriptor.size, descriptor.sha256)
        descriptors.push(descriptor)
      } catch {
        // Incomplete, edited, or corrupt artifacts are intentionally absent from recovery choices.
      }
    }
    return descriptors.sort((left, right) =>
      right.createdAt - left.createdAt || right.id.localeCompare(left.id)
    )
  }

  async restore(backupId: string, targetDatabasePath: string): Promise<void> {
    const descriptor = (await this.listValid()).find(({ id }) => id === backupId)
    if (!descriptor) throw new Error(`backup ${backupId} is missing or invalid`)

    const targetPath = resolve(targetDatabasePath)
    await mkdir(dirname(targetPath), { recursive: true })
    const restoredAt = this.#nextTimestamp()
    const temporaryPath = `${targetPath}.restore-${restoredAt}-${randomUUID()}.partial`
    const replacedPath = `${targetPath}.replaced-${restoredAt}`
    const replacedPartialPath = `${replacedPath}.partial`

    try {
      await copyFile(descriptor.path, temporaryPath)
      await verifyBackup(temporaryPath, descriptor.size, descriptor.sha256)

      const targetExists = await isFile(targetPath)
      if (targetExists) {
        const oldDatabase = new DatabaseSync(targetPath, { readOnly: true })
        try {
          await backup(oldDatabase, replacedPartialPath)
        } finally {
          oldDatabase.close()
        }
        await inspectBackup(replacedPartialPath)
        await this.#rename(replacedPartialPath, replacedPath)
      }

      try {
        await this.#rename(temporaryPath, targetPath)
      } catch (error) {
        if (!targetExists) {
          await link(temporaryPath, targetPath)
          await verifyBackup(targetPath, descriptor.size, descriptor.sha256)
          await rm(`${targetPath}-wal`, { force: true })
          await rm(`${targetPath}-shm`, { force: true })
        }
        throw error
      }
      await rm(`${targetPath}-wal`, { force: true })
      await rm(`${targetPath}-shm`, { force: true })
    } finally {
      await rm(temporaryPath, { force: true }).catch(() => undefined)
      await rm(replacedPartialPath, { force: true }).catch(() => undefined)
    }
  }

  async rotate(maxCount = 7): Promise<void> {
    if (!Number.isSafeInteger(maxCount) || maxCount < 0) {
      throw new Error('backup retention count must be a non-negative integer')
    }
    const expired = (await this.listValid()).slice(maxCount)
    for (const descriptor of expired) {
      await rm(descriptor.path, { force: true })
      await rm(join(this.#backupDirectory, `${descriptor.id}.json`), { force: true })
    }
  }

  #nextTimestamp(): number {
    const timestamp = Math.max(this.#now(), this.#lastTimestamp + 1)
    this.#lastTimestamp = timestamp
    return timestamp
  }

}

function readSchemaVersion(database: RuntimeDatabase): number {
  const hasHistory = database.get<{ present: number }>(
    `SELECT 1 AS present FROM sqlite_master
     WHERE type = 'table' AND name = 'schema_migrations'`
  )
  if (!hasHistory) return 0
  return database.get<{ version: number | null }>(
    'SELECT MAX(version) AS version FROM schema_migrations'
  )?.version ?? 0
}

async function inspectBackup(path: string): Promise<{ size: number; sha256: string }> {
  assertIntegrity(path)
  const bytes = await readFile(path)
  return {
    size: bytes.byteLength,
    sha256: createHash('sha256').update(bytes).digest('hex')
  }
}

async function verifyBackup(path: string, expectedSize: number, expectedSha256: string): Promise<void> {
  const metadata = await stat(path)
  if (!metadata.isFile() || metadata.size !== expectedSize) {
    throw new Error('backup size does not match its manifest')
  }
  const bytes = await readFile(path)
  const sha256 = createHash('sha256').update(bytes).digest('hex')
  if (sha256 !== expectedSha256) throw new Error('backup checksum does not match its manifest')
  assertIntegrity(path)
}

function assertIntegrity(path: string): void {
  const database = new DatabaseSync(path, { readOnly: true })
  try {
    const results = database.prepare('PRAGMA integrity_check').all() as Record<string, unknown>[]
    if (
      results.length !== 1 ||
      String(Object.values(results[0] ?? {})[0]).toLowerCase() !== 'ok'
    ) {
      throw new Error('SQLite integrity check failed')
    }
  } finally {
    database.close()
  }
}

function validateManifest(
  value: unknown,
  backupDirectory: string,
  match: RegExpExecArray
): DatabaseBackupDescriptor {
  if (typeof value !== 'object' || value === null) throw new Error('invalid backup manifest')
  const descriptor = value as Partial<DatabaseBackupDescriptor>
  const createdAt = Number(match[1])
  const reason = match[2] as DatabaseBackupReason
  const schemaVersion = Number(match[3])
  const id = basename(match[0]!, '.json')
  const path = join(backupDirectory, `${id}.sqlite`)
  if (
    descriptor.id !== id ||
    descriptor.path !== path ||
    descriptor.createdAt !== createdAt ||
    descriptor.reason !== reason ||
    descriptor.schemaVersion !== schemaVersion ||
    !Number.isSafeInteger(descriptor.size) ||
    Number(descriptor.size) < 0 ||
    typeof descriptor.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(descriptor.sha256)
  ) {
    throw new Error('invalid backup manifest')
  }
  return descriptor as DatabaseBackupDescriptor
}

function errorCode(error: unknown): string | undefined {
  return typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : undefined
}

async function isFile(path: string): Promise<boolean> {
  try {
    return (await stat(path)).isFile()
  } catch (error) {
    if (errorCode(error) === 'ENOENT') return false
    throw error
  }
}
