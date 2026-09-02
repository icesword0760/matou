import type { RuntimeDatabase } from './storage/database'
import type { DatabaseLifecycleService } from './storage/database-lifecycle-service'

interface StoppableResource {
  stop(): Promise<void>
}

interface RuntimeShutdownOperations {
  closeIncoming(): void
  shutdownSessions(): Promise<void>
}

interface OwnedDatabaseLifecycle {
  database: RuntimeDatabase
  lifecycle: DatabaseLifecycleService
}

export class RuntimeShutdownRequestedError extends Error {
  constructor() {
    super('Runtime shutdown was requested during initialization')
    this.name = 'RuntimeShutdownRequestedError'
  }
}

export class RuntimeLifecycleCoordinator {
  #shutdownRequested = false
  #shutdownPromise: Promise<void> | undefined
  #initialization: Promise<unknown> | undefined
  #database: OwnedDatabaseLifecycle | undefined
  #providerHooks: StoppableResource | undefined
  #hostControl: StoppableResource | undefined

  get shutdownRequested(): boolean {
    return this.#shutdownRequested
  }

  startInitialization<T>(initialize: () => Promise<T>): Promise<T> {
    if (this.#initialization) throw new Error('Runtime initialization is already registered')
    let initialization: Promise<T>
    try {
      initialization = Promise.resolve(initialize())
    } catch (error) {
      initialization = Promise.reject(error)
    }
    this.#initialization = initialization
    return initialization
  }

  registerDatabaseLifecycle(
    database: RuntimeDatabase,
    lifecycle: DatabaseLifecycleService
  ): void {
    if (this.#database && this.#database.database !== database) {
      throw new Error('a different Runtime database lifecycle is already registered')
    }
    this.#database = { database, lifecycle }
  }

  releaseDatabaseLifecycle(database: RuntimeDatabase): void {
    if (this.#database?.database === database) this.#database = undefined
  }

  registerProviderHooks(providerHooks: StoppableResource): void {
    this.#providerHooks = providerHooks
  }

  registerHostControl(hostControl: StoppableResource): void {
    this.#hostControl = hostControl
  }

  assertStartupActive(): void {
    if (this.#shutdownRequested) throw new RuntimeShutdownRequestedError()
  }

  shutdown(
    initialization: Promise<unknown> = this.#initialization ?? Promise.resolve(),
    operations: RuntimeShutdownOperations
  ): Promise<void> {
    if (this.#shutdownPromise) return this.#shutdownPromise
    this.#shutdownRequested = true
    this.#shutdownPromise = this.#shutdown(initialization, operations)
    return this.#shutdownPromise
  }

  async #shutdown(
    initialization: Promise<unknown>,
    operations: RuntimeShutdownOperations
  ): Promise<void> {
    const errors: unknown[] = []
    await capture(() => operations.closeIncoming(), errors)
    await capture(() => operations.shutdownSessions(), errors)
    await capture(async () => {
      try {
        await initialization
      } catch (error) {
        if (!(error instanceof RuntimeShutdownRequestedError)) throw error
      }
    }, errors)
    await capture(() => this.#providerHooks?.stop(), errors)
    await capture(() => this.#hostControl?.stop(), errors)
    await capture(() => this.#database?.lifecycle.closeCleanly(), errors)

    if (errors.length === 1) throw errors[0]
    if (errors.length > 1) throw new AggregateError(errors, 'Runtime shutdown failed')
  }
}

async function capture(operation: () => void | Promise<void> | undefined, errors: unknown[]): Promise<void> {
  try {
    await operation()
  } catch (error) {
    errors.push(error)
  }
}
