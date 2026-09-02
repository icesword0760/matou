import {
  RuntimeSessionRecoveryScheduler,
  type RecoveryJob,
  type RecoveryJobSnapshot
} from './runtime-session-recovery-scheduler'

export class RuntimeRecoveryCoordinator {
  readonly #jobs = new Map<string, RecoveryJob>()
  readonly #scheduler: RuntimeSessionRecoveryScheduler
  #started = false

  constructor(options: {
    concurrency: number
    jobs: readonly RecoveryJob[]
    start(job: RecoveryJob): Promise<void>
    publish?(snapshot: readonly RecoveryJobSnapshot[]): void
  }) {
    for (const job of options.jobs) this.#jobs.set(job.sessionId, job)
    this.#scheduler = new RuntimeSessionRecoveryScheduler({
      concurrency: options.concurrency,
      start: options.start,
      ...(options.publish ? { onChange: options.publish } : {})
    })
  }

  start(): void {
    if (this.#started) return
    this.#started = true
    this.#scheduler.enqueue([...this.#jobs.values()])
  }

  prioritizeScene(
    sceneId: string,
    activeSessionId?: string,
    foregroundSessionIds?: readonly string[]
  ): void {
    this.#scheduler.prioritizeScene(
      sceneId,
      activeSessionId,
      foregroundSessionIds ?? (activeSessionId ? [activeSessionId] : [])
    )
  }

  settleExternal(sessionId: string, state: 'ready' | 'failed', error?: string): void {
    this.#scheduler.settleExternal(sessionId, state, error)
  }

  trackExternal(job: RecoveryJob & { recoveryAuthority: 'fork' }): void {
    this.#jobs.set(job.sessionId, job)
    if (this.#started) this.#scheduler.enqueue([job])
  }

  cancel(sessionIds: readonly string[]): void {
    for (const sessionId of sessionIds) this.#jobs.delete(sessionId)
    this.#scheduler.cancel(sessionIds)
  }

  retry(sessionId: string): void {
    const job = this.#jobs.get(sessionId)
    if (job) this.#scheduler.enqueue([job])
  }

  snapshot(): readonly RecoveryJobSnapshot[] {
    return this.#scheduler.snapshot()
  }

  whenIdle(): Promise<void> {
    return this.#scheduler.whenIdle()
  }
}
