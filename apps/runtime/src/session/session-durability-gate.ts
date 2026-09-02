export const DEFAULT_MAX_RETAINED_BYTES = 4 * 1024 * 1024

export type SessionDurabilityState = 'healthy' | 'pausing' | 'paused' | 'recovering' | 'ended'

export interface PendingDurableFrame {
  sequence: number
  kind: 'output' | 'resize' | 'exit'
  bytes: Uint8Array
  persist: () => Promise<void>
  /** Runs only after persist succeeds. It is never retried after it has run. */
  afterPersist?: () => void
}

export interface SessionDurabilityFaultEvent {
  sessionId: string
  failedSequence: number
  retainedBytes: number
  error: unknown
}

export interface SessionDurabilityRecoveredEvent {
  sessionId: string
  throughSequence: number
}

export interface DurabilityExecutionPauser {
  pause(): void | Promise<void>
  resume(): void | Promise<void>
}

export interface SessionDurabilityGateOptions {
  sessionId: string
  initialSequence?: number
  maxRetainedBytes?: number
  pauser: DurabilityExecutionPauser
  onFault?: (event: SessionDurabilityFaultEvent) => void
  onRecovered?: (event: SessionDurabilityRecoveredEvent) => void
  onSideEffectError?: (error: unknown, frame: PendingDurableFrame) => void
}

export class DurabilityBufferOverflowError extends Error {
  readonly maxRetainedBytes: number

  constructor(maxRetainedBytes: number) {
    super(`session durability buffer exceeds ${maxRetainedBytes} bytes`)
    this.name = 'DurabilityBufferOverflowError'
    this.maxRetainedBytes = maxRetainedBytes
  }
}

type QueuedFrame = { frame: PendingDurableFrame; retainedSize: number }

/** A single-session, bounded write-ahead gate for PTY journal frames. */
export class SessionDurabilityGate {
  readonly #sessionId: string
  readonly #maxRetainedBytes: number
  readonly #pauser: DurabilityExecutionPauser
  readonly #onFault: SessionDurabilityGateOptions['onFault']
  readonly #onRecovered: SessionDurabilityGateOptions['onRecovered']
  readonly #onSideEffectError: SessionDurabilityGateOptions['onSideEffectError']
  readonly #queue: QueuedFrame[] = []
  #state: SessionDurabilityState = 'healthy'
  #retainedBytes = 0
  #lastAcceptedSequence: number
  #lastPersistedSequence: number
  #operation: Promise<void> = Promise.resolve()
  #retryOperation: Promise<void> | undefined

  constructor(options: SessionDurabilityGateOptions) {
    this.#sessionId = options.sessionId
    this.#maxRetainedBytes = options.maxRetainedBytes ?? DEFAULT_MAX_RETAINED_BYTES
    if (!Number.isSafeInteger(this.#maxRetainedBytes) || this.#maxRetainedBytes <= 0) {
      throw new Error('maxRetainedBytes must be a positive safe integer')
    }
    this.#pauser = options.pauser
    this.#onFault = options.onFault
    this.#onRecovered = options.onRecovered
    this.#onSideEffectError = options.onSideEffectError
    this.#lastAcceptedSequence = options.initialSequence ?? 0
    this.#lastPersistedSequence = options.initialSequence ?? 0
  }

  get state(): SessionDurabilityState { return this.#state }
  get retainedBytes(): number { return this.#retainedBytes }
  get lastAcceptedSequence(): number { return this.#lastAcceptedSequence }
  get lastPersistedSequence(): number { return this.#lastPersistedSequence }

  /**
   * Admission is synchronous so callers only consume a sequence when the
   * bounded FIFO accepted the frame. The returned promise settles once the
   * frame is durable, or once a storage fault has retained it for retry.
   */
  append(frame: PendingDurableFrame): Promise<void> {
    if (this.#state === 'ended') throw new Error('session durability gate is ended')
    if (!Number.isSafeInteger(frame.sequence) || frame.sequence !== this.#lastAcceptedSequence + 1) {
      throw new Error('durability frame sequence must be contiguous')
    }
    const retainedSize = Math.max(1, frame.bytes.byteLength)
    if (this.#retainedBytes + retainedSize > this.#maxRetainedBytes) {
      throw new DurabilityBufferOverflowError(this.#maxRetainedBytes)
    }
    this.#queue.push({ frame, retainedSize })
    this.#retainedBytes += retainedSize
    this.#lastAcceptedSequence = frame.sequence

    if (this.#state !== 'healthy') return Promise.resolve()
    const operation = this.#operation.then(() => this.#drainHealthy())
    this.#operation = operation.catch(() => undefined)
    return operation
  }

  retry(): Promise<void> {
    if (this.#state === 'ended') return Promise.reject(new Error('session durability gate is ended'))
    if (this.#state === 'healthy') return this.#operation
    if (this.#retryOperation !== undefined) return this.#retryOperation
    const operation = this.#operation.then(async () => {
      if (this.#state === 'ended' || this.#state === 'healthy') return
      this.#state = 'recovering'
      try {
        await this.#drainRecovering()
      } catch (error) {
        this.#state = 'paused'
        throw error
      }
      try {
        await this.#pauser.resume()
      } catch (error) {
        this.#state = 'paused'
        throw error
      }
      this.#state = 'healthy'
      this.#emitRecovered()
      if (this.#queue.length > 0) await this.#drainHealthy()
    })
    this.#operation = operation.catch(() => undefined)
    this.#retryOperation = operation.finally(() => { this.#retryOperation = undefined })
    return this.#retryOperation
  }

  /** Discards the in-memory tail and releases a stopped process for termination. */
  async end(): Promise<void> {
    if (this.#state === 'ended') return
    await this.#operation
    const needsResume = this.#state === 'paused' || this.#state === 'pausing' || this.#state === 'recovering'
    this.#queue.length = 0
    this.#retainedBytes = 0
    this.#state = 'ended'
    if (needsResume) await this.#pauser.resume()
  }

  async #drainHealthy(): Promise<void> {
    while (this.#state === 'healthy' && this.#queue.length > 0) {
      const queued = this.#queue[0]!
      try {
        await queued.frame.persist()
      } catch (error) {
        this.#state = 'pausing'
        try {
          await this.#pauser.pause()
        } catch {
          // Read pausing is attempted before the process-group signal. Even if
          // the latter races process exit, retain the frame and keep recovery live.
        } finally {
          this.#state = 'paused'
          this.#emitFault(queued.frame.sequence, error)
        }
        return
      }
      this.#commitHead(queued)
    }
  }

  async #drainRecovering(): Promise<void> {
    while (this.#queue.length > 0) {
      const queued = this.#queue[0]!
      await queued.frame.persist()
      this.#commitHead(queued)
    }
  }

  #commitHead(queued: QueuedFrame): void {
    this.#queue.shift()
    this.#retainedBytes -= queued.retainedSize
    this.#lastPersistedSequence = queued.frame.sequence
    if (queued.frame.afterPersist === undefined) return
    try {
      queued.frame.afterPersist()
    } catch (error) {
      this.#onSideEffectError?.(error, queued.frame)
    }
  }

  #emitFault(failedSequence: number, error: unknown): void {
    try {
      this.#onFault?.({
        sessionId: this.#sessionId,
        failedSequence,
        retainedBytes: this.#retainedBytes,
        error
      })
    } catch {
      // Observers must not poison the journal recovery path.
    }
  }

  #emitRecovered(): void {
    try {
      this.#onRecovered?.({
        sessionId: this.#sessionId,
        throughSequence: this.#lastPersistedSequence
      })
    } catch {
      // Observers must not poison the journal recovery path.
    }
  }
}
