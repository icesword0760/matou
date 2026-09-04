export interface DisposableTerminalModel {
  dispose(): void
  suspend?(): void
}

/**
 * Owns xterm's VT model independently from the virtualized Session card DOM.
 * Mounted surfaces hold leases and are never evicted. Unmounted models form a
 * bounded LRU warm cache across Workspace, Task, and graph-level navigation so
 * recently visited cards can paint immediately without retaining their DOM or
 * GPU renderer indefinitely.
 */
export class ForegroundTerminalModelCache<T extends DisposableTerminalModel> {
  readonly #models = new Map<string, T>()
  readonly #leases = new Map<string, number>()
  readonly #lastUsed = new Map<string, number>()
  readonly #maximumModels: number
  #foregroundSessions = new Set<string>()
  #clock = 0

  constructor(maximumModels = 16) {
    if (!Number.isSafeInteger(maximumModels) || maximumModels < 1) {
      throw new RangeError('maximumModels must be a positive integer')
    }
    this.#maximumModels = maximumModels
  }

  get size(): number { return this.#models.size }

  has(sessionId: string): boolean { return this.#models.has(sessionId) }

  setForegroundSessions(sessionIds: readonly string[]): void {
    const next = new Set(sessionIds)
    this.#foregroundSessions = next
    for (const sessionId of next) {
      if (this.#models.has(sessionId)) this.#touch(sessionId)
    }
    this.#prune()
  }

  acquire(sessionId: string, create: () => T): T {
    const current = this.#models.get(sessionId)
    const model = current ?? create()
    if (!current) this.#models.set(sessionId, model)
    this.#leases.set(sessionId, (this.#leases.get(sessionId) ?? 0) + 1)
    this.#touch(sessionId)
    this.#prune()
    return model
  }

  /** Returns true when the model remains available as a warm cache entry. */
  release(sessionId: string): boolean {
    const remainingLeases = Math.max(0, (this.#leases.get(sessionId) ?? 0) - 1)
    if (remainingLeases > 0) {
      this.#leases.set(sessionId, remainingLeases)
      return false
    }
    this.#leases.delete(sessionId)
    const model = this.#models.get(sessionId)
    if (!model) return false
    this.#touch(sessionId)
    this.#prune()
    const retained = this.#models.has(sessionId)
    if (retained) model.suspend?.()
    return retained
  }

  clear(): void {
    for (const model of this.#models.values()) model.dispose()
    this.#models.clear()
    this.#leases.clear()
    this.#lastUsed.clear()
    this.#foregroundSessions.clear()
  }

  #touch(sessionId: string): void {
    this.#clock += 1
    this.#lastUsed.set(sessionId, this.#clock)
  }

  #prune(): void {
    while (this.#models.size > this.#maximumModels) {
      const evictable = [...this.#models.keys()].filter(
        (sessionId) => (this.#leases.get(sessionId) ?? 0) === 0
      )
      if (evictable.length === 0) return
      const background = evictable.filter((sessionId) => !this.#foregroundSessions.has(sessionId))
      const candidates = background.length > 0 ? background : evictable
      const oldest = candidates.reduce((left, right) =>
        (this.#lastUsed.get(left) ?? 0) <= (this.#lastUsed.get(right) ?? 0) ? left : right)
      const model = this.#models.get(oldest)
      this.#models.delete(oldest)
      this.#leases.delete(oldest)
      this.#lastUsed.delete(oldest)
      model?.dispose()
    }
  }
}

export const foregroundTerminalModels = new ForegroundTerminalModelCache<DisposableTerminalModel>()
