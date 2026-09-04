export interface DisposableTerminalModel {
  dispose(): void
}

/**
 * Owns xterm's VT model independently from the virtualized Session card DOM.
 * A model is retained while its horizontal sibling level is foreground. A
 * mounted surface also holds a lease so a transient projection gap cannot
 * dispose xterm out from under the live card.
 */
export class ForegroundTerminalModelCache<T extends DisposableTerminalModel> {
  readonly #models = new Map<string, T>()
  readonly #leases = new Map<string, number>()
  #foregroundSessions = new Set<string>()

  get size(): number { return this.#models.size }

  setForegroundSessions(sessionIds: readonly string[]): void {
    const next = new Set(sessionIds)
    for (const [sessionId, model] of this.#models) {
      if (next.has(sessionId)) continue
      if ((this.#leases.get(sessionId) ?? 0) > 0) continue
      this.#models.delete(sessionId)
      this.#leases.delete(sessionId)
      model.dispose()
    }
    this.#foregroundSessions = next
  }

  acquire(sessionId: string, create: () => T): T {
    const current = this.#models.get(sessionId)
    const model = current ?? create()
    if (!current) this.#models.set(sessionId, model)
    this.#leases.set(sessionId, (this.#leases.get(sessionId) ?? 0) + 1)
    return model
  }

  /** Returns true when the model remains owned by the foreground level. */
  release(sessionId: string): boolean {
    const remainingLeases = Math.max(0, (this.#leases.get(sessionId) ?? 0) - 1)
    if (remainingLeases > 0) {
      this.#leases.set(sessionId, remainingLeases)
      return false
    }
    this.#leases.delete(sessionId)
    if (this.#foregroundSessions.has(sessionId)) return true
    const model = this.#models.get(sessionId)
    if (model) {
      this.#models.delete(sessionId)
      model.dispose()
    }
    return false
  }
}

export const foregroundTerminalModels = new ForegroundTerminalModelCache<DisposableTerminalModel>()
