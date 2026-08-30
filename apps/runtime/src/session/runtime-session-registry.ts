import type { PtySession } from './pty-session'

/** Runtime-generation owner for live PTYs; Renderer connections only attach views. */
export class RuntimeSessionRegistry {
  readonly #sessions = new Map<string, PtySession>()

  get(sessionId: string): PtySession | undefined { return this.#sessions.get(sessionId) }
  has(sessionId: string): boolean { return this.#sessions.has(sessionId) }
  set(session: PtySession): void { this.#sessions.set(session.sessionId, session) }
  delete(sessionId: string, expected?: PtySession): boolean {
    if (expected && this.#sessions.get(sessionId) !== expected) return false
    return this.#sessions.delete(sessionId)
  }
  values(): IterableIterator<PtySession> { return this.#sessions.values() }
  disposeAll(): void {
    for (const session of this.#sessions.values()) session.dispose()
    this.#sessions.clear()
  }
  async shutdownAll(): Promise<void> {
    const sessions = [...this.#sessions.values()]
    for (const session of sessions) {
      session.dispose({ notifyExit: false, reason: 'runtime-shutdown' })
    }
    await Promise.all(sessions.map((session) => session.whenClosed()))
    for (const session of sessions) this.delete(session.sessionId, session)
  }
}
