import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { RuntimeDatabase } from './database'
import { DatabaseLifecycleService } from './database-lifecycle-service'

const opened: RuntimeDatabase[] = []

afterEach(() => {
  for (const database of opened.splice(0)) database.close()
  vi.useRealTimers()
})

describe('DatabaseLifecycleService', () => {
  it('waits for the clean-exit backup and rotation before closing the database', async () => {
    const database = await createDatabase()
    const createGate = deferred<void>()
    const rotateGate = deferred<void>()
    const events: string[] = []
    const backupService = {
      async create(_database: RuntimeDatabase, reason: 'clean-exit') {
        events.push(`create:${reason}`)
        await createGate.promise
        return {
          id: 'backup', path: '/tmp/backup', createdAt: 1, reason,
          schemaVersion: 0, size: 1, sha256: 'a'.repeat(64)
        }
      },
      async rotate() {
        events.push('rotate')
        await rotateGate.promise
      }
    }
    const closing = new DatabaseLifecycleService(database, backupService).closeCleanly()

    expect(() => database.exec('SELECT 1')).not.toThrow()
    createGate.resolve()
    await vi.waitFor(() => expect(events).toEqual(['create:clean-exit', 'rotate']))
    expect(() => database.exec('SELECT 1')).not.toThrow()
    rotateGate.resolve()
    await closing

    expect(() => database.exec('SELECT 1')).toThrow('database is closed')
  })

  it('closes the database within the deadline and preserves a backup failure', async () => {
    vi.useFakeTimers()
    const database = await createDatabase()
    const failure = new Error('backup storage unavailable')
    const backupService = {
      create: async () => { throw failure },
      rotate: async () => undefined
    }
    const closing = new DatabaseLifecycleService(database, backupService, {
      timeoutMs: 25
    }).closeCleanly()

    await expect(closing).rejects.toBe(failure)
    expect(() => database.exec('SELECT 1')).toThrow('database is closed')
  })

  it('bounds a stalled backup and still closes the database', async () => {
    vi.useFakeTimers()
    const database = await createDatabase()
    const backupService = {
      create: async () => new Promise<never>(() => undefined),
      rotate: async () => undefined
    }
    const closing = new DatabaseLifecycleService(database, backupService, {
      timeoutMs: 25
    }).closeCleanly()
    const result = expect(closing).rejects.toThrow('clean shutdown backup timed out after 25ms')

    await vi.advanceTimersByTimeAsync(25)

    await result
    expect(() => database.exec('SELECT 1')).toThrow('database is closed')
  })
})

async function createDatabase(): Promise<RuntimeDatabase> {
  const root = await mkdtemp(join(tmpdir(), 'matou-lifecycle-'))
  const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  opened.push(database)
  return database
}

function deferred<T>(): { promise: Promise<T>; resolve(value?: T): void } {
  let resolve!: (value: T | PromiseLike<T>) => void
  const promise = new Promise<T>((fulfill) => { resolve = fulfill })
  return { promise, resolve: (value?: T) => resolve(value as T) }
}
