import { randomUUID } from 'node:crypto'
import { access, constants, stat } from 'node:fs/promises'

import type { WorkspacePathReason, WorkspacePathState } from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

export const WORKSPACE_PATH_INVALID_MESSAGE =
  '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'

export class WorkspacePathInvalidError extends Error {
  readonly code = 'WORKSPACE_PATH_INVALID' as const
  readonly workspaceId: string

  constructor(workspaceId: string) {
    super(WORKSPACE_PATH_INVALID_MESSAGE)
    this.name = 'WorkspacePathInvalidError'
    this.workspaceId = workspaceId
  }
}

interface PathStateRow {
  workspace_id: string
  status: 'valid' | 'invalid'
  reason: WorkspacePathReason
  checked_at: number
  validation_generation: number
}

export class WorkspacePathService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager
  readonly #generations = new Map<string, number>()
  readonly #pollIntervalMs: number
  #pollTimer: NodeJS.Timeout | undefined

  constructor(
    database: RuntimeDatabase,
    transactions: DomainTransactionManager,
    pollIntervalMs = 30_000
  ) {
    this.#database = database
    this.#transactions = transactions
    this.#pollIntervalMs = pollIntervalMs
  }

  async validateWorkspace(workspaceId: string): Promise<WorkspacePathState> {
    const workspace = this.#database.get<{ root_directory: string }>(
      'SELECT root_directory FROM workspaces WHERE id = ? AND archived_at IS NULL',
      workspaceId
    )
    if (!workspace) throw new Error(`Workspace ${workspaceId} does not exist`)
    const storedGeneration = this.#database.get<{ validation_generation: number }>(
      'SELECT validation_generation FROM workspace_path_state WHERE workspace_id = ?',
      workspaceId
    )?.validation_generation ?? 0
    const generation = Math.max(
      storedGeneration,
      this.#generations.get(workspaceId) ?? 0
    ) + 1
    this.#generations.set(workspaceId, generation)

    const derived = await inspectWorkspacePath(workspace.root_directory)
    const state: WorkspacePathState = {
      workspaceId,
      status: derived === '' ? 'valid' : 'invalid',
      reason: derived,
      checkedAt: Date.now(),
      validationGeneration: generation
    }

    if (this.#generations.get(workspaceId) !== generation) {
      return this.getState(workspaceId) ?? state
    }

    const previous = this.getState(workspaceId)
    return this.#transactions.execute(
      {
        commandId: `workspace-path-${randomUUID()}`,
        commandType: 'workspace.validate-path',
        requestHash: `${workspaceId}:${generation}:${state.status}:${state.reason}`
      },
      ({ tx, emit }) => {
        tx.run(
          `INSERT INTO workspace_path_state (
             workspace_id, status, reason, checked_at, validation_generation
           ) VALUES (?, ?, ?, ?, ?)
           ON CONFLICT(workspace_id) DO UPDATE SET
             status = excluded.status,
             reason = excluded.reason,
             checked_at = excluded.checked_at,
             validation_generation = excluded.validation_generation
           WHERE workspace_path_state.validation_generation < excluded.validation_generation`,
          workspaceId,
          state.status,
          state.reason,
          state.checkedAt,
          state.validationGeneration
        )
        if (previous?.status !== state.status || previous?.reason !== state.reason) {
          emit({
            eventId: `workspace-path:${workspaceId}:${generation}`,
            eventType: 'workspace.path-status-changed',
            aggregateType: 'workspace',
            aggregateId: workspaceId,
            workspaceId,
            payload: state,
            occurredAt: state.checkedAt
          })
        }
        return state
      }
    ).result
  }

  async validateBeforeExecution(workspaceId: string): Promise<WorkspacePathState> {
    const state = await this.validateWorkspace(workspaceId)
    if (state.status === 'invalid') throw new WorkspacePathInvalidError(workspaceId)
    return state
  }

  async validateExecutionContextBeforeExecution(executionContextId: string): Promise<void> {
    const context = this.#database.get<{ workspace_id: string }>(
      `SELECT workspace_id FROM execution_contexts
       WHERE id = ? AND archived_at IS NULL`,
      executionContextId
    )
    if (!context) return
    await this.validateBeforeExecution(context.workspace_id)
  }

  async assertSessionInputAllowed(sessionId: string): Promise<void> {
    const authority = this.#database.get<{
      workspace_id: string
      status: 'valid' | 'invalid' | null
    }>(
      `SELECT tasks.workspace_id, workspace_path_state.status
       FROM sessions
       JOIN tasks ON tasks.id = sessions.task_id
       LEFT JOIN workspace_path_state
         ON workspace_path_state.workspace_id = tasks.workspace_id
       WHERE sessions.id = ? AND sessions.archived_at IS NULL`,
      sessionId
    )
    if (!authority) return
    if (authority.status === null) {
      await this.validateBeforeExecution(authority.workspace_id)
      return
    }
    if (authority.status === 'invalid') {
      throw new WorkspacePathInvalidError(authority.workspace_id)
    }
  }

  getState(workspaceId: string): WorkspacePathState | undefined {
    const row = this.#database.get<PathStateRow>(
      'SELECT * FROM workspace_path_state WHERE workspace_id = ?',
      workspaceId
    )
    return row === undefined ? undefined : mapState(row)
  }

  startPolling(): void {
    if (this.#pollTimer !== undefined) return
    this.#pollTimer = setInterval(() => {
      void this.#poll().catch((error) => {
        console.error(`[workspace-path-poll] ${errorMessage(error)}`)
      })
    }, this.#pollIntervalMs)
    this.#pollTimer.unref()
  }

  stopPolling(): void {
    if (this.#pollTimer === undefined) return
    clearInterval(this.#pollTimer)
    this.#pollTimer = undefined
  }

  async #poll(): Promise<void> {
    const workspaces = this.#database.all<{ workspace_id: string }>(
      `SELECT DISTINCT workspace_id FROM (
         SELECT active_workspace_id AS workspace_id
         FROM window_navigation WHERE active_workspace_id IS NOT NULL
         UNION
         SELECT workspace_id FROM workspace_path_state WHERE status = 'invalid'
       ) ORDER BY workspace_id`
    )
    await Promise.all(workspaces.map(async ({ workspace_id }) => {
      try {
        await this.validateWorkspace(workspace_id)
      } catch (error) {
        console.error(`[workspace-path:${workspace_id}] ${errorMessage(error)}`)
      }
    }))
  }
}

async function inspectWorkspacePath(rootDirectory: string): Promise<WorkspacePathReason> {
  let metadata
  try {
    metadata = await stat(rootDirectory)
  } catch (error) {
    return isCode(error, 'ENOENT') ? 'missing' : 'unknown'
  }
  if (!metadata.isDirectory()) return 'not-directory'
  try {
    await access(rootDirectory, constants.R_OK | constants.X_OK)
  } catch {
    return 'no-access'
  }
  return ''
}

function mapState(row: PathStateRow): WorkspacePathState {
  return {
    workspaceId: row.workspace_id,
    status: row.status,
    reason: row.reason,
    checkedAt: row.checked_at,
    validationGeneration: row.validation_generation
  }
}

function isCode(error: unknown, code: string): boolean {
  return error instanceof Error && 'code' in error && error.code === code
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
