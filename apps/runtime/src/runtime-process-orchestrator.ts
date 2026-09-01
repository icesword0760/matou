interface RuntimeProcessOrchestratorOptions {
  runtimeReady: Promise<unknown>
  shutdown(): Promise<void>
  reportError(label: string, error: unknown): void
  exit(code: number): void
}

export class RuntimeProcessOrchestrator {
  readonly #runtimeReady: Promise<unknown>
  readonly #shutdown: () => Promise<void>
  readonly #reportError: (label: string, error: unknown) => void
  readonly #exit: (code: number) => void
  #termination: Promise<void> | undefined
  #watching: Promise<void> | undefined

  constructor(options: RuntimeProcessOrchestratorOptions) {
    this.#runtimeReady = options.runtimeReady
    this.#shutdown = options.shutdown
    this.#reportError = options.reportError
    this.#exit = options.exit
  }

  watchInitialization(): Promise<void> {
    this.#watching ??= this.#runtimeReady.then(
      () => undefined,
      (error: unknown) => this.#terminateForInitializationFailure(error)
    )
    return this.#watching
  }

  terminateFromSignal(): Promise<void> {
    if (this.#termination) return this.#termination
    this.#termination = this.#terminateFromSignal()
    return this.#termination
  }

  #terminateForInitializationFailure(initializationError: unknown): Promise<void> {
    if (this.#termination) return this.#termination
    this.#termination = this.#terminateAfterInitializationFailure(initializationError)
    return this.#termination
  }

  async #terminateAfterInitializationFailure(initializationError: unknown): Promise<void> {
    let finalError = initializationError
    try {
      await this.#shutdown()
    } catch (cleanupError) {
      finalError = combineErrors(initializationError, cleanupError)
    }
    this.#finishWithError('[runtime.initialization]', finalError)
  }

  async #terminateFromSignal(): Promise<void> {
    try {
      await this.#shutdown()
      this.#exit(0)
    } catch (error) {
      this.#finishWithError('[runtime.shutdown]', error)
    }
  }

  #finishWithError(label: string, error: unknown): void {
    try {
      this.#reportError(label, error)
    } finally {
      this.#exit(1)
    }
  }
}

function combineErrors(initializationError: unknown, cleanupError: unknown): unknown {
  if (cleanupError === initializationError || cleanupError instanceof AggregateError) {
    return cleanupError
  }
  return new AggregateError(
    [initializationError, cleanupError],
    'Runtime initialization and cleanup failed'
  )
}
