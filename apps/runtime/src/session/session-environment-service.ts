import { randomUUID } from 'node:crypto'
import { realpath } from 'node:fs/promises'

import type {
  SessionEnvironmentActionResult,
  SessionEnvironmentLocateReason,
  SessionEnvironmentOpenResult,
  SessionEnvironmentTarget
} from '@matou/contracts'

import {
  SessionEnvironmentRepository,
  type OwnedWorktreeIdentity
} from './session-environment-repository'
import {
  managedWorktreeIdentityExpectation,
  WorktreeHealthService,
  type WorktreeHealth
} from '../worktrees/worktree-health-service'

export interface SessionEnvironmentServiceDependencies {
  restoreOwnedWorktree(identity: OwnedWorktreeIdentity): Promise<void>
  pauseSession(sessionId: string): Promise<void>
  resumeSession(sessionId: string, target: SessionEnvironmentTarget): Promise<void>
}

type EnvironmentBinding = NonNullable<ReturnType<SessionEnvironmentRepository['get']>>

export class SessionEnvironmentService {
  readonly #environments: SessionEnvironmentRepository
  readonly #dependencies: SessionEnvironmentServiceDependencies
  readonly #health: WorktreeHealthService

  constructor(
    environments: SessionEnvironmentRepository,
    dependencies: SessionEnvironmentServiceDependencies,
    health = new WorktreeHealthService()
  ) {
    this.#environments = environments
    this.#dependencies = dependencies
    this.#health = health
  }

  async restore(input: {
    sessionId: string
    now: number
  }): Promise<SessionEnvironmentActionResult> {
    const original = requireBinding(this.#environments.get(input.sessionId), input.sessionId)
    const identity = this.#requireOwnedWorktree(input.sessionId)
    const operationId = randomUUID()
    this.#environments.beginTransition({
      sessionId: input.sessionId,
      target: 'worktree',
      state: 'recovering',
      now: input.now,
      operation: { operationId, kind: 'restore' }
    })
    try {
      const health = await this.#restoreOwnedWorktree(identity)
      if (health.kind !== 'ready') {
        const reason = healthReason(health)
        this.#restoreOriginalState(original, reason, input.now)
        return rejected(input.sessionId, reason)
      }
      const owner = await this.#findOtherOwner(input.sessionId, health.canonicalPath)
      if (owner) {
        this.#restoreOriginalState(original, 'path-owned-by-another-session', input.now)
        return rejected(input.sessionId, 'path-owned-by-another-session')
      }
      this.#environments.markTransitionExternalReady(input.sessionId, operationId, input.now)
      await this.#dependencies.pauseSession(input.sessionId)
      const binding = this.#environments.completeTransition({
        sessionId: input.sessionId,
        target: 'worktree',
        now: input.now
      })
      await this.#dependencies.resumeSession(input.sessionId, 'worktree')
      return environmentResult(binding, true)
    } catch (error) {
      this.#restoreOriginalState(original, `restore-failed:${errorMessage(error)}`, input.now)
      if (original.state === 'ready') {
        await this.#dependencies.resumeSession(input.sessionId, original.activeTarget)
      }
      throw error
    }
  }

  async locate(input: {
    sessionId: string
    path: string
    now: number
  }): Promise<SessionEnvironmentActionResult> {
    const original = requireBinding(this.#environments.get(input.sessionId), input.sessionId)
    const identity = this.#requireOwnedWorktree(input.sessionId)
    const canonicalPath = await realpath(input.path).catch(() => undefined)
    if (!canonicalPath) {
      this.#restoreOriginalState(original, 'path-missing', input.now)
      return rejected(input.sessionId, 'path-missing')
    }

    const owner = await this.#findOtherOwner(input.sessionId, canonicalPath, true)
    if (owner && owner !== input.sessionId) {
      return { kind: 'switch-session', sessionId: owner }
    }

    const operationId = randomUUID()
    this.#environments.beginTransition({
      sessionId: input.sessionId,
      target: 'worktree',
      state: 'recovering',
      now: input.now,
      operation: { operationId, kind: 'locate', candidatePath: canonicalPath }
    })
    try {
      const health = await this.#health.repairMoved(identityExpectation({
        ...identity,
        path: canonicalPath
      }))
      if (health.kind !== 'ready') {
        const reason = healthReason(health)
        this.#restoreOriginalState(original, reason, input.now)
        return rejected(input.sessionId, reason)
      }
      this.#environments.markTransitionExternalReady(input.sessionId, operationId, input.now)
      await this.#dependencies.pauseSession(input.sessionId)
      const binding = this.#environments.completeRelocation({
        sessionId: input.sessionId,
        path: health.canonicalPath,
        now: input.now
      })
      await this.#dependencies.resumeSession(input.sessionId, 'worktree')
      return environmentResult(binding, true)
    } catch (error) {
      this.#restoreOriginalState(original, `locate-failed:${errorMessage(error)}`, input.now)
      if (original.state === 'ready') {
        await this.#dependencies.resumeSession(input.sessionId, original.activeTarget)
      }
      throw error
    }
  }

  async handoff(input: {
    sessionId: string
    target: SessionEnvironmentTarget
    now: number
  }): Promise<SessionEnvironmentActionResult> {
    const original = requireBinding(this.#environments.get(input.sessionId), input.sessionId)
    if (original.activeTarget === input.target && original.state === 'ready') {
      return environmentResult(original, false)
    }

    const operationId = randomUUID()
    this.#environments.beginTransition({
      sessionId: input.sessionId,
      target: input.target,
      state: 'handoff',
      now: input.now,
      operation: { operationId, kind: 'handoff' }
    })

    let pauseAttempted = false
    try {
      const rejection = await this.#validateTarget(input.sessionId, input.target)
      if (rejection) throw new RejectedEnvironmentTargetError(rejection)
      pauseAttempted = true
      await this.#dependencies.pauseSession(input.sessionId)
      const binding = this.#environments.completeTransition({
        sessionId: input.sessionId,
        target: input.target,
        now: input.now
      })
      await this.#dependencies.resumeSession(input.sessionId, input.target)
      return environmentResult(binding, false)
    } catch (error) {
      this.#restoreOriginalState(original, errorMessage(error), input.now)
      if (pauseAttempted && original.state === 'ready') {
        await this.#dependencies.resumeSession(input.sessionId, original.activeTarget)
      }
      if (error instanceof RejectedEnvironmentTargetError) {
        return rejected(input.sessionId, error.reason)
      }
      throw error
    }
  }

  async open(sessionId: string): Promise<SessionEnvironmentOpenResult> {
    const binding = requireBinding(this.#environments.get(sessionId), sessionId)
    if (binding.state !== 'ready') {
      throw new Error(`Session ${sessionId} environment is ${binding.state}`)
    }
    let path: string | undefined
    if (binding.environment.kind === 'worktree') {
      const health = await this.#health.check(identityExpectation(
        this.#requireOwnedWorktree(sessionId)
      ))
      if (health.kind !== 'ready') {
        throw new Error(`Session ${sessionId} environment identity is ${healthReason(health)}`)
      }
      const owner = await this.#findOtherOwner(sessionId, health.canonicalPath)
      if (owner) throw new Error(`Session ${sessionId} environment belongs to ${owner}`)
      path = health.canonicalPath
    } else {
      path = await realpath(binding.environment.path).catch(() => undefined)
    }
    if (!path) throw new Error(`Session ${sessionId} environment path is missing`)
    return { sessionId, kind: binding.environment.kind, path }
  }

  async reconcileTransitions(now: number): Promise<{
    checked: number
    completed: number
    rolledBack: number
    failed: number
  }> {
    const result = { checked: 0, completed: 0, rolledBack: 0, failed: 0 }
    for (const transition of this.#environments.listTransitions()) {
      result.checked += 1
      try {
        if (transition.kind === 'handoff') {
          this.#restorePersistedTransition(transition, 'handoff-interrupted', now)
          result.rolledBack += 1
          continue
        }
        const identity = this.#requireOwnedWorktree(transition.sessionId)
        let health: WorktreeHealth
        if (transition.kind === 'restore') {
          health = await this.#restoreOwnedWorktree(identity)
        } else {
          if (!transition.candidatePath) throw new Error('locate transition has no candidate path')
          health = await this.#health.repairMoved(identityExpectation({
            ...identity, path: transition.candidatePath
          }))
        }
        if (health.kind !== 'ready') {
          this.#restorePersistedTransition(transition, healthReason(health), now)
          result.rolledBack += 1
          continue
        }
        const owner = await this.#findOtherOwner(
          transition.sessionId, health.canonicalPath
        )
        if (owner) {
          this.#restorePersistedTransition(
            transition, 'path-owned-by-another-session', now
          )
          result.rolledBack += 1
          continue
        }
        this.#environments.markTransitionExternalReady(
          transition.sessionId, transition.operationId, now
        )
        await this.#dependencies.pauseSession(transition.sessionId)
        if (transition.kind === 'locate') {
          this.#environments.completeRelocation({
            sessionId: transition.sessionId,
            path: health.canonicalPath,
            now
          })
        } else {
          this.#environments.completeTransition({
            sessionId: transition.sessionId,
            target: transition.target,
            now
          })
        }
        await this.#dependencies.resumeSession(transition.sessionId, transition.target)
        result.completed += 1
      } catch (error) {
        try {
          this.#restorePersistedTransition(transition, errorMessage(error), now)
          result.rolledBack += 1
        } catch {
          this.#environments.markFailed(transition.sessionId, errorMessage(error), now)
          result.failed += 1
        }
      }
    }
    return result
  }

  async #validateTarget(
    sessionId: string,
    target: SessionEnvironmentTarget
  ): Promise<SessionEnvironmentLocateReason | undefined> {
    if (target === 'local') {
      const path = this.#environments.getTargetPath(sessionId, 'local')
      if (!path) return 'path-missing'
      return await realpath(path).then(
        () => undefined,
        () => 'path-missing' as const
      )
    }
    const health = await this.#health.check(
      identityExpectation(this.#requireOwnedWorktree(sessionId))
    )
    if (health.kind !== 'ready') return healthReason(health)
    const owner = await this.#findOtherOwner(sessionId, health.canonicalPath)
    return owner ? 'path-owned-by-another-session' : undefined
  }

  #requireOwnedWorktree(sessionId: string): OwnedWorktreeIdentity {
    const identity = this.#environments.getOwnedWorktreeIdentity(sessionId)
    if (!identity) throw new Error(`Session ${sessionId} has no owned Worktree`)
    return identity
  }

  async #restoreOwnedWorktree(identity: OwnedWorktreeIdentity): Promise<WorktreeHealth> {
    const expectation = identityExpectation(identity)
    let health = await this.#health.check(expectation)
    // An occupied path with a different Git identity belongs to user data.
    // Setup is only allowed when the original path is genuinely absent.
    if (health.kind === 'mismatch') return health
    if (health.kind === 'missing' && health.reason === 'not-listed-by-git') {
      health = await this.#health.repairMoved(expectation)
    }
    if (health.kind === 'missing' && health.reason === 'path-missing') {
      await this.#dependencies.restoreOwnedWorktree(identity)
      health = await this.#health.check(expectation)
    }
    return health
  }

  async #findOtherOwner(
    sessionId: string,
    canonicalPath: string,
    repairMoved = false
  ): Promise<string | undefined> {
    const direct = this.#environments.findOwningSessionByPath(canonicalPath)
    if (direct && direct !== sessionId) return direct
    // Paths stored in SQLite may be stale after a manual move or symlink. For
    // ambiguous detached Worktrees, fail closed when another owned identity
    // also matches the actual canonical Git worktree.
    for (const candidate of this.#environments.listOwnedWorktreeIdentities()) {
      if (candidate.sessionId === sessionId) continue
      const expectation = identityExpectation({
        ...candidate,
        path: canonicalPath
      })
      const candidateHealth = repairMoved
        ? await this.#health.repairMoved(expectation)
        : await this.#health.check(expectation)
      if (candidateHealth.kind === 'ready') return candidate.sessionId
    }
    return undefined
  }

  #restoreOriginalState(original: EnvironmentBinding, reason: string, now: number): void {
    if (original.state === 'ready') {
      this.#environments.completeTransition({
        sessionId: original.sessionId,
        target: original.activeTarget,
        now
      })
      return
    }
    if (original.state === 'missing' && original.activeTarget === 'worktree') {
      this.#environments.markMissing(original.sessionId, reason, now)
      return
    }
    if (original.state === 'failed') {
      this.#environments.markFailed(
        original.sessionId,
        original.environment.error ?? reason,
        now
      )
      return
    }
    this.#environments.markFailed(original.sessionId, reason, now)
  }

  #restorePersistedTransition(
    transition: ReturnType<SessionEnvironmentRepository['listTransitions']>[number],
    reason: string,
    now: number
  ): void {
    if (transition.previousState === 'ready') {
      this.#environments.completeTransition({
        sessionId: transition.sessionId,
        target: transition.previousActiveTarget,
        now
      })
      return
    }
    if (transition.previousState === 'missing' && transition.previousActiveTarget === 'worktree') {
      this.#environments.markMissing(transition.sessionId, reason, now)
      return
    }
    this.#environments.markFailed(transition.sessionId, reason, now)
  }
}

function identityExpectation(identity: OwnedWorktreeIdentity) {
  return managedWorktreeIdentityExpectation({
    repositoryRoot: identity.repositoryRoot,
    path: identity.path,
    branch: identity.branch,
    ...(identity.baseRevision === undefined ? {} : { baseRevision: identity.baseRevision })
  })
}

function healthReason(
  health: Exclude<WorktreeHealth, { kind: 'ready' }>
): SessionEnvironmentLocateReason {
  return health.reason
}

function rejected(
  sessionId: string,
  reason: SessionEnvironmentLocateReason
): SessionEnvironmentActionResult {
  return { kind: 'rejected', sessionId, reason }
}

function environmentResult(
  binding: EnvironmentBinding,
  restartRequired: boolean
): SessionEnvironmentActionResult {
  return {
    kind: 'environment',
    sessionId: binding.sessionId,
    activeTarget: binding.activeTarget,
    state: binding.state,
    path: binding.environment.path,
    restartRequired
  }
}

function requireBinding<T>(binding: T | undefined, sessionId: string): T {
  if (!binding) throw new Error(`SessionEnvironmentBinding ${sessionId} does not exist`)
  return binding
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

class RejectedEnvironmentTargetError extends Error {
  readonly reason: SessionEnvironmentLocateReason

  constructor(reason: SessionEnvironmentLocateReason) {
    super(reason)
    this.reason = reason
  }
}
