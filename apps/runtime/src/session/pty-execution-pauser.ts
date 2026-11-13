import type { DurabilityExecutionPauser } from './session-durability-gate'

export type PauseReason = 'durability' | 'egress'

export interface PausablePtyExecution {
  readonly pid: number
  pause(): void
  resume(): void
}

export interface PtyExecutionPauserOptions {
  platform?: NodeJS.Platform
  signalProcessGroup?: (pid: number, signal: 'SIGSTOP' | 'SIGCONT') => void
}

/**
 * Applies both sides of PTY backpressure. Pausing the readable stream first
 * bounds user-space output immediately; stopping the process group then keeps
 * providers and their descendants from filling the kernel PTY buffer.
 */
export class PtyExecutionPauser {
  readonly #terminal: PausablePtyExecution
  readonly #platform: NodeJS.Platform
  readonly #signalProcessGroup: (pid: number, signal: 'SIGSTOP' | 'SIGCONT') => void
  #paused = false

  constructor(terminal: PausablePtyExecution, options: PtyExecutionPauserOptions = {}) {
    this.#terminal = terminal
    this.#platform = options.platform ?? process.platform
    this.#signalProcessGroup = options.signalProcessGroup ?? ((pid, signal) => {
      process.kill(-pid, signal)
    })
  }

  get isPaused(): boolean { return this.#paused }

  async pause(): Promise<void> {
    if (this.#paused) return
    this.#terminal.pause()
    this.#paused = true
    if (this.#platform === 'win32') return
    try {
      this.#signalProcessGroup(this.#terminal.pid, 'SIGSTOP')
    } catch (error) {
      if (!isMissingProcess(error)) throw error
    }
  }

  async resume(): Promise<void> {
    if (!this.#paused) return
    if (this.#platform !== 'win32') {
      try {
        this.#signalProcessGroup(this.#terminal.pid, 'SIGCONT')
      } catch (error) {
        if (!isMissingProcess(error)) throw error
      }
    }
    this.#terminal.resume()
    this.#paused = false
  }
}

function isMissingProcess(error: unknown): boolean {
  return (error as NodeJS.ErrnoException | undefined)?.code === 'ESRCH'
}

/**
 * Coordinates multiple independent pause reasons over a single underlying
 * execution pauser. The inner pauser is paused on the transition from zero to
 * one active reason and resumed only when the last active reason clears.
 */
export class SharedExecutionPauser {
  readonly #inner: DurabilityExecutionPauser
  readonly #reasons = new Set<PauseReason>()
  readonly #leases = new Map<PauseReason, DurabilityExecutionPauser>()
  #inFlight: Promise<void> | undefined

  constructor(inner: DurabilityExecutionPauser) {
    this.#inner = inner
  }

  lease(reason: PauseReason): DurabilityExecutionPauser {
    let existing = this.#leases.get(reason)
    if (!existing) {
      existing = {
        pause: () => this.#pause(reason),
        resume: () => this.#resume(reason)
      }
      this.#leases.set(reason, existing)
    }
    return existing
  }

  isPausedFor(reason: PauseReason): boolean {
    return this.#reasons.has(reason)
  }

  #pause(reason: PauseReason): Promise<void> {
    return this.#enqueue(() => {
      if (this.#reasons.has(reason)) return
      const shouldPause = this.#reasons.size === 0
      this.#reasons.add(reason)
      if (!shouldPause) return
      return this.#inner.pause()
    })
  }

  #resume(reason: PauseReason): Promise<void> {
    return this.#enqueue(() => {
      if (!this.#reasons.has(reason)) return
      if (this.#reasons.size > 1) {
        this.#reasons.delete(reason)
        return
      }
      const result = this.#inner.resume()
      if (isPromiseLike(result)) {
        return Promise.resolve(result).then(() => {
          this.#reasons.delete(reason)
        })
      }
      this.#reasons.delete(reason)
      return undefined
    })
  }

  #enqueue(run: () => void | Promise<void>): Promise<void> {
    if (this.#inFlight === undefined) {
      let result: void | Promise<void>
      try {
        result = run()
      } catch (error) {
        return Promise.reject(error)
      }
      if (!isPromiseLike(result)) {
        return Promise.resolve()
      }
      const promise = Promise.resolve(result)
      const tracked = promise.then(() => undefined, () => undefined).finally(() => {
        if (this.#inFlight === tracked) this.#inFlight = undefined
      })
      this.#inFlight = tracked
      return promise
    }
    const next = this.#inFlight.then(() => run())
    const tracked = next.then(() => undefined, () => undefined).finally(() => {
      if (this.#inFlight === tracked) this.#inFlight = undefined
    })
    this.#inFlight = tracked
    return next
  }
}

function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    typeof (value as { then?: unknown }).then === 'function'
  )
}
