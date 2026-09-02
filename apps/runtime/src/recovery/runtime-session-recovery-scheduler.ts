export type RecoveryState = 'queued' | 'restoring' | 'ready' | 'failed'
export type RecoveryPriority =
  | 'active-session'
  | 'foreground-scene'
  | 'active-task'
  | 'active-workspace'
  | 'background'

export interface RecoveryJob {
  sessionId: string
  sceneId: string
  priority: RecoveryPriority
  enqueueSequence: number
  workspaceId?: string
  taskId?: string
  executionContextId?: string
  profile?: 'shell' | 'claude-code' | 'codex'
  recoveryAuthority?: 'fork'
}

export interface RecoveryJobSnapshot extends RecoveryJob {
  state: RecoveryState
  error?: string
}

interface MutableRecoveryJob extends RecoveryJobSnapshot {
  insertionOrder: number
}

export class RuntimeSessionRecoveryScheduler {
  readonly #concurrency: number
  readonly #start: (job: RecoveryJob) => Promise<void>
  readonly #onChange: ((snapshot: readonly RecoveryJobSnapshot[]) => void) | undefined
  readonly #jobs = new Map<string, MutableRecoveryJob>()
  readonly #idleWaiters = new Set<() => void>()
  #activeSceneId: string | undefined
  #activeSessionId: string | undefined
  #foregroundSessionIds = new Set<string>()
  #running = 0
  #insertionOrder = 0
  #foregroundBurst = 0
  #drainScheduled = false

  constructor(options: {
    concurrency: number
    start(job: RecoveryJob): Promise<void>
    onChange?(snapshot: readonly RecoveryJobSnapshot[]): void
  }) {
    if (!Number.isInteger(options.concurrency) || options.concurrency < 1) {
      throw new Error('recovery concurrency must be a positive integer')
    }
    this.#concurrency = options.concurrency
    this.#start = options.start
    this.#onChange = options.onChange
  }

  get runningCount(): number { return this.#running }

  enqueue(jobs: readonly RecoveryJob[]): void {
    let changed = false
    for (const job of jobs) {
      const current = this.#jobs.get(job.sessionId)
      if (current && current.state !== 'failed') continue
      this.#jobs.set(job.sessionId, {
        ...job,
        state: job.recoveryAuthority === 'fork' ? 'restoring' : 'queued',
        insertionOrder: current?.insertionOrder ?? this.#insertionOrder++
      })
      changed = true
    }
    if (changed) this.#changed()
    this.#scheduleDrain()
  }

  prioritizeScene(
    sceneId: string,
    activeSessionId?: string,
    foregroundSessionIds: readonly string[] = activeSessionId ? [activeSessionId] : []
  ): void {
    this.#activeSceneId = sceneId
    this.#activeSessionId = activeSessionId
    this.#foregroundSessionIds = new Set(foregroundSessionIds)
    this.#changed()
    this.#scheduleDrain()
  }

  settleExternal(sessionId: string, state: 'ready' | 'failed', error?: string): void {
    const current = this.#jobs.get(sessionId)
    if (!current || current.recoveryAuthority !== 'fork') return
    current.state = state
    if (error === undefined) delete current.error
    else current.error = error
    this.#changed()
    this.#resolveIdleIfNeeded()
  }

  cancel(sessionIds: readonly string[]): void {
    let changed = false
    for (const sessionId of sessionIds) {
      const current = this.#jobs.get(sessionId)
      if (!current) continue
      if (current.state === 'restoring' && current.recoveryAuthority !== 'fork') continue
      this.#jobs.delete(sessionId)
      changed = true
    }
    if (changed) this.#changed()
    this.#resolveIdleIfNeeded()
  }

  snapshot(): readonly RecoveryJobSnapshot[] {
    return [...this.#jobs.values()]
      .sort((left, right) => left.insertionOrder - right.insertionOrder)
      .map(({ insertionOrder: _insertionOrder, ...job }) => ({ ...job }))
  }

  whenIdle(): Promise<void> {
    if (this.#isIdle()) return Promise.resolve()
    return new Promise((resolve) => this.#idleWaiters.add(resolve))
  }

  #scheduleDrain(): void {
    if (this.#drainScheduled) return
    this.#drainScheduled = true
    queueMicrotask(() => {
      this.#drainScheduled = false
      this.#drain()
    })
  }

  #drain(): void {
    while (this.#running < this.#concurrency) {
      const candidate = this.#next()
      if (!candidate) break
      candidate.state = 'restoring'
      delete candidate.error
      this.#running += 1
      this.#foregroundBurst = this.#isForeground(candidate)
        ? this.#foregroundBurst + 1
        : 0
      this.#changed()
      const publicJob = this.#publicJob(candidate)
      void Promise.resolve().then(() => this.#start(publicJob)).then(
        () => this.#settle(candidate, 'ready'),
        (error: unknown) => this.#settle(candidate, 'failed', errorMessage(error))
      )
    }
    this.#resolveIdleIfNeeded()
  }

  #settle(candidate: MutableRecoveryJob, state: 'ready' | 'failed', error?: string): void {
    const current = this.#jobs.get(candidate.sessionId)
    if (current === candidate) {
      candidate.state = state
      if (error === undefined) delete candidate.error
      else candidate.error = error
    }
    this.#running -= 1
    this.#changed()
    this.#drain()
  }

  #next(): MutableRecoveryJob | undefined {
    const queued = [...this.#jobs.values()].filter(({ state }) => state === 'queued')
    if (queued.length === 0) return undefined
    const background = queued.filter((candidate) => !this.#isForeground(candidate))
    if (this.#foregroundBurst >= 8 && background.length > 0) {
      return background.sort((left, right) => this.#compare(left, right))[0]
    }
    return queued.sort((left, right) => this.#compare(left, right))[0]
  }

  #compare(left: MutableRecoveryJob, right: MutableRecoveryJob): number {
    const rankDifference = this.#rank(left) - this.#rank(right)
    if (rankDifference !== 0) return rankDifference
    const sequenceDifference = left.enqueueSequence - right.enqueueSequence
    return sequenceDifference !== 0 ? sequenceDifference : left.insertionOrder - right.insertionOrder
  }

  #rank(job: RecoveryJob): number {
    if (job.sessionId === this.#activeSessionId) return 0
    if (this.#foregroundSessionIds.has(job.sessionId)) return 1
    return PRIORITY_RANK[job.priority] + (this.#activeSceneId === undefined ? 0 : 2)
  }

  #isForeground(job: RecoveryJob): boolean {
    return job.sessionId === this.#activeSessionId || this.#foregroundSessionIds.has(job.sessionId)
  }

  #publicJob(job: MutableRecoveryJob): RecoveryJob {
    const { state: _state, error: _error, insertionOrder: _insertionOrder, ...publicJob } = job
    return publicJob
  }

  #changed(): void {
    this.#onChange?.(this.snapshot())
  }

  #isIdle(): boolean {
    return this.#running === 0 && ![...this.#jobs.values()].some(({ state }) => state === 'queued')
  }

  #resolveIdleIfNeeded(): void {
    if (!this.#isIdle()) return
    for (const resolve of this.#idleWaiters) resolve()
    this.#idleWaiters.clear()
  }
}

const PRIORITY_RANK: Record<RecoveryPriority, number> = {
  'active-session': 0,
  'foreground-scene': 1,
  'active-task': 2,
  'active-workspace': 3,
  background: 4
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
