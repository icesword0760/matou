import { randomUUID } from 'node:crypto'
import {
  existsSync,
  mkdirSync,
  statSync
} from 'node:fs'
import { dirname } from 'node:path'
import { pathToFileURL } from 'node:url'
import type {
  DatabaseSync as DatabaseSyncType,
  SQLInputValue,
  StatementResultingChanges
} from 'node:sqlite'

import { StorageQueue } from './storage-queue'
import {
  acquireDatabaseOwner,
  assertNoLiveDatabaseOwner,
  releaseDatabaseOwner
} from './database-owner'

// Electron 43 embeds Node 24 with node:sqlite, while the current esbuild
// builtin table rewrites a normal node:sqlite import to require("sqlite").
// Loading through Node's builtin registry preserves the real builtin at runtime.
const { DatabaseSync, backup } = process.getBuiltinModule(
  'node:sqlite'
) as typeof import('node:sqlite')
const DATABASE_CONSTRUCTOR_TOKEN = Symbol('RuntimeDatabase.constructor')

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

export class StorageReadOnlyError extends Error {
  readonly code = 'STORAGE_READ_ONLY' as const

  constructor() {
    super('STORAGE_READ_ONLY: database is open in read-only recovery mode')
    this.name = 'StorageReadOnlyError'
  }
}

export interface RuntimeDatabaseOwnership {
  readonly runtimeGeneration: string
  openReadOnly(): RuntimeDatabase
  openWritable(): RuntimeDatabase
  release(): void
}

export class RuntimeDatabase implements DatabaseTransaction {
  readonly runtimeGeneration: string
  readonly path: string
  readonly readOnly: boolean
  readonly #connection: DatabaseSyncType
  readonly #queue = new StorageQueue()
  readonly #ownerPath: string | undefined
  #statementCount: number | undefined
  #statementProfile: Map<string, number> | undefined
  #closed = false

  constructor(
    token: typeof DATABASE_CONSTRUCTOR_TOKEN,
    path: string,
    generation: string,
    ownerPath: string | undefined,
    readOnly: boolean
  ) {
    if (token !== DATABASE_CONSTRUCTOR_TOKEN) throw new Error('invalid RuntimeDatabase constructor')
    this.path = path
    this.runtimeGeneration = generation
    this.#ownerPath = ownerPath
    this.readOnly = readOnly
    this.#connection = readOnly ? openReadOnlyConnection(path) : new DatabaseSync(path)

    try {
      if (readOnly) {
        return
      }
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
      if (ownerPath !== undefined) releaseDatabaseOwner(ownerPath, generation)
      throw error
    }
  }

  static open(path: string): RuntimeDatabase {
    return RuntimeDatabase.acquireOwnership(path).openWritable()
  }

  static acquireOwnership(path: string): RuntimeDatabaseOwnership {
    mkdirSync(dirname(path), { recursive: true })
    return new RuntimeDatabaseOwnershipLease(path)
  }

  static openReadOnly(path: string): RuntimeDatabase {
    return new RuntimeDatabase(DATABASE_CONSTRUCTOR_TOKEN, path, randomUUID(), undefined, true)
  }

  static assertNoLiveOwner(path: string): void {
    const ownerPath = `${path}.owner`
    if (!existsSync(ownerPath)) return
    assertNoLiveDatabaseOwner(ownerPath)
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
    this.#assertWritable()
    this.#countStatement(sql)
    this.#connection.exec(sql)
  }

  run(sql: string, ...params: SQLInputValue[]): StatementResultingChanges {
    this.#assertOpen()
    this.#assertWritable()
    this.#countStatement(sql)
    return this.#connection.prepare(sql).run(...params)
  }

  get<T extends object>(sql: string, ...params: SQLInputValue[]): T | undefined {
    this.#assertOpen()
    this.#countStatement(sql)
    return this.#connection.prepare(sql).get(...params) as T | undefined
  }

  all<T extends object>(sql: string, ...params: SQLInputValue[]): T[] {
    this.#assertOpen()
    this.#countStatement(sql)
    return this.#connection.prepare(sql).all(...params) as T[]
  }

  /** Enables a measurement window on reset and returns only statements executed through this authority. */
  readStatementCount(reset = false): number {
    const count = this.#statementCount ?? 0
    if (reset) {
      this.#statementCount = 0
      this.#statementProfile = new Map()
    }
    return count
  }

  readStatementProfile(): Array<{ statement: string; count: number }> {
    return [...(this.#statementProfile ?? new Map())]
      .map(([statement, count]) => ({ statement, count }))
      .sort((left, right) => right.count - left.count || left.statement.localeCompare(right.statement))
      .slice(0, 12)
  }

  transaction<T>(callback: (transaction: DatabaseTransaction) => T): T {
    this.#assertOpen()
    this.#assertWritable()
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
    this.#assertWritable()
    return this.#queue.enqueue(operation)
  }

  async backupTo(path: string): Promise<void> {
    this.#assertOpen()
    await backup(this.#connection, path)
  }

  close(): void {
    if (this.#closed) return
    this.#closeConnection()
    if (this.#ownerPath !== undefined) {
      releaseDatabaseOwner(this.#ownerPath, this.runtimeGeneration)
    }
  }

  closeRetainingOwnership(): RuntimeDatabaseOwnership {
    if (this.#closed) throw new Error('database is closed')
    if (this.#ownerPath === undefined) {
      throw new Error('read-only database does not own the Runtime database fence')
    }
    this.#closeConnection()
    return RuntimeDatabaseOwnershipLease.adopt(
      this.path,
      this.runtimeGeneration,
      this.#ownerPath
    )
  }

  #closeConnection(): void {
    if (this.#closed) {
      return
    }
    this.#queue.close()
    this.#connection.close()
    this.#closed = true
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

  #assertWritable(): void {
    if (this.readOnly) throw new StorageReadOnlyError()
  }

  #countStatement(sql: string): void {
    if (this.#statementCount === undefined) return
    this.#statementCount += 1
    if (this.#statementProfile) {
      const statement = sql.replace(/\s+/g, ' ').trim().slice(0, 240)
      this.#statementProfile.set(statement, (this.#statementProfile.get(statement) ?? 0) + 1)
    }
  }
}

class RuntimeDatabaseOwnershipLease implements RuntimeDatabaseOwnership {
  readonly runtimeGeneration: string
  readonly #path: string
  readonly #ownerPath: string
  #held = true

  constructor(
    path: string,
    runtimeGeneration: string = randomUUID(),
    ownerPath = `${path}.owner`,
    acquire = true
  ) {
    this.#path = path
    this.runtimeGeneration = runtimeGeneration
    this.#ownerPath = ownerPath
    if (acquire) acquireDatabaseOwner(this.#ownerPath, this.runtimeGeneration)
  }

  static adopt(
    path: string,
    runtimeGeneration: string,
    ownerPath: string
  ): RuntimeDatabaseOwnershipLease {
    return new RuntimeDatabaseOwnershipLease(path, runtimeGeneration, ownerPath, false)
  }

  openReadOnly(): RuntimeDatabase {
    this.#assertHeld()
    return new RuntimeDatabase(
      DATABASE_CONSTRUCTOR_TOKEN,
      this.#path,
      this.runtimeGeneration,
      undefined,
      true
    )
  }

  openWritable(): RuntimeDatabase {
    this.#assertHeld()
    this.#held = false
    try {
      return new RuntimeDatabase(
        DATABASE_CONSTRUCTOR_TOKEN,
        this.#path,
        this.runtimeGeneration,
        this.#ownerPath,
        false
      )
    } catch (error) {
      releaseDatabaseOwner(this.#ownerPath, this.runtimeGeneration)
      throw error
    }
  }

  release(): void {
    if (!this.#held) return
    releaseDatabaseOwner(this.#ownerPath, this.runtimeGeneration)
    this.#held = false
  }

  #assertHeld(): void {
    if (!this.#held) throw new Error('database ownership has already been released')
  }
}

function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    'then' in value &&
    typeof (value as { then?: unknown }).then === 'function'
  )
}

function openReadOnlyConnection(path: string): DatabaseSyncType {
  const connection = new DatabaseSync(path, { readOnly: true })
  try {
    connection.exec('PRAGMA query_only = ON;')
    connection.prepare('PRAGMA schema_version').get()
    return connection
  } catch (error) {
    connection.close()
    if (!/attempt to write a readonly database/i.test(errorMessage(error))) throw error
    if (hasCommittedWal(path)) {
      throw new Error('database WAL requires recovery before read-only open')
    }
  }

  const immutableUrl = pathToFileURL(path)
  immutableUrl.searchParams.set('immutable', '1')
  const immutable = new DatabaseSync(immutableUrl, { readOnly: true })
  try {
    immutable.exec('PRAGMA query_only = ON;')
    return immutable
  } catch (error) {
    immutable.close()
    throw error
  }
}

function hasCommittedWal(path: string): boolean {
  const walPath = `${path}-wal`
  return existsSync(walPath) && statSync(walPath).size > 32
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
