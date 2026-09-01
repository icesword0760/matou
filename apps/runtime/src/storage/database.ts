import { randomUUID } from 'node:crypto'
import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync
} from 'node:fs'
import { dirname } from 'node:path'
import { pathToFileURL } from 'node:url'
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

interface OwnerRecord {
  pid: number
  runtimeGeneration: string
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
  readonly #ownerDirectory: string | undefined
  #closed = false

  constructor(
    token: typeof DATABASE_CONSTRUCTOR_TOKEN,
    path: string,
    generation: string,
    ownerDirectory: string | undefined,
    readOnly: boolean
  ) {
    if (token !== DATABASE_CONSTRUCTOR_TOKEN) throw new Error('invalid RuntimeDatabase constructor')
    this.path = path
    this.runtimeGeneration = generation
    this.#ownerDirectory = ownerDirectory
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
      if (ownerDirectory !== undefined) releaseOwner(ownerDirectory, generation)
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
    const existing = readOwner(`${path}.owner`)
    if (existing !== undefined && isLiveProcess(existing.pid)) {
      throw new Error('database is already owned by a live Runtime')
    }
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
    this.#connection.exec(sql)
  }

  run(sql: string, ...params: SQLInputValue[]): StatementResultingChanges {
    this.#assertOpen()
    this.#assertWritable()
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
    if (this.#ownerDirectory !== undefined) {
      releaseOwner(this.#ownerDirectory, this.runtimeGeneration)
    }
  }

  closeRetainingOwnership(): RuntimeDatabaseOwnership {
    if (this.#closed) throw new Error('database is closed')
    if (this.#ownerDirectory === undefined) {
      throw new Error('read-only database does not own the Runtime database fence')
    }
    this.#closeConnection()
    return RuntimeDatabaseOwnershipLease.adopt(
      this.path,
      this.runtimeGeneration,
      this.#ownerDirectory
    )
  }

  #closeConnection(): void {
    if (this.#closed) {
      return
    }
    this.#closed = true
    this.#queue.close()
    this.#connection.close()
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
}

class RuntimeDatabaseOwnershipLease implements RuntimeDatabaseOwnership {
  readonly runtimeGeneration: string
  readonly #path: string
  readonly #ownerDirectory: string
  #held = true

  constructor(
    path: string,
    runtimeGeneration: string = randomUUID(),
    ownerDirectory = `${path}.owner`,
    acquire = true
  ) {
    this.#path = path
    this.runtimeGeneration = runtimeGeneration
    this.#ownerDirectory = ownerDirectory
    if (acquire) acquireOwner(this.#ownerDirectory, this.runtimeGeneration)
  }

  static adopt(
    path: string,
    runtimeGeneration: string,
    ownerDirectory: string
  ): RuntimeDatabaseOwnershipLease {
    return new RuntimeDatabaseOwnershipLease(path, runtimeGeneration, ownerDirectory, false)
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
        this.#ownerDirectory,
        false
      )
    } catch (error) {
      releaseOwner(this.#ownerDirectory, this.runtimeGeneration)
      throw error
    }
  }

  release(): void {
    if (!this.#held) return
    this.#held = false
    releaseOwner(this.#ownerDirectory, this.runtimeGeneration)
  }

  #assertHeld(): void {
    if (!this.#held) throw new Error('database ownership has already been released')
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
