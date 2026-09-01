import type { RuntimeDatabase } from './database'
import type { DatabaseBackupDescriptor, DatabaseBackupService } from './database-backup-service'

interface CleanExitBackupService {
  create(
    database: RuntimeDatabase,
    reason: 'clean-exit'
  ): Promise<DatabaseBackupDescriptor>
  rotate(maxCount?: number): Promise<void>
}

interface DatabaseLifecycleOptions {
  timeoutMs?: number
}

export class DatabaseLifecycleService {
  readonly #database: RuntimeDatabase
  readonly #backups: CleanExitBackupService
  readonly #timeoutMs: number
  #closing: Promise<void> | undefined

  constructor(
    database: RuntimeDatabase,
    backups: DatabaseBackupService | CleanExitBackupService,
    options: DatabaseLifecycleOptions = {}
  ) {
    this.#database = database
    this.#backups = backups
    this.#timeoutMs = options.timeoutMs ?? 5_000
  }

  closeCleanly(): Promise<void> {
    this.#closing ??= this.#closeCleanly()
    return this.#closing
  }

  async #closeCleanly(): Promise<void> {
    let failure: unknown
    try {
      await withTimeout((async () => {
        await this.#backups.create(this.#database, 'clean-exit')
        await this.#backups.rotate()
      })(), this.#timeoutMs)
    } catch (error) {
      failure = error
    } finally {
      this.#database.close()
    }
    if (failure !== undefined) throw failure
  }
}

function withTimeout<T>(operation: Promise<T>, timeoutMs: number): Promise<T> {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    return Promise.reject(new Error('clean shutdown timeout must be positive'))
  }
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new Error(`clean shutdown backup timed out after ${timeoutMs}ms`))
    }, timeoutMs)
    timer.unref?.()
    operation.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      (error: unknown) => {
        clearTimeout(timer)
        reject(error)
      }
    )
  })
}
