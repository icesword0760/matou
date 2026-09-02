import { readdir } from 'node:fs/promises'
import { join } from 'node:path'

import { CheckpointManager } from '../checkpoints/checkpoint-manager'
import { JournalCorruptionError, SegmentJournal } from '../journal/segment-journal'
import type { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { JournalEventCoordinator } from './journal-event-alignment'
import { SessionRepository } from '../domain/session-repository'
import type { RecoveryJob, RecoveryPriority } from './runtime-session-recovery-scheduler'

export interface RecoveredSession {
  sessionId: string
  terminalSequence: number
  domainEventSequence: number
  repairedAlignment: boolean
  removedCheckpointOrphans: number
}

export interface FailedSessionRecovery {
  sessionId: string
  code: 'JOURNAL_CORRUPT' | 'RECOVERY_FAILED'
  message: string
}

export interface RuntimeRecoveryReport {
  interruptedRuns: string[]
  recovered: RecoveredSession[]
  failed: FailedSessionRecovery[]
}

/**
 * Reconciles durable terminal and domain state before the Runtime accepts clients.
 * Every Session is an independent recovery unit so one damaged journal never
 * prevents other workspaces from opening.
 */
export class RuntimeRecoveryService {
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase

  constructor(dataRoot: string, database: RuntimeDatabase) {
    this.#dataRoot = dataRoot
    this.#database = database
  }

  async recoverAll(): Promise<RuntimeRecoveryReport> {
    const report: RuntimeRecoveryReport = { interruptedRuns: [], recovered: [], failed: [] }
    const transactions = new DomainTransactionManager(this.#database)
    const sessions = new SessionRepository(this.#database, transactions)
    for (const run of this.#database.all<{ id: string; session_id: string; profile: string }>(
      `SELECT id, session_id, profile FROM session_runs
       WHERE status IN ('starting', 'running') AND runtime_generation <> ?
       ORDER BY started_at`,
      this.#database.runtimeGeneration
    )) {
      sessions.interruptRun(
        {
          commandId: `runtime-recovery-interrupt-${run.id}`,
          commandType: 'session.run-interrupt',
          requestHash: `interrupt:${run.id}`
        },
        run.id,
        Date.now()
      )
      report.interruptedRuns.push(run.id)
    }
    const persistedSessions = new Set(
      this.#database.all<{ id: string }>('SELECT id FROM sessions').map(({ id }) => id)
    )
    const journalRoot = join(this.#dataRoot, 'journal')
    let entries: string[]
    try {
      entries = await readdir(journalRoot)
    } catch (error) {
      if (isMissingFile(error)) return report
      throw error
    }

    const coordinator = new JournalEventCoordinator(
      this.#database,
      transactions
    )
    const checkpoints = new CheckpointManager(this.#dataRoot, this.#database)
    for (const sessionId of entries.sort()) {
      if (!persistedSessions.has(sessionId)) continue
      let journal: SegmentJournal | undefined
      try {
        journal = await SegmentJournal.open(this.#dataRoot, sessionId)
        const watermark = await coordinator.recover(sessionId, journal)
        const removedCheckpointOrphans = await checkpoints.removeOrphans(sessionId)
        report.recovered.push({
          sessionId,
          terminalSequence: watermark.terminalSequence,
          domainEventSequence: watermark.domainEventSequence,
          repairedAlignment: watermark.repaired,
          removedCheckpointOrphans
        })
      } catch (error) {
        report.failed.push({
          sessionId,
          code: error instanceof JournalCorruptionError ? 'JOURNAL_CORRUPT' : 'RECOVERY_FAILED',
          message: errorMessage(error)
        })
      } finally {
        await journal?.close().catch(() => undefined)
      }
    }
    return report
  }

  planSessionRecovery(): RecoveryJob[] {
    const focus = this.#database.get<{
      active_workspace_id: string | null
      active_task_id: string | null
      active_scene_id: string | null
      active_session_id: string | null
    }>(
      `SELECT navigation.active_workspace_id,
              workspace_focus.active_task_id,
              task_focus.active_scene_id,
              scene_focus.active_session_id
       FROM app_windows window
       JOIN window_navigation navigation ON navigation.window_id = window.id
       LEFT JOIN window_workspace_focus workspace_focus
         ON workspace_focus.window_id = window.id
        AND workspace_focus.workspace_id = navigation.active_workspace_id
       LEFT JOIN window_task_focus task_focus
         ON task_focus.window_id = window.id
        AND task_focus.task_id = workspace_focus.active_task_id
       LEFT JOIN window_scene_focus scene_focus
         ON scene_focus.window_id = window.id
        AND scene_focus.scene_id = task_focus.active_scene_id
       WHERE window.kind = 'main' AND window.state <> 'closed'
       ORDER BY (window.state = 'visible') DESC, window.updated_at DESC, window.id
       LIMIT 1`
    )
    const rows = this.#database.all<{
      session_id: string
      scene_id: string
      task_id: string
      workspace_id: string
      execution_context_id: string
      profile: 'shell' | 'claude-code' | 'codex' | 'agent-team-member'
      mount_created_at: number
      last_activity_at: number
    }>(
      `SELECT sessions.id AS session_id,
              scenes.id AS scene_id,
              tasks.id AS task_id,
              tasks.workspace_id,
              sessions.execution_context_id,
              sessions.kind AS profile,
              session_mounts.created_at AS mount_created_at,
              sessions.last_activity_at
       FROM sessions
       JOIN tasks ON tasks.id = sessions.task_id AND tasks.archived_at IS NULL
       JOIN workspaces ON workspaces.id = tasks.workspace_id AND workspaces.archived_at IS NULL
       JOIN session_mounts ON session_mounts.session_id = sessions.id
       JOIN scenes ON scenes.id = session_mounts.scene_id AND scenes.archived_at IS NULL
       WHERE sessions.archived_at IS NULL
         AND sessions.work_status IN ('starting', 'running', 'needs-input', 'interrupted')
       ORDER BY sessions.last_activity_at DESC, session_mounts.created_at, sessions.id`
    )

    const selected = new Map<string, RecoveryJob & { mountCreatedAt: number }>()
    for (const row of rows) {
      if (row.profile === 'agent-team-member') continue
      const priority = recoveryPriority(row, focus)
      const candidate: RecoveryJob & { mountCreatedAt: number } = {
        sessionId: row.session_id,
        sceneId: row.scene_id,
        taskId: row.task_id,
        workspaceId: row.workspace_id,
        executionContextId: row.execution_context_id,
        profile: row.profile,
        priority,
        enqueueSequence: 0,
        mountCreatedAt: row.mount_created_at
      }
      const current = selected.get(row.session_id)
      if (!current || recoveryPriorityRank(candidate.priority) < recoveryPriorityRank(current.priority)) {
        selected.set(row.session_id, candidate)
      }
    }

    return [...selected.values()]
      .sort((left, right) =>
        recoveryPriorityRank(left.priority) - recoveryPriorityRank(right.priority) ||
        left.mountCreatedAt - right.mountCreatedAt ||
        left.sessionId.localeCompare(right.sessionId)
      )
      .map(({ mountCreatedAt: _mountCreatedAt, ...job }, index) => ({
        ...job,
        enqueueSequence: index + 1
      }))
  }
}

function recoveryPriority(
  row: { session_id: string; scene_id: string; task_id: string; workspace_id: string },
  focus: {
    active_workspace_id: string | null
    active_task_id: string | null
    active_scene_id: string | null
    active_session_id: string | null
  } | undefined
): RecoveryPriority {
  if (row.session_id === focus?.active_session_id) return 'active-session'
  if (row.scene_id === focus?.active_scene_id) return 'foreground-scene'
  if (row.task_id === focus?.active_task_id) return 'active-task'
  if (row.workspace_id === focus?.active_workspace_id) return 'active-workspace'
  return 'background'
}

function recoveryPriorityRank(priority: RecoveryPriority): number {
  return {
    'active-session': 0,
    'foreground-scene': 1,
    'active-task': 2,
    'active-workspace': 3,
    background: 4
  }[priority]
}

function isMissingFile(error: unknown): boolean {
  return error instanceof Error && 'code' in error && error.code === 'ENOENT'
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
