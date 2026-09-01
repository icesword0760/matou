import { randomUUID } from 'node:crypto'

import type { ForkProgress, ForkStage } from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'

export type ForkWorktreeMode = 'current' | 'new'

interface ForkIntentRow {
  session_id: string
  source_session_id: string
  source_provider_session_id: string
  state: 'pending' | 'starting' | 'succeeded' | 'failed'
  error_message: string | null
  operation_id: string
  submission_key: string
  stage: ForkStage
  completed_steps: number
  total_steps: number
  attempt: number
  attempt_count: number
  worktree_mode: ForkWorktreeMode
  worktree_id: string | null
  target_execution_context_id: string | null
  worktree_path: string | null
  branch_name: string | null
  lease_owner: string | null
  lease_token: string | null
  lease_expires_at: number | null
  lease_fence: number
  last_heartbeat_at: number | null
}

export interface ForkOperationIdentity {
  operationId: string
  submissionKey: string
  sessionId: string
  worktreeId?: string
  executionContextId?: string
  worktreePath?: string
  branchName?: string
}

export interface AcceptForkIntentInput extends ForkOperationIdentity {
  sourceSessionId: string
  sourceProviderSessionId: string
  displayName: string
  worktreeMode: ForkWorktreeMode
  totalSteps: number
  now: number
}

export interface ForkAcceptance {
  created: boolean
  identity: ForkOperationIdentity
  progress: ForkProgress
}

export interface ForkLease {
  owner: string
  token: string
  expiresAt: number
  fence: number
}

export type ForkLeaseDecision =
  | { kind: 'acquired'; lease: ForkLease }
  | { kind: 'busy'; owner: string; expiresAt: number; fence: number }

export type FencedMutationResult =
  | { kind: 'applied'; progress: ForkProgress }
  | { kind: 'stale' }

export type ForkLaunchDecision =
  | {
      kind: 'launch'
      sourceSessionId: string
      sourceProviderSessionId: string
    }
  | { kind: 'failed'; error: string }

export class SessionForkIntentRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  accept(input: AcceptForkIntentInput, transaction?: DatabaseTransaction): ForkAcceptance {
    return this.#mutate(transaction, (tx) => {
      const existing = tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE submission_key = ?', input.submissionKey
      )
      if (existing) return acceptance(existing, false)
      tx.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, error_message, created_at, display_name, worktree_mode,
           worktree_id, target_execution_context_id, worktree_path, branch_name,
           attempt_count, updated_at, operation_id, submission_key, stage,
           completed_steps, total_steps, attempt, lease_fence
         ) VALUES (
           ?, ?, 'claude-code', ?, 'pending', NULL, ?, ?, ?, ?, ?, ?, ?,
           0, ?, ?, ?, 'queued', 0, ?, 0, 0
         )`,
        input.sessionId,
        input.sourceSessionId,
        input.sourceProviderSessionId,
        input.now,
        input.displayName,
        input.worktreeMode,
        input.worktreeId ?? null,
        input.executionContextId ?? null,
        input.worktreePath ?? null,
        input.branchName ?? null,
        input.now,
        input.operationId,
        input.submissionKey,
        input.totalSteps
      )
      return acceptance(requireRow(tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE operation_id = ?', input.operationId
      ), `Fork operation ${input.operationId}`), true)
    })
  }

  findBySubmissionKey(
    submissionKey: string,
    source: DatabaseTransaction = this.#database
  ): ForkAcceptance | undefined {
    const row = source.get<ForkIntentRow>(
      'SELECT * FROM session_fork_intents WHERE submission_key = ?', submissionKey
    )
    return row ? acceptance(row, false) : undefined
  }

  progressByOperation(operationId: string): ForkProgress | undefined {
    const row = this.#database.get<ForkIntentRow>(
      'SELECT * FROM session_fork_intents WHERE operation_id = ?', operationId
    )
    return row ? progress(row) : undefined
  }

  acquireLease(input: {
    operationId: string
    owner: string
    now: number
    ttlMs: number
  }): ForkLeaseDecision {
    return this.#database.transaction((tx) => {
      const row = requireRow(tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE operation_id = ?', input.operationId
      ), `Fork operation ${input.operationId}`)
      if (terminal(row.stage)) {
        return { kind: 'busy', owner: row.lease_owner ?? '', expiresAt: row.lease_expires_at ?? 0, fence: row.lease_fence }
      }
      if (
        row.lease_token && row.lease_owner && row.lease_expires_at !== null &&
        row.lease_expires_at > input.now
      ) {
        if (row.lease_owner !== input.owner) {
          return {
            kind: 'busy', owner: row.lease_owner,
            expiresAt: row.lease_expires_at, fence: row.lease_fence
          }
        }
        const lease = {
          owner: row.lease_owner, token: row.lease_token,
          expiresAt: input.now + input.ttlMs, fence: row.lease_fence
        }
        tx.run(
          `UPDATE session_fork_intents
           SET lease_expires_at = ?, last_heartbeat_at = ?, updated_at = ?
           WHERE operation_id = ? AND lease_token = ? AND lease_fence = ?`,
          lease.expiresAt, input.now, input.now,
          input.operationId, lease.token, lease.fence
        )
        return { kind: 'acquired', lease }
      }
      const fence = row.lease_fence + 1
      const lease: ForkLease = {
        owner: input.owner,
        token: `${input.operationId}:${fence}:${randomUUID()}`,
        expiresAt: input.now + input.ttlMs,
        fence
      }
      tx.run(
        `UPDATE session_fork_intents
         SET lease_owner = ?, lease_token = ?, lease_expires_at = ?, lease_fence = ?,
             last_heartbeat_at = ?, updated_at = ? WHERE operation_id = ?`,
        lease.owner, lease.token, lease.expiresAt, lease.fence,
        input.now, input.now, input.operationId
      )
      return { kind: 'acquired', lease }
    })
  }

  heartbeat(input: {
    operationId: string
    lease: Pick<ForkLease, 'token' | 'fence'>
    now: number
    ttlMs: number
  }, transaction?: DatabaseTransaction): FencedMutationResult {
    return this.#mutate(transaction, (tx) => {
      const row = this.#fencedRow(tx, input.operationId, input.lease, input.now)
      if (!row) return { kind: 'stale' }
      tx.run(
        `UPDATE session_fork_intents
         SET lease_expires_at = ?, last_heartbeat_at = ?, updated_at = ?
         WHERE operation_id = ? AND lease_token = ? AND lease_fence = ?
           AND lease_expires_at > ?`,
        input.now + input.ttlMs, input.now, input.now,
        input.operationId, input.lease.token, input.lease.fence, input.now
      )
      return { kind: 'applied', progress: progress(requireRow(tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE operation_id = ?', input.operationId
      ), `Fork operation ${input.operationId}`)) }
    })
  }

  advanceStage(input: {
    operationId: string
    lease: Pick<ForkLease, 'token' | 'fence'>
    stage: ForkStage
    now: number
    error?: string
  }, transaction?: DatabaseTransaction): FencedMutationResult {
    return this.#mutate(transaction, (tx) => {
      const row = this.#fencedRow(tx, input.operationId, input.lease, input.now)
      if (!row) return { kind: 'stale' }
      assertStageTransition(row, input.stage)
      const stages = operationStages(row.worktree_mode)
      const completedSteps = input.stage === 'succeeded'
        ? row.total_steps
        : input.stage === 'failed'
          ? row.completed_steps
          : Math.max(0, stages.indexOf(input.stage as (typeof stages)[number]))
      tx.run(
        `UPDATE session_fork_intents
         SET stage = ?, state = ?, completed_steps = ?, error_message = ?,
             completed_at = CASE WHEN ? IN ('succeeded', 'failed') THEN ? ELSE NULL END,
             updated_at = ?
         WHERE operation_id = ? AND lease_token = ? AND lease_fence = ?
           AND lease_expires_at > ?`,
        input.stage,
        legacyState(input.stage),
        completedSteps,
        input.stage === 'failed' ? requiredError(input.error) : null,
        input.stage,
        input.now,
        input.now,
        input.operationId,
        input.lease.token,
        input.lease.fence,
        input.now
      )
      return { kind: 'applied', progress: progress(requireRow(tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE operation_id = ?', input.operationId
      ), `Fork operation ${input.operationId}`)) }
    })
  }

  complete(
    operationId: string,
    lease: Pick<ForkLease, 'token' | 'fence'>,
    now: number,
    transaction?: DatabaseTransaction
  ): FencedMutationResult {
    return this.advanceStage({ operationId, lease, stage: 'succeeded', now }, transaction)
  }

  failOperation(input: {
    operationId: string
    lease: Pick<ForkLease, 'token' | 'fence'>
    error: string
    now: number
  }, transaction?: DatabaseTransaction): FencedMutationResult {
    return this.advanceStage({ ...input, stage: 'failed' }, transaction)
  }

  retry(operationId: string, now: number, transaction?: DatabaseTransaction): ForkProgress {
    return this.#mutate(transaction, (tx) => {
      const row = requireRow(tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE operation_id = ?', operationId
      ), `Fork operation ${operationId}`)
      if (row.stage !== 'failed') throw new Error(`Fork operation ${operationId} is not failed`)
      const stages = operationStages(row.worktree_mode)
      const retryStage = stages[Math.min(row.completed_steps, stages.length - 1)]!
      tx.run(
        `UPDATE session_fork_intents
         SET stage = ?, state = 'starting', error_message = NULL, completed_at = NULL,
             attempt = attempt + 1, attempt_count = attempt_count + 1,
             lease_owner = NULL, lease_token = NULL, lease_expires_at = NULL,
             last_heartbeat_at = NULL, updated_at = ? WHERE operation_id = ?`,
        retryStage, now, operationId
      )
      return progress(requireRow(tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE operation_id = ?', operationId
      ), `Fork operation ${operationId}`))
    })
  }

  claimForLaunch(sessionId: string, now: number): ForkLaunchDecision | undefined {
    return this.#database.transaction((tx) => {
      const row = tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE session_id = ?', sessionId
      )
      if (!row || row.state === 'succeeded' || !legacyOperation(row.operation_id)) return undefined
      if (activeLease(row, now)) return undefined
      if (row.state === 'failed') {
        return { kind: 'failed', error: row.error_message ?? 'Fork 会话启动失败' }
      }
      tx.run(
        `UPDATE session_fork_intents
         SET state = 'starting',
             stage = CASE
               WHEN stage = 'queued' AND worktree_mode = 'current' THEN 'restoring-provider'
               ELSE stage
             END,
             started_at = ?, error_message = NULL,
             attempt_count = attempt_count + 1, updated_at = ? WHERE session_id = ?`,
        now, now, sessionId
      )
      return {
        kind: 'launch',
        sourceSessionId: row.source_session_id,
        sourceProviderSessionId: row.source_provider_session_id
      }
    })
  }

  fail(sessionId: string, error: string, now: number): void {
    this.#database.run(
      `UPDATE session_fork_intents
       SET state = 'failed', stage = 'failed', error_message = ?, completed_at = ?, updated_at = ?
       WHERE session_id = ? AND state IN ('pending', 'starting')
         AND (operation_id = '' OR operation_id LIKE 'legacy-operation:%')
         AND (lease_token IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= ?)`,
      error, now, now, sessionId, now
    )
  }

  state(sessionId: string): ForkIntentRow['state'] | undefined {
    try {
      return this.#database.get<Pick<ForkIntentRow, 'state'>>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', sessionId
      )?.state
    } catch {
      return undefined
    }
  }

  #fencedRow(
    tx: DatabaseTransaction,
    operationId: string,
    lease: Pick<ForkLease, 'token' | 'fence'>,
    now: number
  ): ForkIntentRow | undefined {
    return tx.get<ForkIntentRow>(
      `SELECT * FROM session_fork_intents
       WHERE operation_id = ? AND lease_token = ? AND lease_fence = ?
         AND lease_expires_at > ?`,
      operationId, lease.token, lease.fence, now
    )
  }

  #mutate<T>(transaction: DatabaseTransaction | undefined, operation: (tx: DatabaseTransaction) => T): T {
    return transaction ? operation(transaction) : this.#database.transaction(operation)
  }
}

function acceptance(row: ForkIntentRow, created: boolean): ForkAcceptance {
  return {
    created,
    identity: {
      operationId: row.operation_id,
      submissionKey: row.submission_key,
      sessionId: row.session_id,
      ...(row.worktree_id === null ? {} : { worktreeId: row.worktree_id }),
      ...(row.target_execution_context_id === null ? {} : { executionContextId: row.target_execution_context_id }),
      ...(row.worktree_path === null ? {} : { worktreePath: row.worktree_path }),
      ...(row.branch_name === null ? {} : { branchName: row.branch_name })
    },
    progress: progress(row)
  }
}

function progress(row: ForkIntentRow): ForkProgress {
  return {
    operationId: row.operation_id,
    sessionId: row.session_id,
    submissionKey: row.submission_key,
    stage: row.stage,
    completedSteps: row.completed_steps,
    totalSteps: row.total_steps,
    attempt: row.attempt,
    ...(row.error_message === null ? {} : { error: row.error_message })
  }
}

function operationStages(mode: ForkWorktreeMode): Exclude<ForkStage, 'queued' | 'succeeded' | 'failed'>[] {
  return mode === 'new'
    ? ['creating-worktree', 'applying-setup', 'binding-session', 'restoring-provider', 'starting-window']
    : ['restoring-provider', 'starting-window']
}

function assertStageTransition(row: ForkIntentRow, target: ForkStage): void {
  if (terminal(row.stage)) throw new Error(`Fork operation ${row.operation_id} is already ${row.stage}`)
  if (target === 'failed') return
  const stages = operationStages(row.worktree_mode)
  const expected = row.stage === 'queued'
    ? stages[0]
    : stages[stages.indexOf(row.stage as (typeof stages)[number]) + 1] ?? 'succeeded'
  if (target !== expected) {
    throw new Error(`Fork stage must advance from ${row.stage} to ${expected}, received ${target}`)
  }
}

function legacyState(stage: ForkStage): ForkIntentRow['state'] {
  if (stage === 'queued') return 'pending'
  if (stage === 'succeeded') return 'succeeded'
  if (stage === 'failed') return 'failed'
  return 'starting'
}

function terminal(stage: ForkStage): boolean {
  return stage === 'succeeded' || stage === 'failed'
}

function legacyOperation(operationId: string): boolean {
  return operationId === '' || operationId.startsWith('legacy-operation:')
}

function activeLease(row: ForkIntentRow, now: number): boolean {
  return row.lease_token !== null && row.lease_expires_at !== null && row.lease_expires_at > now
}

function requiredError(error: string | undefined): string {
  const value = error?.trim()
  if (!value) throw new Error('Fork failure requires an error')
  return value
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (!row) throw new Error(`${label} does not exist`)
  return row
}
