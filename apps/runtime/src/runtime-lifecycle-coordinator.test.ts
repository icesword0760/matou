import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from './storage/database'
import { DatabaseLifecycleService } from './storage/database-lifecycle-service'
import {
  RuntimeLifecycleCoordinator,
  RuntimeShutdownRequestedError
} from './runtime-lifecycle-coordinator'

const opened: RuntimeDatabase[] = []

afterEach(() => {
  for (const database of opened.splice(0)) database.close()
})

describe('RuntimeLifecycleCoordinator', () => {
  it.each(['sessions', 'provider', 'host'] as const)(
    'still backs up, rotates, and closes exactly once when %s shutdown fails',
    async (failurePoint) => {
      const database = await createDatabase()
      const events: string[] = []
      const failure = new Error(`${failurePoint} stop failed`)
      const backups = {
        async create() {
          events.push('backup')
          return {
            id: 'clean', path: '/tmp/clean', createdAt: 1, reason: 'clean-exit' as const,
            schemaVersion: 0, size: 1, sha256: 'a'.repeat(64)
          }
        },
        async rotate() { events.push('rotate') }
      }
      const coordinator = new RuntimeLifecycleCoordinator()
      coordinator.registerDatabaseLifecycle(
        database,
        new DatabaseLifecycleService(database, backups)
      )
      coordinator.registerProviderHooks({
        async stop() {
          events.push('provider')
          if (failurePoint === 'provider') throw failure
        }
      })
      coordinator.registerHostControl({
        async stop() {
          events.push('host')
          if (failurePoint === 'host') throw failure
        }
      })
      const initialized = coordinator.startInitialization(async () => undefined)

      const shutdown = coordinator.shutdown(initialized, {
        closeIncoming: () => events.push('incoming'),
        shutdownSessions: async () => {
          events.push('sessions')
          if (failurePoint === 'sessions') throw failure
        }
      })

      await expect(shutdown).rejects.toBe(failure)
      expect(events).toEqual(['incoming', 'sessions', 'provider', 'host', 'backup', 'rotate'])
      expect(() => database.exec('SELECT 1')).toThrow('database is closed')
      await expect(coordinator.shutdown(initialized, {
        closeIncoming: () => events.push('incoming-again'),
        shutdownSessions: async () => { events.push('sessions-again') }
      })).rejects.toBe(failure)
      expect(events).toEqual(['incoming', 'sessions', 'provider', 'host', 'backup', 'rotate'])
    }
  )

  it.each(['database-opened', 'backup-running', 'migration-running'] as const)(
    'waits for the %s initialization safe point, skips service startup, and closes once',
    async (shutdownPoint) => {
      const database = await createDatabase()
      const events: string[] = []
      const databaseOpened = deferred<void>()
      const beginBackup = deferred<void>()
      const backupRunning = deferred<void>()
      const finishBackup = deferred<void>()
      const migrationRunning = deferred<void>()
      const finishMigration = deferred<void>()
      const coordinator = new RuntimeLifecycleCoordinator()
      const lifecycle = new DatabaseLifecycleService(database, {
        async create() {
          events.push('clean-backup')
          return {
            id: 'clean', path: '/tmp/clean', createdAt: 1, reason: 'clean-exit' as const,
            schemaVersion: 0, size: 1, sha256: 'a'.repeat(64)
          }
        },
        async rotate() { events.push('clean-rotate') }
      })
      const initialized = coordinator.startInitialization(async () => {
        coordinator.registerDatabaseLifecycle(database, lifecycle)
        events.push('database-opened')
        databaseOpened.resolve()
        await beginBackup.promise
        coordinator.assertStartupActive()
        events.push('backup-running')
        backupRunning.resolve()
        await finishBackup.promise
        coordinator.assertStartupActive()
        events.push('migration-running')
        migrationRunning.resolve()
        await finishMigration.promise
        coordinator.assertStartupActive()
        events.push('services-started')
      })
      await databaseOpened.promise
      if (shutdownPoint !== 'database-opened') {
        beginBackup.resolve()
        await backupRunning.promise
      }
      if (shutdownPoint === 'migration-running') {
        finishBackup.resolve()
        await migrationRunning.promise
      }

      const shutdown = coordinator.shutdown(initialized, {
        closeIncoming: () => events.push('incoming'),
        shutdownSessions: async () => { events.push('sessions') }
      })
      await Promise.resolve()
      expect(events).not.toContain('clean-backup')

      beginBackup.resolve()
      finishBackup.resolve()
      finishMigration.resolve()

      await expect(initialized).rejects.toBeInstanceOf(RuntimeShutdownRequestedError)
      await shutdown
      expect(events).not.toContain('services-started')
      expect(events.filter((event) => event === 'clean-backup')).toHaveLength(1)
      expect(events.filter((event) => event === 'clean-rotate')).toHaveLength(1)
      expect(() => database.exec('SELECT 1')).toThrow('database is closed')
    }
  )
})

async function createDatabase(): Promise<RuntimeDatabase> {
  const root = await mkdtemp(join(tmpdir(), 'matou-runtime-lifecycle-'))
  const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  opened.push(database)
  return database
}

function deferred<T>(): { promise: Promise<T>; resolve(value?: T): void } {
  let resolve!: (value: T | PromiseLike<T>) => void
  const promise = new Promise<T>((fulfill) => { resolve = fulfill })
  return { promise, resolve: (value?: T) => resolve(value as T) }
}
