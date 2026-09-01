import type { WorktreeState } from '@matou/domain'

import { SessionEnvironmentRepository } from '../session/session-environment-repository'
import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'
import {
  WorktreeHealthService,
  type WorktreeHealth
} from './worktree-health-service'
import { WorktreeService, type WorktreeSetupStep } from './worktree-service'

interface ReconcileRow {
  id: string
  execution_context_id: string
  workspace_id: string
  repository_root: string
  worktree_path: string
  branch_name: string
  base_ref: string | null
  base_revision: string | null
  state: WorktreeState
  setup_policy_json: string
}

export interface WorktreeReconcileResult {
  checked: number
  repaired: number
  degraded: number
}

export class WorktreeReconciler {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager
  readonly #worktrees: WorktreeService
  readonly #health: WorktreeHealthService
  readonly #environments: SessionEnvironmentRepository

  constructor(
    database: RuntimeDatabase,
    transactions: DomainTransactionManager,
    worktrees: WorktreeService,
    health = new WorktreeHealthService()
  ) {
    this.#database = database
    this.#transactions = transactions
    this.#worktrees = worktrees
    this.#health = health
    this.#environments = new SessionEnvironmentRepository(database)
  }

  async reconcileAll(now: number): Promise<WorktreeReconcileResult> {
    const rows = this.#database.all<ReconcileRow>(
      `SELECT worktrees.*, execution_contexts.workspace_id
       FROM worktrees
       JOIN execution_contexts ON execution_contexts.id = worktrees.execution_context_id
       WHERE worktrees.state IN ('creating', 'ready', 'dirty', 'retained', 'removing')
       ORDER BY worktrees.created_at, worktrees.id`
    )
    const result: WorktreeReconcileResult = { checked: 0, repaired: 0, degraded: 0 }
    for (const row of rows) {
      result.checked += 1
      try {
        if (row.state === 'creating') {
          await this.#reconcileCreating(row, now, result)
        } else if (row.state === 'removing') {
          await this.#reconcileRemoving(row, now, result)
        } else {
          await this.#reconcileReady(row, now, result)
        }
      } catch (error) {
        this.#degradeWithReason(
          row,
          'failed',
          `health-check-failed:${errorMessage(error)}`,
          now
        )
        result.degraded += 1
      }
    }
    return result
  }

  async #reconcileCreating(
    row: ReconcileRow,
    now: number,
    result: WorktreeReconcileResult
  ): Promise<void> {
    const health = await this.#check(row)
    if (health.kind === 'mismatch') {
      this.#degrade(row, health, now)
      result.degraded += 1
      return
    }
    try {
      await this.#worktrees.create(command(row, 'creating', now), {
        id: row.id,
        executionContextId: row.execution_context_id,
        workspaceId: row.workspace_id,
        repositoryRoot: row.repository_root,
        path: row.worktree_path,
        branch: row.branch_name,
        baseRef: row.base_ref ?? row.base_revision ?? 'HEAD',
        setupPolicy: parseSetupPolicy(row.setup_policy_json),
        now
      })
      result.repaired += 1
    } catch (error) {
      this.#degradeWithReason(row, 'failed', `creation-failed:${errorMessage(error)}`, now)
      result.degraded += 1
    }
  }

  async #reconcileRemoving(
    row: ReconcileRow,
    now: number,
    result: WorktreeReconcileResult
  ): Promise<void> {
    const health = await this.#check(row)
    if (
      health.kind === 'mismatch' ||
      (health.kind === 'missing' && health.reason === 'not-listed-by-git')
    ) {
      this.#degrade(row, health, now)
      result.degraded += 1
      return
    }
    try {
      await this.#worktrees.remove(command(row, 'removing', now), row.id, now)
      result.repaired += 1
    } catch {
      result.degraded += 1
    }
  }

  async #reconcileReady(
    row: ReconcileRow,
    now: number,
    result: WorktreeReconcileResult
  ): Promise<void> {
    const health = await this.#check(row)
    if (health.kind !== 'ready') {
      this.#degrade(row, health, now)
      result.degraded += 1
      return
    }
    if (row.state === 'retained') return
    const targetState: WorktreeState = health.dirty ? 'dirty' : 'ready'
    if (targetState === row.state) return
    this.#transactions.execute(command(row, `state-${targetState}`, now), ({ tx, emit }) => {
      tx.run('UPDATE worktrees SET state = ?, updated_at = ? WHERE id = ?', targetState, now, row.id)
      emit({
        eventId: `worktree-reconcile-${row.id}-${targetState}-${now}`,
        eventType: `worktree.${targetState}`,
        aggregateType: 'worktree',
        aggregateId: row.id,
        workspaceId: row.workspace_id,
        payload: { worktreeId: row.id, state: targetState },
        occurredAt: now
      })
      return null
    })
    result.repaired += 1
  }

  async #check(row: ReconcileRow): Promise<WorktreeHealth> {
    return this.#health.check({
      repositoryRoot: row.repository_root,
      path: row.worktree_path,
      expectedBranch: row.branch_name
    })
  }

  #degrade(row: ReconcileRow, health: Exclude<WorktreeHealth, { kind: 'ready' }>, now: number): void {
    const reason = `${health.kind}:${health.reason}`
    this.#degradeWithReason(
      row,
      health.kind === 'missing' ? 'missing' : 'failed',
      reason,
      now
    )
  }

  #degradeWithReason(
    row: ReconcileRow,
    environmentState: 'missing' | 'failed',
    reason: string,
    now: number
  ): void {
    this.#transactions.execute(command(row, 'degraded', now), ({ tx, emit }) => {
      tx.run("UPDATE worktrees SET state = 'failed', updated_at = ? WHERE id = ?", now, row.id)
      const sessions = tx.all<{ session_id: string }>(
        `SELECT session_id FROM session_environment_bindings
         WHERE managed_worktree_id = ? AND active_target = 'worktree'`,
        row.id
      )
      for (const { session_id: sessionId } of sessions) {
        if (environmentState === 'missing') {
          this.#environments.markMissing(sessionId, reason, now, tx)
        } else {
          this.#environments.markFailed(sessionId, reason, now, tx)
        }
      }
      emit({
        eventId: `worktree-reconcile-${row.id}-degraded-${now}`,
        eventType: 'worktree.health-degraded',
        aggregateType: 'worktree',
        aggregateId: row.id,
        workspaceId: row.workspace_id,
        payload: { worktreeId: row.id, state: environmentState, reason },
        occurredAt: now
      })
      return null
    })
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function parseSetupPolicy(value: string): WorktreeSetupStep[] {
  const parsed = JSON.parse(value) as unknown
  if (!Array.isArray(parsed)) throw new Error('Worktree setup policy must be an array')
  return parsed as WorktreeSetupStep[]
}

function command(row: ReconcileRow, stage: string, now: number) {
  return {
    commandId: `worktree-reconcile-${row.id}-${stage}-${now}`,
    commandType: `worktree.reconcile.${stage}`,
    requestHash: `${row.id}:${row.state}:${stage}:${now}`
  }
}
