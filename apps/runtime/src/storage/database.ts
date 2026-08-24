import { randomUUID } from 'node:crypto'
import {
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync
} from 'node:fs'
import { dirname } from 'node:path'
import type {
  DatabaseSync as DatabaseSyncType,
  SQLInputValue,
  StatementResultingChanges
} from 'node:sqlite'

import { StorageQueue } from './storage-queue'

// Electron 43 embeds Node 24 with node:sqlite, while the current esbuild
// builtin table rewrites a normal node:sqlite import to require("sqlite").
// Loading through Node's builtin registry preserves the real builtin at runtime.
const { DatabaseSync, backup } = process.getBuiltinModule(
  'node:sqlite'
) as typeof import('node:sqlite')

export interface DatabasePragmas {
  journalMode: string
  foreignKeys: boolean
  synchronous: number
  busyTimeout: number
  trustedSchema: boolean
}

export interface DatabaseTransaction {
  run(sql: string, ...params: SQLInputValue[]): StatementResultingChanges
  get<T extends object>(sql: string, ...params: SQLInputValue[]): T | undefined
  all<T extends object>(sql: string, ...params: SQLInputValue[]): T[]
  exec(sql: string): void
}

interface OwnerRecord {
  pid: number
  runtimeGeneration: string
}

export class RuntimeDatabase implements DatabaseTransaction {
  readonly runtimeGeneration: string
  readonly path: string
  readonly #connection: DatabaseSyncType
  readonly #queue = new StorageQueue()
  readonly #ownerDirectory: string
  #closed = false

  private constructor(path: string, generation: string, ownerDirectory: string) {
    this.path = path
    this.runtimeGeneration = generation
    this.#ownerDirectory = ownerDirectory
    this.#connection = new DatabaseSync(path)

    try {
      this.#connection.exec(`
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;
        PRAGMA synchronous = FULL;
        PRAGMA busy_timeout = 5000;
        PRAGMA trusted_schema = OFF;
      `)
      this.#connection.exec(`
        CREATE TABLE IF NOT EXISTS _runtime_meta (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        ) STRICT;
      `)
      this.#connection
        .prepare('INSERT OR REPLACE INTO _runtime_meta (key, value) VALUES (?, ?)')
        .run('runtime_generation', generation)
    } catch (error) {
      this.#connection.close()
      releaseOwner(ownerDirectory, generation)
      throw error
    }
  }

  static open(path: string): RuntimeDatabase {
    mkdirSync(dirname(path), { recursive: true })
    const runtimeGeneration = randomUUID()
    const ownerDirectory = `${path}.owner`
    acquireOwner(ownerDirectory, runtimeGeneration)
    return new RuntimeDatabase(path, runtimeGeneration, ownerDirectory)
  }

  pragmas(): DatabasePragmas {
    return {
      journalMode: this.#pragmaString('journal_mode'),
      foreignKeys: this.#pragmaNumber('foreign_keys') === 1,
      synchronous: this.#pragmaNumber('synchronous'),
      busyTimeout: this.#pragmaNumber('busy_timeout'),
      trustedSchema: this.#pragmaNumber('trusted_schema') === 1
    }
  }

  exec(sql: string): void {
    this.#assertOpen()
    this.#connection.exec(sql)
  }

  run(sql: string, ...params: SQLInputValue[]): StatementResultingChanges {
    this.#assertOpen()
    return this.#connection.prepare(sql).run(...params)
  }

  get<T extends object>(sql: string, ...params: SQLInputValue[]): T | undefined {
    this.#assertOpen()
    return this.#connection.prepare(sql).get(...params) as T | undefined
  }

  all<T extends object>(sql: string, ...params: SQLInputValue[]): T[] {
    this.#assertOpen()
    return this.#connection.prepare(sql).all(...params) as T[]
  }

  transaction<T>(callback: (transaction: DatabaseTransaction) => T): T {
    this.#assertOpen()
    this.#connection.exec('BEGIN IMMEDIATE')
    try {
      const result = callback(this)
      if (isPromiseLike(result)) {
        throw new Error('database transaction callbacks must be synchronous')
      }
      this.#connection.exec('COMMIT')
      return result
    } catch (error) {
      this.#connection.exec('ROLLBACK')
      throw error
    }
  }

  enqueueWrite<T>(operation: () => T | Promise<T>): Promise<T> {
    this.#assertOpen()
    return this.#queue.enqueue(operation)
  }

  async backupTo(path: string): Promise<void> {
    this.#assertOpen()
    await backup(this.#connection, path)
  }

  close(): void {
    if (this.#closed) {
      return
    }
    this.#closed = true
    this.#queue.close()
    this.#connection.close()
    releaseOwner(this.#ownerDirectory, this.runtimeGeneration)
  }

  #pragmaNumber(name: string): number {
    const row = this.get<Record<string, number>>(`PRAGMA ${name}`)
    return row === undefined ? Number.NaN : Number(Object.values(row)[0])
  }

  #pragmaString(name: string): string {
    const row = this.get<Record<string, string>>(`PRAGMA ${name}`)
    return row === undefined ? '' : String(Object.values(row)[0]).toLowerCase()
  }

  #assertOpen(): void {
    if (this.#closed) {
      throw new Error('database is closed')
    }
  }
}

function acquireOwner(ownerDirectory: string, runtimeGeneration: string): void {
  const owner: OwnerRecord = { pid: process.pid, runtimeGeneration }
  try {
    mkdirSync(ownerDirectory)
  } catch (error) {
    if (!isAlreadyExists(error)) {
      throw error
    }

    const existing = readOwner(ownerDirectory)
    if (existing !== undefined && isLiveProcess(existing.pid)) {
      throw new Error('database is already owned by a live Runtime')
    }

    rmSync(ownerDirectory, { recursive: true, force: true })
    mkdirSync(ownerDirectory)
  }
  writeFileSync(`${ownerDirectory}/owner.json`, JSON.stringify(owner), { mode: 0o600 })
}

function releaseOwner(ownerDirectory: string, runtimeGeneration: string): void {
  const existing = readOwner(ownerDirectory)
  if (existing?.runtimeGeneration === runtimeGeneration) {
    rmSync(ownerDirectory, { recursive: true, force: true })
  }
}

function readOwner(ownerDirectory: string): OwnerRecord | undefined {
  try {
    return JSON.parse(readFileSync(`${ownerDirectory}/owner.json`, 'utf8')) as OwnerRecord
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

function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    'then' in value &&
    typeof (value as { then?: unknown }).then === 'function'
  )
}
