import { randomUUID } from 'node:crypto'

import type { DomainCommandMetadata, ForkStage } from '@matou/domain'

import {
  SessionForkIntentRepository,
  type ForkLease,
  type ForkOperationRecord
} from '../session/session-fork-intent-repository'
import {
  WorktreeHealthService,
  managedWorktreeIdentityExpectation
} from '../worktrees/worktree-health-service'
import type { ExecuteForkInput, ForkWorkflowResult } from './fork-workflow-service'

export type ForkKillPoint =
  | 'intent-accepted'
  | 'branch-created'
  | 'path-created'
  | 'setup-completed'
  | 'session-bound'
  | 'provider-before'

export interface ForkKillPointObserver {
  reach(point: ForkKillPoint, operation: ForkOperationRecord): Promise<void> | void
}

export class ForkKillPointCrash extends Error {
  constructor(point: ForkKillPoint) {
    super(`fork killpoint: ${point}`)
    this.name = 'ForkKillPointCrash'
  }
}

export interface ForkOperationNotification {
  eventId: string
  replacementKey: string
  status: 'succeeded' | 'failed'
  operationId: string
  sessionId: string
  error?: string
}

export interface ForkOperationExecutor {
  executeFork(
    command: DomainCommandMetadata,
    input: ExecuteForkInput
  ): Promise<ForkWorkflowResult>
}

export interface ForkOperationCoordinatorDependencies {
  ownerId: string
  executeFork: ForkOperationExecutor['executeFork']
  startOrResume(
    sessionId: string,
    authority: { operationId: string; runId: string; lease: Pick<ForkLease, 'token' | 'fence'> }
  ): Promise<void | { kind: 'started' | 'deferred' | 'stale-authority' }>
  notify?(notification: ForkOperationNotification): Promise<void> | void
  observer?: ForkKillPointObserver
  health?: Pick<WorktreeHealthService, 'check'>
  now?: () => number
  concurrency?: number
  heartbeatMs?: number
  leaseTtlMs?: number
}

interface ActiveLease {
  lease: ForkLease
  timer: ReturnType<typeof setInterval>
  stale: boolean
}

const NOOP_OBSERVER: ForkKillPointObserver = { reach: () => undefined }

export class ForkOperationCoordinator {
  readonly #intents: SessionForkIntentRepository
  readonly #dependencies: ForkOperationCoordinatorDependencies
  readonly #observer: ForkKillPointObserver
  readonly #health: Pick<WorktreeHealthService, 'check'>
  readonly #now: () => number
  readonly #concurrency: number
  readonly #heartbeatMs: number
  readonly #leaseTtlMs: number
  readonly #running = new Map<string, Promise<void>>()
  readonly #waitingForIdentity = new Set<string>()
  readonly #notifiedTerminal = new Set<string>()
  readonly #terminalNotificationInFlight = new Set<string>()
  readonly #leases = new Map<string, ActiveLease>()
  #scanTimer: ReturnType<typeof setInterval> | undefined
  #stopped = true

  constructor(
    intents: SessionForkIntentRepository,
    dependencies: ForkOperationCoordinatorDependencies
  ) {
    this.#intents = intents
    this.#dependencies = dependencies
    this.#observer = dependencies.observer ?? NOOP_OBSERVER
    this.#health = dependencies.health ?? new WorktreeHealthService()
    this.#now = dependencies.now ?? Date.now
    this.#concurrency = dependencies.concurrency ?? 2
    this.#heartbeatMs = dependencies.heartbeatMs ?? 2_000
    this.#leaseTtlMs = dependencies.leaseTtlMs ?? 8_000
  }

  start(): void {
    if (!this.#stopped) return
    this.#stopped = false
    void this.reconcile()
    this.#scanTimer = setInterval(() => { void this.reconcile() }, this.#heartbeatMs)
    this.#scanTimer.unref?.()
  }

  stop(): void {
    this.#stopped = true
    if (this.#scanTimer) clearInterval(this.#scanTimer)
    this.#scanTimer = undefined
    for (const operationId of [...this.#leases.keys()]) this.#releaseLease(operationId)
    this.#waitingForIdentity.clear()
  }

  async reconcile(): Promise<void> {
    if (this.#stopped) return
    await this.#recoverTerminalNotifications()
    await this.#settleWaitingOperations()
    const available = Math.max(0, this.#concurrency - this.#running.size)
    if (available === 0) return
    const candidates = this.#intents.listClaimable(this.#now(), available)
    for (const operation of candidates) this.#launch(operation)
  }

  recordOrdinaryOutput(_sessionId: string, _text: string): false {
    return false
  }

  async confirmAuthoritativeIdentity(
    sessionId: string,
    providerSessionId: string,
    operationId?: string
  ): Promise<boolean> {
    if (!providerSessionId.trim()) return false
    const operation = operationId
      ? this.#intents.operationById(operationId)
      : this.#intents.nonTerminalBySession(sessionId)
    if (!operation || operation.identity.sessionId !== sessionId) return false
    if (!this.#waitingForIdentity.has(operation.identity.operationId)) return false
    // The provider hook transaction is the completion authority. The
    // coordinator only publishes the product notification and releases its
    // lease after observing that fenced commit.
    if (operation.progress.stage !== 'succeeded') return false
    await this.#notifyTerminalOnce(operation, 'succeeded')
    this.#waitingForIdentity.delete(operation.identity.operationId)
    this.#releaseLease(operation.identity.operationId)
    void this.reconcile()
    return true
  }

  #launch(operation: ForkOperationRecord): void {
    const operationId = operation.identity.operationId
    if (this.#running.has(operationId) || this.#waitingForIdentity.has(operationId)) return
    const active = this.#acquire(operation)
    if (!active) return
    const running = this.#run(operation, active)
      .catch(() => undefined)
      .finally(() => {
        this.#running.delete(operationId)
        if (!this.#waitingForIdentity.has(operationId)) this.#releaseLease(operationId)
        void this.reconcile()
      })
    this.#running.set(operationId, running)
  }

  #acquire(operation: ForkOperationRecord): ActiveLease | undefined {
    const operationId = operation.identity.operationId
    const existing = this.#leases.get(operationId)
    if (existing) return existing
    const owner = `${this.#dependencies.ownerId}:${operationId}`
    const decision = this.#intents.acquireLease({
      operationId,
      owner,
      now: this.#now(),
      ttlMs: this.#leaseTtlMs
    })
    if (decision.kind !== 'acquired') return undefined
    const active: ActiveLease = {
      lease: decision.lease,
      stale: false,
      timer: setInterval(() => {
        const renewed = this.#intents.heartbeat({
          operationId,
          lease: decision.lease,
          now: this.#now(),
          ttlMs: this.#leaseTtlMs
        })
        if (renewed.kind === 'stale') active.stale = true
      }, this.#heartbeatMs)
    }
    active.timer.unref?.()
    this.#leases.set(operationId, active)
    return active
  }

  async #run(operation: ForkOperationRecord, active: ActiveLease): Promise<void> {
    const operationId = operation.identity.operationId
    try {
      await this.#observer.reach('intent-accepted', operation)
      let current = this.#intents.operationById(operationId) ?? operation
      if (beforeProvider(current.progress.stage)) {
        await this.#validateWorktree(current, active)
        const result = await this.#dependencies.executeFork(
          command(operationId, current.progress.attempt, 'execute'),
          {
            windowId: current.windowId,
            sceneId: current.sceneId,
            operationId,
            lease: active.lease,
            now: this.#now(),
            observer: this.#observer
          }
        )
        current = this.#intents.operationById(operationId) ?? current
        if (result.forkProgress?.stage === 'failed' || current.progress.stage === 'failed') {
          await this.#notifyTerminalOnce(current, 'failed', result.error ?? current.progress.error)
          return
        }
      }
      if (current.progress.stage === 'restoring-provider') {
        this.#assertLease(operationId, active)
        await this.#observer.reach('provider-before', current)
        this.#assertLease(operationId, active)
        const started = await this.#dependencies.startOrResume(current.identity.sessionId, {
          operationId,
          runId: randomUUID(),
          lease: active.lease
        })
        if (started && started.kind !== 'started') return
        this.#waitingForIdentity.add(operationId)
      }
      if (current.progress.stage === 'starting-window') {
        this.#assertLease(operationId, active)
        const recovered = this.#intents.recoverInterruptedProviderLaunch({
          operationId,
          lease: active.lease,
          now: this.#now()
        })
        if (recovered.kind !== 'applied') return
        current = this.#intents.operationById(operationId) ?? current
        await this.#observer.reach('provider-before', current)
        this.#assertLease(operationId, active)
        const started = await this.#dependencies.startOrResume(current.identity.sessionId, {
          operationId,
          runId: randomUUID(),
          lease: active.lease
        })
        if (started && started.kind !== 'started') return
        this.#waitingForIdentity.add(operationId)
      }
    } catch (error) {
      if (error instanceof ForkKillPointCrash) return
      if (active.stale) return
      const failed = this.#intents.failOperation({
        operationId,
        lease: active.lease,
        error: errorMessage(error),
        now: this.#now()
      })
      if (failed.kind === 'applied') {
        const current = this.#intents.operationById(operationId) ?? operation
        await this.#notifyTerminalOnce(current, 'failed', errorMessage(error))
      }
    }
  }

  async #settleWaitingOperations(): Promise<void> {
    for (const operationId of [...this.#waitingForIdentity]) {
      const operation = this.#intents.operationById(operationId)
      if (!operation || operation.progress.stage === 'failed') {
        if (operation) await this.#notifyTerminalOnce(operation, 'failed', operation.progress.error)
        this.#waitingForIdentity.delete(operationId)
        this.#releaseLease(operationId)
        continue
      }
      if (operation.progress.stage === 'succeeded') {
        await this.#notifyTerminalOnce(operation, 'succeeded')
        this.#waitingForIdentity.delete(operationId)
        this.#releaseLease(operationId)
      }
    }
  }

  async #recoverTerminalNotifications(): Promise<void> {
    for (const operation of this.#intents.terminalWithoutNotification()) {
      const status = operation.progress.stage
      if (status !== 'succeeded' && status !== 'failed') continue
      await this.#notifyTerminalOnce(operation, status, operation.progress.error)
    }
  }

  async #notifyTerminalOnce(
    operation: ForkOperationRecord,
    status: ForkOperationNotification['status'],
    error?: string
  ): Promise<void> {
    const key = `${operation.identity.operationId}:${status}`
    if (this.#notifiedTerminal.has(key) || this.#terminalNotificationInFlight.has(key)) return
    this.#terminalNotificationInFlight.add(key)
    try {
      await this.#notify(operation, status, error)
      this.#notifiedTerminal.add(key)
    } finally {
      this.#terminalNotificationInFlight.delete(key)
    }
  }

  async #validateWorktree(operation: ForkOperationRecord, active: ActiveLease): Promise<void> {
    if (
      operation.worktreeMode !== 'new' ||
      !operation.repositoryRoot ||
      !operation.identity.worktreePath ||
      !operation.identity.branchName
    ) return
    this.#assertLease(operation.identity.operationId, active)
    const health = await this.#health.check(managedWorktreeIdentityExpectation({
      repositoryRoot: operation.repositoryRoot,
      path: operation.identity.worktreePath,
      branch: operation.identity.branchName
    }))
    this.#assertLease(operation.identity.operationId, active)
    if (health.kind === 'mismatch') {
      throw new Error(`Fork Worktree identity mismatch: ${health.reason}`)
    }
  }

  #assertLease(operationId: string, active: ActiveLease): void {
    if (active.stale) throw new Error('stale Fork lease')
    const heartbeat = this.#intents.heartbeat({
      operationId,
      lease: active.lease,
      now: this.#now(),
      ttlMs: this.#leaseTtlMs
    })
    if (heartbeat.kind === 'stale') {
      active.stale = true
      throw new Error('stale Fork lease')
    }
  }

  async #notify(
    operation: ForkOperationRecord,
    status: ForkOperationNotification['status'],
    error?: string
  ): Promise<void> {
    await this.#dependencies.notify?.({
      eventId: `fork-operation:${operation.identity.operationId}:${status}`,
      replacementKey: `fork-operation:${operation.identity.operationId}`,
      status,
      operationId: operation.identity.operationId,
      sessionId: operation.identity.sessionId,
      ...(error ? { error } : {})
    })
  }

  #releaseLease(operationId: string): void {
    const active = this.#leases.get(operationId)
    if (!active) return
    clearInterval(active.timer)
    this.#leases.delete(operationId)
  }
}

function beforeProvider(stage: ForkStage): boolean {
  return stage === 'queued' || stage === 'creating-worktree' ||
    stage === 'applying-setup' || stage === 'binding-session'
}

function command(operationId: string, attempt: number, suffix: string): DomainCommandMetadata {
  const commandId = `fork-operation:${operationId}:${attempt}:${suffix}`
  return { commandId, commandType: 'fork-operation', requestHash: commandId }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
