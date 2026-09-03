export interface ProviderReadyIdentity {
  sessionId: string
  runId: string
}

interface ProviderReadyWaiter {
  resolve(identity: ProviderReadyIdentity): void
  reject(error: Error): void
  timer: ReturnType<typeof setTimeout>
}

/**
 * One-shot readiness rendezvous for a provider process.
 *
 * Readiness is deliberately not cached: a caller registers before launching a
 * Session and only a later identity hook for that Session can release it. This
 * prevents an identity from an older run from making a new launch appear ready.
 */
export class ProviderReadyRegistry {
  readonly #waiters = new Map<string, Set<ProviderReadyWaiter>>()

  wait(sessionId: string, timeoutMs: number): Promise<ProviderReadyIdentity> {
    if (!sessionId) return Promise.reject(new Error('sessionId is required'))
    if (!Number.isFinite(timeoutMs) || timeoutMs < 0) {
      return Promise.reject(new Error('timeoutMs must be a non-negative finite number'))
    }

    return new Promise<ProviderReadyIdentity>((resolve, reject) => {
      const waiter: ProviderReadyWaiter = {
        resolve,
        reject,
        timer: setTimeout(() => {
          this.#delete(sessionId, waiter)
          reject(new Error(`等待会话 ${sessionId} 的 Provider 就绪超时`))
        }, timeoutMs)
      }
      waiter.timer.unref?.()
      const sessionWaiters = this.#waiters.get(sessionId) ?? new Set<ProviderReadyWaiter>()
      sessionWaiters.add(waiter)
      this.#waiters.set(sessionId, sessionWaiters)
    })
  }

  record(sessionId: string, runId: string): void {
    const sessionWaiters = this.#waiters.get(sessionId)
    if (!sessionWaiters) return
    this.#waiters.delete(sessionId)
    const identity = { sessionId, runId }
    for (const waiter of sessionWaiters) {
      clearTimeout(waiter.timer)
      waiter.resolve(identity)
    }
  }

  #delete(sessionId: string, waiter: ProviderReadyWaiter): void {
    const sessionWaiters = this.#waiters.get(sessionId)
    if (!sessionWaiters) return
    sessionWaiters.delete(waiter)
    if (sessionWaiters.size === 0) this.#waiters.delete(sessionId)
  }
}
