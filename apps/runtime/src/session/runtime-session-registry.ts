import type { PtySession } from './pty-session'

/** Runtime-generation owner for live PTYs; Renderer connections only attach views. */
export class RuntimeSessionRegistry {
  readonly #sessions = new Map<string, PtySession>()
  readonly #operations = new Map<string, Promise<void>>()
  readonly #pendingProviderRuns = new Map<string, string>()

  get(sessionId: string): PtySession | undefined { return this.#sessions.get(sessionId) }
  has(sessionId: string): boolean { return this.#sessions.has(sessionId) }
  get size(): number { return this.#sessions.size }
  pids(): number[] { return [...this.#sessions.values()].map(({ pid }) => pid) }
  sessionPids(): Array<{ sessionId: string; pid: number }> {
    return [...this.#sessions.values()].map(({ sessionId, pid }) => ({ sessionId, pid }))
  }
  maxUnackedBytes(): number {
    return Math.max(0, ...[...this.#sessions.values()].map(({ maximumUnackedBytes }) => maximumUnackedBytes))
  }
  retainedDurabilityBytes(): number {
    return [...this.#sessions.values()].reduce(
      (total, session) => total + session.retainedDurabilityBytes,
      0
    )
  }
  set(session: PtySession): void { this.#sessions.set(session.sessionId, session) }
  delete(sessionId: string, expected?: PtySession): boolean {
    if (expected && this.#sessions.get(sessionId) !== expected) return false
    return this.#sessions.delete(sessionId)
  }
  values(): IterableIterator<PtySession> { return this.#sessions.values() }
  markProviderIdentityPending(sessionId: string, runId: string): void {
    this.#pendingProviderRuns.set(sessionId, runId)
  }
  providerIdentityPending(sessionId: string): boolean {
    return this.#pendingProviderRuns.has(sessionId)
  }
  clearProviderIdentityPending(sessionId: string, expectedRunId?: string): boolean {
    if (
      expectedRunId !== undefined &&
      this.#pendingProviderRuns.get(sessionId) !== expectedRunId
    ) return false
    return this.#pendingProviderRuns.delete(sessionId)
  }
  async runExclusive<T>(sessionId: string, operation: () => Promise<T>): Promise<T> {
    const previous = this.#operations.get(sessionId) ?? Promise.resolve()
    const result = previous.catch(() => undefined).then(operation)
    const completion = result.then(() => undefined, () => undefined)
    this.#operations.set(sessionId, completion)
    try {
      return await result
    } finally {
      if (this.#operations.get(sessionId) === completion) {
        this.#operations.delete(sessionId)
      }
    }
  }
  disposeAll(): void {
    for (const session of this.#sessions.values()) session.dispose()
    this.#sessions.clear()
    this.#pendingProviderRuns.clear()
  }
  async shutdownAll(): Promise<void> {
    const sessions = [...this.#sessions.values()]
    await Promise.all(sessions.map((session) => session.shutdownForRuntime()))
    for (const session of sessions) this.delete(session.sessionId, session)
    this.#pendingProviderRuns.clear()
  }
}
