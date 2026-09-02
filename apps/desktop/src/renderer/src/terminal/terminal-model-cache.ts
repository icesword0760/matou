export interface DisposableTerminalModel {
  dispose(): void
}

/**
 * Owns xterm's VT model independently from the virtualized Session card DOM.
 * A model is retained only while its horizontal sibling level is foreground;
 * moving to another level, Scene, Task, or Workspace disposes it immediately.
 */
export class ForegroundTerminalModelCache<T extends DisposableTerminalModel> {
  readonly #models = new Map<string, T>()
  #foregroundSessions = new Set<string>()

  get size(): number { return this.#models.size }

  setForegroundSessions(sessionIds: readonly string[]): void {
    const next = new Set(sessionIds)
    for (const [sessionId, model] of this.#models) {
      if (next.has(sessionId)) continue
      this.#models.delete(sessionId)
      model.dispose()
    }
    this.#foregroundSessions = next
  }

  acquire(sessionId: string, create: () => T): T {
    const current = this.#models.get(sessionId)
    if (current) return current
    const model = create()
    this.#models.set(sessionId, model)
    return model
  }

  /** Returns true when the model remains owned by the foreground level. */
  release(sessionId: string): boolean {
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
