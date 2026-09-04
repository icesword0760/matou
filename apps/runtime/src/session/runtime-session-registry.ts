import type { PtySession } from './pty-session'

interface PendingProviderIdentity {
  runId: string
  confirm(): void
  reject(): void
}

/** Runtime-generation owner for live PTYs; Renderer connections only attach views. */
export class RuntimeSessionRegistry {
  readonly #sessions = new Map<string, PtySession>()
  readonly #operations = new Map<string, Promise<void>>()
  readonly #pendingProviderRuns = new Map<string, PendingProviderIdentity>()
  readonly #confirmedProviderRuns = new Set<string>()

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
  markProviderIdentityPending(
    sessionId: string,
    runId: string,
    callbacks: Pick<PendingProviderIdentity, 'confirm' | 'reject'>
  ): void {
    this.#confirmedProviderRuns.delete(runId)
    this.#pendingProviderRuns.set(sessionId, { runId, ...callbacks })
  }
  providerIdentityPending(sessionId: string): boolean {
    return this.#pendingProviderRuns.has(sessionId)
  }
  confirmProviderIdentity(sessionId: string, runId: string): boolean {
    const pending = this.#pendingProviderRuns.get(sessionId)
    if (!pending) {
      if (this.#sessions.get(sessionId)?.runId !== runId) return false
      this.#confirmedProviderRuns.add(runId)
      return true
    }
    if (pending.runId !== runId) return false
    this.#pendingProviderRuns.delete(sessionId)
    this.#confirmedProviderRuns.add(runId)
    pending.confirm()
    return true
  }
  rejectProviderIdentity(sessionId: string, expectedRunId?: string): boolean {
    const pending = this.#pendingProviderRuns.get(sessionId)
    if (!pending || (expectedRunId !== undefined && pending.runId !== expectedRunId)) return false
    this.#pendingProviderRuns.delete(sessionId)
    this.#confirmedProviderRuns.delete(pending.runId)
    pending.reject()
    return true
  }
  providerIdentityConfirmed(runId: string): boolean {
    return this.#confirmedProviderRuns.has(runId)
  }
  forgetProviderIdentity(sessionId: string, runId?: string): void {
    if (runId !== undefined) this.#confirmedProviderRuns.delete(runId)
    this.rejectProviderIdentity(sessionId, runId)
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
    this.#rejectAllProviderIdentities()
  }
  async shutdownAll(): Promise<void> {
    const sessions = [...this.#sessions.values()]
    await Promise.all(sessions.map((session) => session.shutdownForRuntime()))
    for (const session of sessions) this.delete(session.sessionId, session)
    this.#rejectAllProviderIdentities()
  }
  #rejectAllProviderIdentities(): void {
    for (const [sessionId, pending] of [...this.#pendingProviderRuns]) {
      this.rejectProviderIdentity(sessionId, pending.runId)
    }
    this.#confirmedProviderRuns.clear()
  }
}
