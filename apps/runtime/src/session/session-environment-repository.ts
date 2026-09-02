import type {
  SessionEnvironment,
  SessionEnvironmentBinding
} from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'

type EnvironmentState = SessionEnvironmentBinding['state']
type ActiveTarget = SessionEnvironmentBinding['activeTarget']

interface BindingRow {
  session_id: string
  local_execution_context_id: string
  managed_worktree_id: string | null
  active_target: ActiveTarget
  state: EnvironmentState
  error_message: string | null
  updated_at: number
  local_path: string
  worktree_execution_context_id: string | null
  worktree_path: string | null
}

interface BindingIdentityRow {
  session_id: string
  local_execution_context_id: string
  managed_worktree_id: string | null
  active_target: ActiveTarget
  state: EnvironmentState
}

interface WorktreeOwnerRow {
  session_id: string
}

interface WorktreeIdentityRow {
  id: string
  execution_context_id: string
  worktree_path: string
  workspace_id: string
}

interface OwnedWorktreeIdentityRow {
  session_id: string
  worktree_id: string
  execution_context_id: string
  workspace_id: string
  repository_root: string
  worktree_path: string
  branch_name: string
  base_ref: string | null
  base_revision: string | null
}

interface SessionOwnerRow {
  workspace_id: string
}

interface EnvironmentTransitionRow {
  session_id: string
  operation_id: string
  kind: 'restore' | 'locate' | 'handoff'
  previous_active_target: ActiveTarget
  previous_state: 'ready' | 'missing' | 'failed'
  target: ActiveTarget
  candidate_path: string | null
  phase: 'accepted' | 'external-ready' | 'failed'
  error_message: string | null
  created_at: number
  updated_at: number
}

export interface BindOwnedWorktreeInput {
  sessionId: string
  worktreeId: string
  activate: boolean
  now: number
}

export interface BeginEnvironmentTransitionInput {
  sessionId: string
  target: ActiveTarget
  state: 'recovering' | 'handoff'
  now: number
  operation?: {
    operationId: string
    kind: 'restore' | 'locate' | 'handoff'
    candidatePath?: string
  }
}

export interface CompleteEnvironmentTransitionInput {
  sessionId: string
  target: ActiveTarget
  now: number
}

export interface OwnedWorktreeIdentity {
  sessionId: string
  worktreeId: string
  executionContextId: string
  workspaceId: string
  repositoryRoot: string
  path: string
  branch: string
  baseRef?: string
  baseRevision?: string
}

export interface CompleteEnvironmentRelocationInput {
  sessionId: string
  path: string
  now: number
}

export interface SessionEnvironmentTransition {
  sessionId: string
  operationId: string
  kind: 'restore' | 'locate' | 'handoff'
  previousActiveTarget: ActiveTarget
  previousState: 'ready' | 'missing' | 'failed'
  target: ActiveTarget
  candidatePath?: string
  phase: 'accepted' | 'external-ready' | 'failed'
  error?: string
  createdAt: number
  updatedAt: number
}

export class SessionEnvironmentRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  get(sessionId: string, source: DatabaseTransaction = this.#database): SessionEnvironmentBinding | undefined {
    const row = source.get<BindingRow>(
      `SELECT
         binding.*,
         local_context.cwd AS local_path,
         worktree.execution_context_id AS worktree_execution_context_id,
         worktree.worktree_path AS worktree_path
       FROM session_environment_bindings AS binding
       JOIN execution_contexts AS local_context
         ON local_context.id = binding.local_execution_context_id
       LEFT JOIN worktrees AS worktree ON worktree.id = binding.managed_worktree_id
       WHERE binding.session_id = ?`,
      sessionId
    )
    return row ? mapBinding(row) : undefined
  }

  findOwningSession(worktreeId: string, source: DatabaseTransaction = this.#database): string | undefined {
    return source.get<WorktreeOwnerRow>(
      'SELECT session_id FROM session_environment_bindings WHERE managed_worktree_id = ?',
      worktreeId
    )?.session_id
  }

  findOwningSessionByPath(
    path: string,
    source: DatabaseTransaction = this.#database
  ): string | undefined {
    return source.get<WorktreeOwnerRow>(
      `SELECT binding.session_id
       FROM session_environment_bindings AS binding
       JOIN worktrees ON worktrees.id = binding.managed_worktree_id
       WHERE worktrees.worktree_path = ? AND worktrees.state <> 'removed'`,
      path
    )?.session_id
  }

  getOwnedWorktreeIdentity(
    sessionId: string,
    source: DatabaseTransaction = this.#database
  ): OwnedWorktreeIdentity | undefined {
    const row = source.get<OwnedWorktreeIdentityRow>(
      `SELECT
         binding.session_id,
         worktrees.id AS worktree_id,
         worktrees.execution_context_id,
         execution_contexts.workspace_id,
         worktrees.repository_root,
         worktrees.worktree_path,
         worktrees.branch_name,
         worktrees.base_ref,
         worktrees.base_revision
       FROM session_environment_bindings AS binding
       JOIN worktrees ON worktrees.id = binding.managed_worktree_id
       JOIN execution_contexts ON execution_contexts.id = worktrees.execution_context_id
       WHERE binding.session_id = ?`,
      sessionId
    )
    return row ? mapOwnedWorktreeIdentity(row) : undefined
  }

  listOwnedWorktreeIdentities(
    source: DatabaseTransaction = this.#database
  ): OwnedWorktreeIdentity[] {
    return source.all<OwnedWorktreeIdentityRow>(
      `SELECT
         binding.session_id,
         worktrees.id AS worktree_id,
         worktrees.execution_context_id,
         execution_contexts.workspace_id,
         worktrees.repository_root,
         worktrees.worktree_path,
         worktrees.branch_name,
         worktrees.base_ref,
         worktrees.base_revision
       FROM session_environment_bindings AS binding
       JOIN worktrees ON worktrees.id = binding.managed_worktree_id
       JOIN execution_contexts ON execution_contexts.id = worktrees.execution_context_id
       WHERE worktrees.state <> 'removed'
       ORDER BY binding.session_id`
    ).map(mapOwnedWorktreeIdentity)
  }

  getTargetPath(
    sessionId: string,
    target: ActiveTarget,
    source: DatabaseTransaction = this.#database
  ): string | undefined {
    const binding = source.get<BindingIdentityRow>(
      `SELECT session_id, local_execution_context_id, managed_worktree_id,
              active_target, state
       FROM session_environment_bindings WHERE session_id = ?`,
      sessionId
    )
    if (!binding) return undefined
    assertTargetAvailable(binding, target, sessionId)
    if (target === 'local') {
      return source.get<{ cwd: string }>(
        'SELECT cwd FROM execution_contexts WHERE id = ?',
        binding.local_execution_context_id
      )?.cwd
    }
    return source.get<{ worktree_path: string }>(
      'SELECT worktree_path FROM worktrees WHERE id = ?',
      binding.managed_worktree_id!
    )?.worktree_path
  }

  bindOwnedWorktree(
    input: BindOwnedWorktreeInput,
    transaction?: DatabaseTransaction
  ): SessionEnvironmentBinding {
    return this.#mutate(transaction, (tx) => {
      const binding = requireBinding(tx, input.sessionId)
      if (binding.managed_worktree_id !== null && binding.managed_worktree_id !== input.worktreeId) {
        throw new Error(
          `Session ${input.sessionId} already owns Worktree ${binding.managed_worktree_id}`
        )
      }
      const owner = tx.get<WorktreeOwnerRow>(
        `SELECT session_id FROM session_environment_bindings
         WHERE managed_worktree_id = ? AND session_id <> ?`,
        input.worktreeId,
        input.sessionId
      )
      if (owner) {
        throw new Error(`Worktree ${input.worktreeId} is already owned by Session ${owner.session_id}`)
      }
      const sessionOwner = requireRow(tx.get<SessionOwnerRow>(
        `SELECT tasks.workspace_id
         FROM sessions JOIN tasks ON tasks.id = sessions.task_id
         WHERE sessions.id = ?`,
        input.sessionId
      ), `Session ${input.sessionId}`)
      const worktree = requireRow(tx.get<WorktreeIdentityRow>(
        `SELECT worktrees.id, worktrees.execution_context_id, worktrees.worktree_path,
                execution_contexts.workspace_id
         FROM worktrees
         JOIN execution_contexts ON execution_contexts.id = worktrees.execution_context_id
         WHERE worktrees.id = ?`,
        input.worktreeId
      ), `Worktree ${input.worktreeId}`)
      if (sessionOwner.workspace_id !== worktree.workspace_id) {
        throw new Error('Session and Worktree must belong to the same Workspace')
      }

      tx.run(
        `UPDATE session_environment_bindings
         SET managed_worktree_id = ?,
             active_target = CASE WHEN ? = 1 THEN 'worktree' ELSE active_target END,
             state = 'ready', error_message = NULL, updated_at = ?
         WHERE session_id = ?`,
        input.worktreeId,
        input.activate ? 1 : 0,
        input.now,
        input.sessionId
      )
      if (input.activate) {
        updateSessionExecutionContext(
          tx,
          input.sessionId,
          worktree.execution_context_id,
          worktree.worktree_path,
          input.now
        )
      }
      return requireEnvironment(this.get(input.sessionId, tx), input.sessionId)
    })
  }

  beginTransition(
    input: BeginEnvironmentTransitionInput,
    transaction?: DatabaseTransaction
  ): SessionEnvironmentBinding {
    return this.#mutate(transaction, (tx) => {
      const binding = requireBinding(tx, input.sessionId)
      assertTargetAvailable(binding, input.target, input.sessionId)
      if (input.operation) {
        const current = tx.get<EnvironmentTransitionRow>(
          'SELECT * FROM session_environment_transitions WHERE session_id = ?',
          input.sessionId
        )
        if (current && current.operation_id !== input.operation.operationId) {
          throw new Error(`Session ${input.sessionId} already has an environment transition`)
        }
        if (!current) {
          tx.run(
            `INSERT INTO session_environment_transitions (
               session_id, operation_id, kind, previous_active_target, previous_state, target,
               candidate_path, phase, error_message, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, 'accepted', NULL, ?, ?)`,
            input.sessionId,
            input.operation.operationId,
            input.operation.kind,
            binding.active_target,
            binding.state === 'recovering' || binding.state === 'handoff'
              ? 'failed'
              : binding.state,
            input.target,
            input.operation.candidatePath ?? null,
            input.now,
            input.now
          )
        }
      }
      tx.run(
        `UPDATE session_environment_bindings
         SET state = ?, error_message = NULL, updated_at = ?
         WHERE session_id = ?`,
        input.state,
        input.now,
        input.sessionId
      )
      return requireEnvironment(this.get(input.sessionId, tx), input.sessionId)
    })
  }

  completeTransition(
    input: CompleteEnvironmentTransitionInput,
    transaction?: DatabaseTransaction
  ): SessionEnvironmentBinding {
    return this.#mutate(transaction, (tx) => {
      const binding = requireBinding(tx, input.sessionId)
      assertTargetAvailable(binding, input.target, input.sessionId)
      const target = input.target === 'local'
        ? requireRow(tx.get<{ id: string; cwd: string }>(
            'SELECT id, cwd FROM execution_contexts WHERE id = ?',
            binding.local_execution_context_id
          ), `ExecutionContext ${binding.local_execution_context_id}`)
        : requireRow(tx.get<{ id: string; cwd: string }>(
            `SELECT execution_contexts.id, execution_contexts.cwd
             FROM worktrees
             JOIN execution_contexts ON execution_contexts.id = worktrees.execution_context_id
             WHERE worktrees.id = ?`,
            binding.managed_worktree_id!
          ), `Worktree ${binding.managed_worktree_id}`)

      tx.run(
        `UPDATE session_environment_bindings
         SET active_target = ?, state = 'ready', error_message = NULL, updated_at = ?
         WHERE session_id = ?`,
        input.target,
        input.now,
        input.sessionId
      )
      updateSessionExecutionContext(tx, input.sessionId, target.id, target.cwd, input.now)
      tx.run('DELETE FROM session_environment_transitions WHERE session_id = ?', input.sessionId)
      return requireEnvironment(this.get(input.sessionId, tx), input.sessionId)
    })
  }

  completeRelocation(
    input: CompleteEnvironmentRelocationInput,
    transaction?: DatabaseTransaction
  ): SessionEnvironmentBinding {
    const path = input.path.trim()
    if (!path) throw new Error('Relocated Worktree path must not be empty')
    return this.#mutate(transaction, (tx) => {
      const identity = this.getOwnedWorktreeIdentity(input.sessionId, tx)
      if (!identity) throw new Error(`Session ${input.sessionId} has no owned Worktree`)
      const conflict = tx.get<{ id: string }>(
        `SELECT id FROM worktrees
         WHERE worktree_path = ? AND id <> ? AND state <> 'removed'`,
        path,
        identity.worktreeId
      )
      if (conflict) throw new Error(`Worktree path is already registered by ${conflict.id}`)

      tx.run(
        `UPDATE worktrees
         SET worktree_path = ?, state = 'ready', updated_at = ?
         WHERE id = ?`,
        path,
        input.now,
        identity.worktreeId
      )
      tx.run(
        'UPDATE execution_contexts SET cwd = ? WHERE id = ?',
        path,
        identity.executionContextId
      )
      return this.completeTransition({
        sessionId: input.sessionId,
        target: 'worktree',
        now: input.now
      }, tx)
    })
  }

  markMissing(
    sessionId: string,
    error: string,
    now: number,
    transaction?: DatabaseTransaction
  ): SessionEnvironmentBinding {
    const message = requiredMessage(error)
    return this.#mutate(transaction, (tx) => {
      const binding = requireBinding(tx, sessionId)
      if (binding.active_target !== 'worktree' || binding.managed_worktree_id === null) {
        throw new Error(`Session ${sessionId} has no active owned Worktree to mark missing`)
      }
      tx.run(
        `UPDATE session_environment_bindings
         SET state = 'missing', error_message = ?, updated_at = ?
         WHERE session_id = ?`,
        message,
        now,
        sessionId
      )
      tx.run('DELETE FROM session_environment_transitions WHERE session_id = ?', sessionId)
      return requireEnvironment(this.get(sessionId, tx), sessionId)
    })
  }

  markFailed(
    sessionId: string,
    error: string,
    now: number,
    transaction?: DatabaseTransaction
  ): SessionEnvironmentBinding {
    const message = requiredMessage(error)
    return this.#mutate(transaction, (tx) => {
      requireBinding(tx, sessionId)
      tx.run(
        `UPDATE session_environment_bindings
         SET state = 'failed', error_message = ?, updated_at = ?
         WHERE session_id = ?`,
        message,
        now,
        sessionId
      )
      tx.run('DELETE FROM session_environment_transitions WHERE session_id = ?', sessionId)
      return requireEnvironment(this.get(sessionId, tx), sessionId)
    })
  }

  markTransitionExternalReady(
    sessionId: string,
    operationId: string,
    now: number,
    transaction?: DatabaseTransaction
  ): SessionEnvironmentTransition {
    return this.#mutate(transaction, (tx) => {
      const result = tx.run(
        `UPDATE session_environment_transitions
         SET phase = 'external-ready', error_message = NULL, updated_at = ?
         WHERE session_id = ? AND operation_id = ?`,
        now, sessionId, operationId
      )
      if (result.changes !== 1) throw new Error(`Environment transition ${operationId} is stale`)
      return requireTransition(this.getTransition(sessionId, tx), sessionId)
    })
  }

  getTransition(
    sessionId: string,
    source: DatabaseTransaction = this.#database
  ): SessionEnvironmentTransition | undefined {
    const row = source.get<EnvironmentTransitionRow>(
      'SELECT * FROM session_environment_transitions WHERE session_id = ?', sessionId
    )
    return row ? mapTransition(row) : undefined
  }

  listTransitions(source: DatabaseTransaction = this.#database): SessionEnvironmentTransition[] {
    return source.all<EnvironmentTransitionRow>(
      'SELECT * FROM session_environment_transitions ORDER BY created_at, session_id'
    ).map(mapTransition)
  }

  #mutate<T>(
    transaction: DatabaseTransaction | undefined,
    operation: (tx: DatabaseTransaction) => T
  ): T {
    return transaction ? operation(transaction) : this.#database.transaction(operation)
  }
}

function mapOwnedWorktreeIdentity(row: OwnedWorktreeIdentityRow): OwnedWorktreeIdentity {
  return {
    sessionId: row.session_id,
    worktreeId: row.worktree_id,
    executionContextId: row.execution_context_id,
    workspaceId: row.workspace_id,
    repositoryRoot: row.repository_root,
    path: row.worktree_path,
    branch: row.branch_name,
    ...(row.base_ref === null ? {} : { baseRef: row.base_ref }),
    ...(row.base_revision === null ? {} : { baseRevision: row.base_revision })
  }
}

function mapBinding(row: BindingRow): SessionEnvironmentBinding {
  const environment = mapEnvironment(row)
  return {
    sessionId: row.session_id,
    localExecutionContextId: row.local_execution_context_id,
    ...(row.managed_worktree_id === null ? {} : { managedWorktreeId: row.managed_worktree_id }),
    activeTarget: row.active_target,
    state: row.state,
    updatedAt: row.updated_at,
    environment
  }
}

function mapTransition(row: EnvironmentTransitionRow): SessionEnvironmentTransition {
  return {
    sessionId: row.session_id,
    operationId: row.operation_id,
    kind: row.kind,
    previousActiveTarget: row.previous_active_target,
    previousState: row.previous_state,
    target: row.target,
    ...(row.candidate_path === null ? {} : { candidatePath: row.candidate_path }),
    phase: row.phase,
    ...(row.error_message === null ? {} : { error: row.error_message }),
    createdAt: row.created_at,
    updatedAt: row.updated_at
  }
}

function mapEnvironment(row: BindingRow): SessionEnvironment {
  if (row.active_target === 'local') {
    if (row.state === 'missing') {
      throw new Error(`Local environment for Session ${row.session_id} cannot be missing`)
    }
    if (row.state === 'failed') {
      return {
        kind: 'local', state: 'failed', path: row.local_path,
        localExecutionContextId: row.local_execution_context_id,
        error: row.error_message ?? 'Local environment failed'
      }
    }
    if (row.state === 'ready') {
      return {
        kind: 'local', state: 'ready', path: row.local_path,
        localExecutionContextId: row.local_execution_context_id
      }
    }
    return {
      kind: 'local', state: row.state, path: row.local_path,
      localExecutionContextId: row.local_execution_context_id,
      ...(row.error_message === null ? {} : { error: row.error_message })
    }
  }
  if (
    row.managed_worktree_id === null ||
    row.worktree_execution_context_id === null ||
    row.worktree_path === null
  ) {
    throw new Error(`Owned Worktree identity for Session ${row.session_id} is incomplete`)
  }
  const base = {
    kind: 'worktree' as const,
    path: row.worktree_path,
    localExecutionContextId: row.local_execution_context_id,
    worktreeId: row.managed_worktree_id,
    worktreeExecutionContextId: row.worktree_execution_context_id
  }
  if (row.state === 'missing' || row.state === 'failed') {
    return {
      ...base,
      state: row.state,
      error: row.error_message ?? `Worktree environment is ${row.state}`
    }
  }
  if (row.state === 'ready') {
    return { ...base, state: 'ready' }
  }
  return {
    ...base,
    state: row.state,
    ...(row.error_message === null ? {} : { error: row.error_message })
  }
}

function requireBinding(source: DatabaseTransaction, sessionId: string): BindingIdentityRow {
  return requireRow(source.get<BindingIdentityRow>(
    `SELECT session_id, local_execution_context_id, managed_worktree_id,
            active_target, state
     FROM session_environment_bindings WHERE session_id = ?`,
    sessionId
  ), `SessionEnvironmentBinding ${sessionId}`)
}

function assertTargetAvailable(
  binding: BindingIdentityRow,
  target: ActiveTarget,
  sessionId: string
): void {
  if (target === 'worktree' && binding.managed_worktree_id === null) {
    throw new Error(`Session ${sessionId} has no owned Worktree`)
  }
}

function updateSessionExecutionContext(
  tx: DatabaseTransaction,
  sessionId: string,
  executionContextId: string,
  cwd: string,
  now: number
): void {
  tx.run(
    `UPDATE sessions
     SET execution_context_id = ?, cwd = ?, updated_at = ?, version = version + 1
     WHERE id = ?`,
    executionContextId,
    cwd,
    now,
    sessionId
  )
}

function requiredMessage(value: string): string {
  const message = value.trim()
  if (!message) throw new Error('Environment error message must not be empty')
  return message
}

function requireEnvironment(
  binding: SessionEnvironmentBinding | undefined,
  sessionId: string
): SessionEnvironmentBinding {
  if (!binding) throw new Error(`SessionEnvironmentBinding ${sessionId} does not exist`)
  return binding
}

function requireTransition(
  transition: SessionEnvironmentTransition | undefined,
  sessionId: string
): SessionEnvironmentTransition {
  if (!transition) throw new Error(`SessionEnvironmentTransition ${sessionId} does not exist`)
  return transition
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (!row) throw new Error(`${label} does not exist`)
  return row
}
