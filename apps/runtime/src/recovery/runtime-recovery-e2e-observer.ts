import type {
  RecoveryJobSnapshot,
  RecoveryPriority,
  RecoveryState
} from './runtime-session-recovery-scheduler'

export interface RecoveryE2eTransition {
  sequence: number
  sessionId: string
  sceneId: string
  priority: RecoveryPriority
  state: RecoveryState
  restoringCount: number
}

export interface RecoveryE2eSnapshot {
  maxRestoring: number
  transitions: RecoveryE2eTransition[]
}

/** Read-only trace instantiated only by MATOU_E2E runs. */
export class RuntimeRecoveryE2eObserver {
  readonly #states = new Map<string, RecoveryState>()
  readonly #transitions: RecoveryE2eTransition[] = []
  #maxRestoring = 0
  #sequence = 0

  record(snapshot: readonly RecoveryJobSnapshot[]): void {
    const restoringCount = snapshot.filter(({ state }) => state === 'restoring').length
    this.#maxRestoring = Math.max(this.#maxRestoring, restoringCount)
    for (const job of snapshot) {
      if (this.#states.get(job.sessionId) === job.state) continue
      this.#states.set(job.sessionId, job.state)
      this.#transitions.push({
        sequence: ++this.#sequence,
        sessionId: job.sessionId,
        sceneId: job.sceneId,
        priority: job.priority,
        state: job.state,
        restoringCount
      })
    }
  }

  snapshot(): RecoveryE2eSnapshot {
    return {
      maxRestoring: this.#maxRestoring,
      transitions: this.#transitions.map((transition) => ({ ...transition }))
    }
  }
}
