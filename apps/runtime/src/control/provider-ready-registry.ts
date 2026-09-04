export interface ProviderReadyIdentity {
  sessionId: string
  runId: string
}

interface ProviderReadyWaiter {
  resolve(identity: ProviderReadyIdentity): void
  reject(error: Error): void
  timer: ReturnType<typeof setTimeout>
  signal?: AbortSignal
  onAbort?: () => void
}

/**
 * One-shot readiness rendezvous for provider processes.
 *
 * A waiter is registered before startup. Runtime supplies the currently active
 * Run when recording a hook, so delayed hooks from an older Run cannot release
 * a waiter for the new process.
 */
export class ProviderReadyRegistry {
  readonly #waiters = new Map<string, Set<ProviderReadyWaiter>>()

  get pendingWaiterCount(): number {
    let count = 0
    for (const waiters of this.#waiters.values()) count += waiters.size
    return count
  }

  wait(
    sessionId: string,
    timeoutMs: number,
    signal?: AbortSignal
  ): Promise<ProviderReadyIdentity> {
    if (!sessionId) return Promise.reject(new Error('sessionId is required'))
    if (!Number.isFinite(timeoutMs) || timeoutMs < 0) {
      return Promise.reject(new Error('timeoutMs must be a non-negative finite number'))
    }
    if (signal?.aborted) return Promise.reject(abortError(signal))

    return new Promise<ProviderReadyIdentity>((resolve, reject) => {
      const waiter: ProviderReadyWaiter = {
        resolve,
        reject,
        timer: setTimeout(() => {
          this.#remove(sessionId, waiter)
          reject(new Error(`等待会话 ${sessionId} 的 Provider 就绪超时`))
        }, timeoutMs),
        ...(signal === undefined ? {} : { signal })
      }
      waiter.timer.unref?.()
      if (signal) {
        waiter.onAbort = () => {
          this.#remove(sessionId, waiter)
          reject(abortError(signal))
        }
        signal.addEventListener('abort', waiter.onAbort, { once: true })
      }
      const sessionWaiters = this.#waiters.get(sessionId) ?? new Set<ProviderReadyWaiter>()
      sessionWaiters.add(waiter)
      this.#waiters.set(sessionId, sessionWaiters)
    })
  }

  record(sessionId: string, runId: string, authoritativeRunId = runId): boolean {
    if (runId !== authoritativeRunId) return false
    const sessionWaiters = this.#waiters.get(sessionId)
    if (!sessionWaiters) return false
    this.#waiters.delete(sessionId)
    const identity = { sessionId, runId }
    for (const waiter of sessionWaiters) {
      this.#cleanup(waiter)
      waiter.resolve(identity)
    }
    return true
  }

  cancel(sessionId: string, reason = new Error(`等待会话 ${sessionId} 就绪已取消`)): void {
    const sessionWaiters = this.#waiters.get(sessionId)
    if (!sessionWaiters) return
    this.#waiters.delete(sessionId)
    for (const waiter of sessionWaiters) {
      this.#cleanup(waiter)
      waiter.reject(reason)
    }
  }

  cancelAll(reason = new Error('Provider 就绪等待已取消')): void {
    for (const sessionId of [...this.#waiters.keys()]) this.cancel(sessionId, reason)
  }

  #remove(sessionId: string, waiter: ProviderReadyWaiter): void {
    const sessionWaiters = this.#waiters.get(sessionId)
    if (!sessionWaiters) return
    sessionWaiters.delete(waiter)
    this.#cleanup(waiter)
    if (sessionWaiters.size === 0) this.#waiters.delete(sessionId)
  }

  #cleanup(waiter: ProviderReadyWaiter): void {
    clearTimeout(waiter.timer)
    if (waiter.signal && waiter.onAbort) {
      waiter.signal.removeEventListener('abort', waiter.onAbort)
    }
  }
}

function abortError(signal: AbortSignal): Error {
  return signal.reason instanceof Error ? signal.reason : new Error('Provider 就绪等待已取消')
}
