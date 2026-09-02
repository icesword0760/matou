import { execFile } from 'node:child_process'
import { access } from 'node:fs/promises'
import { promisify } from 'node:util'

import type {
  DomainCommandMetadata,
  Worktree,
  WorktreeState
} from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type {
  DomainMutationContext,
  DomainTransactionManager
} from '../storage/domain-transaction'
import { SessionEnvironmentRepository } from '../session/session-environment-repository'

const exec = promisify(execFile)

export interface WorktreeSetupStep {
  idempotencyKey?: string
  command: string
  args: string[]
}

interface WorktreeSetupCheckpoint {
  idempotencyKey: string
  command: string
  status: 'running' | 'succeeded' | 'failed'
  output: string
}

interface WorktreeRow {
  id: string
  execution_context_id: string
  repository_root: string
  worktree_path: string
  branch_name: string
  base_ref: string | null
  base_revision: string | null
  state: WorktreeState
  setup_policy_json: string
  setup_result_json: string
  cleanup_policy: 'retain-dirty'
  created_at: number
  updated_at: number
  retained_at: number | null
}

export class WorktreeService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager
  readonly #stopRuns: (runIds: string[]) => Promise<void>
  readonly #environments: SessionEnvironmentRepository

  constructor(
    database: RuntimeDatabase,
    transactions: DomainTransactionManager,
    dependencies: { stopRuns: (runIds: string[]) => Promise<void> }
  ) {
    this.#database = database
    this.#transactions = transactions
    this.#stopRuns = dependencies.stopRuns
    this.#environments = new SessionEnvironmentRepository(database)
  }

  async create(
    command: DomainCommandMetadata,
    input: {
      id: string
      executionContextId: string
      workspaceId: string
      repositoryRoot: string
      path: string
      branch: string
      baseRef: string
      setupPolicy: WorktreeSetupStep[]
      now: number
      beforeExternalSideEffect?: () => void
      onSetupStarted?: () => Promise<void> | void
      onCheckpoint?: (
        point: 'branch-created' | 'path-created' | 'setup-completed'
      ) => Promise<void> | void
    }
  ): Promise<Worktree> {
    validateSetupPolicy(input.setupPolicy)
    let worktree = this.get(input.id)
    if (!worktree) {
      worktree = this.#transactions.execute(command, ({ tx, emit }) => {
        const workspace = tx.get<{ id: string }>('SELECT id FROM workspaces WHERE id = ?', input.workspaceId)
        if (!workspace) throw new Error(`Workspace ${input.workspaceId} does not exist`)
        tx.run(
          `INSERT INTO execution_contexts (
             id, workspace_id, kind, cwd, created_at
           ) VALUES (?, ?, 'git-worktree', ?, ?)`,
          input.executionContextId,
          input.workspaceId,
          input.path,
          input.now
        )
        tx.run(
          `INSERT INTO worktrees (
             id, execution_context_id, repository_root, worktree_path, branch_name,
             base_ref, state, setup_policy_json, setup_result_json, cleanup_policy, created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, ?, 'creating', ?, '[]', 'retain-dirty', ?, ?)`,
          input.id,
          input.executionContextId,
          input.repositoryRoot,
          input.path,
          input.branch,
          input.baseRef,
          JSON.stringify(input.setupPolicy),
          input.now,
          input.now
        )
        const created = requireWorktreeRow(tx.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', input.id))
        emitWorktree(emit, command.commandId, 'worktree.creation-started', created, input.workspaceId, input.now)
        return mapWorktree(created)
      }).result
    }
    if (worktree.state === 'ready') return worktree
    if (worktree.state !== 'creating' && worktree.state !== 'failed') {
      throw new Error(`Worktree ${input.id} is in state ${worktree.state}`)
    }

    try {
      input.beforeExternalSideEffect?.()
      const repository = (await exec('git', ['-C', input.repositoryRoot, 'rev-parse', '--show-toplevel'])).stdout.trim()
      input.beforeExternalSideEffect?.()
      const baseRevision = (await exec('git', ['-C', repository, 'rev-parse', input.baseRef])).stdout.trim()
      input.beforeExternalSideEffect?.()
      if (!(await pathIsGitWorktree(input.path))) {
        // `git worktree add -b` creates the branch before it creates the target
        // directory. A permission or disk failure can therefore leave the ref
        // behind while no usable worktree exists. Retrying with `-b` can never
        // recover that valid partial result; reuse the same operation-owned
        // branch and let Git still reject it if another worktree has it checked
        // out.
        const args = await localBranchExists(repository, input.branch)
          ? ['-C', repository, 'worktree', 'add', input.path, input.branch]
          : ['-C', repository, 'worktree', 'add', '-b', input.branch, input.path, input.baseRef]
        input.beforeExternalSideEffect?.()
        await exec('git', args)
        input.beforeExternalSideEffect?.()
      }
      await input.onCheckpoint?.('branch-created')
      await input.onCheckpoint?.('path-created')
      if (input.setupPolicy.length > 0) await input.onSetupStarted?.()
      const setupResult = setupCheckpoints(this.get(input.id)?.setupResult ?? [])
      for (const step of input.setupPolicy) {
        const idempotencyKey = step.idempotencyKey!
        if (setupResult.some((entry) =>
          entry.idempotencyKey === idempotencyKey && entry.status === 'succeeded'
        )) continue
        replaceCheckpoint(setupResult, {
          idempotencyKey, command: step.command, status: 'running', output: ''
        })
        input.beforeExternalSideEffect?.()
        this.#persistSetupCheckpoints(
          derivedCommand(command, `setup:${idempotencyKey}:running`),
          input.id,
          setupResult,
          input.now
        )
        try {
          input.beforeExternalSideEffect?.()
          const result = await exec(step.command, step.args, { cwd: input.path })
          input.beforeExternalSideEffect?.()
          replaceCheckpoint(setupResult, {
            idempotencyKey,
            command: step.command,
            status: 'succeeded',
            output: `${result.stdout}${result.stderr}`.slice(-8192)
          })
          input.beforeExternalSideEffect?.()
          this.#persistSetupCheckpoints(
            derivedCommand(command, `setup:${idempotencyKey}:succeeded`),
            input.id,
            setupResult,
            input.now
          )
        } catch (error) {
          replaceCheckpoint(setupResult, {
            idempotencyKey,
            command: step.command,
            status: 'failed',
            output: errorMessage(error).slice(-8192)
          })
          input.beforeExternalSideEffect?.()
          this.#persistSetupCheckpoints(
            derivedCommand(command, `setup:${idempotencyKey}:failed`),
            input.id,
            setupResult,
            input.now
          )
          throw error
        }
      }
      await input.onCheckpoint?.('setup-completed')
      input.beforeExternalSideEffect?.()

      return this.#transactions.execute(derivedCommand(command, 'ready'), ({ tx, emit }) => {
        tx.run(
          `UPDATE worktrees
           SET state = 'ready', base_revision = ?, setup_result_json = ?, updated_at = ?
           WHERE id = ?`,
          baseRevision,
          JSON.stringify(setupResult),
          input.now,
          input.id
        )
        const ready = requireWorktreeRow(tx.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', input.id))
        emitWorktree(emit, `${command.commandId}:ready`, 'worktree.ready', ready, input.workspaceId, input.now)
        return mapWorktree(ready)
      }).result
    } catch (error) {
      input.beforeExternalSideEffect?.()
      this.#transactions.execute(derivedCommand(command, 'failed'), ({ tx, emit }) => {
        tx.run("UPDATE worktrees SET state = 'failed', updated_at = ? WHERE id = ?", input.now, input.id)
        const failed = requireWorktreeRow(tx.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', input.id))
        emitWorktree(emit, `${command.commandId}:failed`, 'worktree.creation-failed', failed, input.workspaceId, input.now, { error: errorMessage(error) })
        return null
      })
      throw error
    }
  }

  #persistSetupCheckpoints(
    command: DomainCommandMetadata,
    worktreeId: string,
    checkpoints: WorktreeSetupCheckpoint[],
    now: number
  ): void {
    this.#transactions.execute(command, ({ tx }) => {
      tx.run(
        `UPDATE worktrees SET setup_result_json = ?, updated_at = ? WHERE id = ?`,
        JSON.stringify(checkpoints), now, worktreeId
      )
      return null
    })
  }

  async remove(command: DomainCommandMetadata, worktreeId: string, now: number): Promise<Worktree> {
    const worktree = this.get(worktreeId)
    if (!worktree) throw new Error(`Worktree ${worktreeId} does not exist`)
    if (worktree.state === 'removed') return worktree
    try {
      if (!(await pathExists(worktree.path))) {
        await exec('git', ['-C', worktree.repositoryRoot, 'worktree', 'prune'])
        return this.#completeRemoval(command, worktreeId, worktree.executionContextId, now)
      }
      const status = await exec('git', ['-C', worktree.path, 'status', '--porcelain'])
      if (status.stdout.trim() !== '') {
        return this.#transactions.execute(command, ({ tx, emit }) => {
          tx.run(
            "UPDATE worktrees SET state = 'retained', retained_at = ?, updated_at = ? WHERE id = ?",
            now,
            now,
            worktreeId
          )
          const retained = requireWorktreeRow(tx.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', worktreeId))
          emitWorktree(emit, command.commandId, 'worktree.retained-dirty', retained, undefined, now)
          return mapWorktree(retained)
        }).result
      }

      const runIds = this.#database
        .all<{ id: string }>(
          `SELECT session_runs.id
           FROM session_runs
           JOIN sessions ON sessions.id = session_runs.session_id
           WHERE sessions.execution_context_id = ?
             AND session_runs.status IN ('starting', 'running')`,
          worktree.executionContextId
        )
        .map(({ id }) => id)
      await this.#stopRuns(runIds)
      this.#transactions.execute(command, ({ tx, emit }) => {
        tx.run("UPDATE worktrees SET state = 'removing', updated_at = ? WHERE id = ?", now, worktreeId)
        const removing = requireWorktreeRow(tx.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', worktreeId))
        emitWorktree(emit, command.commandId, 'worktree.removal-started', removing, undefined, now)
        return null
      })
      await exec('git', ['-C', worktree.repositoryRoot, 'worktree', 'remove', worktree.path])
      return this.#completeRemoval(command, worktreeId, worktree.executionContextId, now)
    } catch (error) {
      this.#transactions.execute(derivedCommand(command, 'remove-failed'), ({ tx, emit }) => {
        tx.run("UPDATE worktrees SET state = 'failed', updated_at = ? WHERE id = ?", now, worktreeId)
        const sessions = tx.all<{ session_id: string }>(
          `SELECT session_id FROM session_environment_bindings
           WHERE managed_worktree_id = ? AND active_target = 'worktree'`,
          worktreeId
        )
        for (const { session_id: sessionId } of sessions) {
          this.#environments.markFailed(sessionId, `worktree-removal-failed:${errorMessage(error)}`, now, tx)
        }
        const failed = requireWorktreeRow(tx.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', worktreeId))
        emitWorktree(emit, `${command.commandId}:remove-failed`, 'worktree.removal-failed', failed, undefined, now, { error: errorMessage(error) })
        return null
      })
      throw error
    }
  }

  get(id: string): Worktree | undefined {
    const row = this.#database.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', id)
    return row ? mapWorktree(row) : undefined
  }

  async registerExisting(
    command: DomainCommandMetadata,
    input: {
      id: string
      executionContextId: string
      workspaceId: string
      repositoryRoot: string
      path: string
      branch: string
      now: number
    }
  ): Promise<Worktree> {
    const existing = this.#database.get<WorktreeRow>(
      `SELECT * FROM worktrees WHERE worktree_path = ? AND state <> 'removed'`, input.path
    )
    if (existing) return mapWorktree(existing)
    if (!(await pathIsGitWorktree(input.path))) {
      throw new Error(`Worktree path does not exist: ${input.path}`)
    }
    const baseRevision = (await exec('git', ['-C', input.path, 'rev-parse', 'HEAD'])).stdout.trim()
    return this.#transactions.execute(command, ({ tx, emit }) => {
      if (!tx.get('SELECT id FROM workspaces WHERE id = ? AND archived_at IS NULL', input.workspaceId)) {
        throw new Error(`Workspace ${input.workspaceId} does not exist`)
      }
      tx.run(
        `INSERT INTO execution_contexts (
           id, workspace_id, kind, cwd, created_at
         ) VALUES (?, ?, 'git-worktree', ?, ?)`,
        input.executionContextId, input.workspaceId, input.path, input.now
      )
      tx.run(
        `INSERT INTO worktrees (
           id, execution_context_id, repository_root, worktree_path, branch_name,
           base_ref, base_revision, state, setup_policy_json, setup_result_json,
           cleanup_policy, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, 'ready', '[]', '[]', 'retain-dirty', ?, ?)`,
        input.id, input.executionContextId, input.repositoryRoot, input.path, input.branch,
        baseRevision, baseRevision, input.now, input.now
      )
      const registered = requireWorktreeRow(tx.get<WorktreeRow>(
        'SELECT * FROM worktrees WHERE id = ?', input.id
      ))
      emitWorktree(emit, command.commandId, 'worktree.registered', registered, input.workspaceId, input.now)
      return mapWorktree(registered)
    }).result
  }

  getByPath(path: string): Worktree | undefined {
    const row = this.#database.get<WorktreeRow>(
      `SELECT * FROM worktrees WHERE worktree_path = ? AND state <> 'removed'`, path
    )
    return row ? mapWorktree(row) : undefined
  }

  #completeRemoval(
    command: DomainCommandMetadata,
    worktreeId: string,
    executionContextId: string,
    now: number
  ): Worktree {
    return this.#transactions.execute(derivedCommand(command, 'removed'), ({ tx, emit }) => {
      tx.run("UPDATE worktrees SET state = 'removed', updated_at = ? WHERE id = ?", now, worktreeId)
      tx.run('UPDATE execution_contexts SET archived_at = ? WHERE id = ?', now, executionContextId)
      const sessions = tx.all<{ session_id: string }>(
        `SELECT session_id FROM session_environment_bindings
         WHERE managed_worktree_id = ? AND active_target = 'worktree'`,
        worktreeId
      )
      for (const { session_id: sessionId } of sessions) {
        this.#environments.markMissing(sessionId, 'worktree:removed', now, tx)
      }
      const removed = requireWorktreeRow(tx.get<WorktreeRow>('SELECT * FROM worktrees WHERE id = ?', worktreeId))
      emitWorktree(emit, `${command.commandId}:removed`, 'worktree.removed', removed, undefined, now)
      return mapWorktree(removed)
    }).result
  }
}

function emitWorktree(
  emit: DomainMutationContext['emit'],
  eventId: string,
  eventType: string,
  row: WorktreeRow,
  workspaceId: string | undefined,
  occurredAt: number,
  extra: object = {}
): void {
  emit({
    eventId,
    eventType,
    aggregateType: 'worktree',
    aggregateId: row.id,
    ...(workspaceId === undefined ? {} : { workspaceId }),
    payload: { worktree: mapWorktree(row), ...extra },
    occurredAt
  })
}

function mapWorktree(row: WorktreeRow): Worktree {
  return {
    id: row.id,
    executionContextId: row.execution_context_id,
    repositoryRoot: row.repository_root,
    path: row.worktree_path,
    branch: row.branch_name,
    ...(row.base_revision === null ? {} : { baseRevision: row.base_revision }),
    state: row.state,
    setupPolicy: JSON.parse(row.setup_policy_json) as unknown[],
    setupResult: JSON.parse(row.setup_result_json) as unknown[],
    cleanupPolicy: row.cleanup_policy,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    ...(row.retained_at === null ? {} : { retainedAt: row.retained_at })
  }
}

function derivedCommand(command: DomainCommandMetadata, suffix: string): DomainCommandMetadata {
  return {
    commandId: `${command.commandId}:${suffix}`,
    commandType: `${command.commandType}.${suffix}`,
    requestHash: `${command.requestHash}:${suffix}`,
    ...(command.causationId === undefined ? {} : { causationId: command.causationId }),
    ...(command.correlationId === undefined ? {} : { correlationId: command.correlationId })
  }
}

async function pathIsGitWorktree(path: string): Promise<boolean> {
  try {
    await access(path)
    await exec('git', ['-C', path, 'rev-parse', '--is-inside-work-tree'])
    return true
  } catch {
    return false
  }
}

async function pathExists(path: string): Promise<boolean> {
  try {
    await access(path)
    return true
  } catch {
    return false
  }
}

async function localBranchExists(repository: string, branch: string): Promise<boolean> {
  try {
    await exec('git', [
      '-C', repository, 'show-ref', '--verify', '--quiet', `refs/heads/${branch}`
    ])
    return true
  } catch (error) {
    if (typeof error === 'object' && error !== null && 'code' in error && error.code === 1) {
      return false
    }
    throw error
  }
}

function requireWorktreeRow(row: WorktreeRow | undefined): WorktreeRow {
  if (!row) throw new Error('Worktree row disappeared during operation')
  return row
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function validateSetupPolicy(policy: WorktreeSetupStep[]): void {
  const keys = new Set<string>()
  for (const step of policy) {
    const key = step.idempotencyKey?.trim()
    if (!key) throw new Error(`Worktree setup command ${step.command} requires idempotencyKey`)
    if (keys.has(key)) throw new Error(`Duplicate Worktree setup idempotencyKey: ${key}`)
    keys.add(key)
  }
}

function setupCheckpoints(value: unknown[]): WorktreeSetupCheckpoint[] {
  return value.filter((entry): entry is WorktreeSetupCheckpoint => {
    if (typeof entry !== 'object' || entry === null) return false
    const candidate = entry as Partial<WorktreeSetupCheckpoint>
    return typeof candidate.idempotencyKey === 'string' &&
      typeof candidate.command === 'string' &&
      (candidate.status === 'running' || candidate.status === 'succeeded' || candidate.status === 'failed') &&
      typeof candidate.output === 'string'
  })
}

function replaceCheckpoint(
  checkpoints: WorktreeSetupCheckpoint[],
  next: WorktreeSetupCheckpoint
): void {
  const index = checkpoints.findIndex(({ idempotencyKey }) => idempotencyKey === next.idempotencyKey)
  if (index === -1) checkpoints.push(next)
  else checkpoints[index] = next
}
