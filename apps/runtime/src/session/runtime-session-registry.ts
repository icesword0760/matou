import type { PtySession } from './pty-session'

/** Runtime-generation owner for live PTYs; Renderer connections only attach views. */
export class RuntimeSessionRegistry {
  readonly #sessions = new Map<string, PtySession>()
  readonly #operations = new Map<string, Promise<void>>()

  get(sessionId: string): PtySession | undefined { return this.#sessions.get(sessionId) }
  has(sessionId: string): boolean { return this.#sessions.has(sessionId) }
  get size(): number { return this.#sessions.size }
  pids(): number[] { return [...this.#sessions.values()].map(({ pid }) => pid) }
  set(session: PtySession): void { this.#sessions.set(session.sessionId, session) }
  delete(sessionId: string, expected?: PtySession): boolean {
    if (expected && this.#sessions.get(sessionId) !== expected) return false
    return this.#sessions.delete(sessionId)
  }
  values(): IterableIterator<PtySession> { return this.#sessions.values() }
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
  }
  async shutdownAll(): Promise<void> {
    const sessions = [...this.#sessions.values()]
    await Promise.all(sessions.map((session) => session.shutdownForRuntime()))
    for (const session of sessions) this.delete(session.sessionId, session)
  }
}
