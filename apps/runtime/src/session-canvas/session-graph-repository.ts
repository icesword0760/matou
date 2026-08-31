import type {
  DomainCommandMetadata,
  DomainCommit,
  ProviderRestoreState,
  SceneSessionGraph,
  SessionCanvasMembership,
  SessionGraphEdge,
  SessionGraphNode,
  SessionKind,
  SessionStatus,
  SessionWorkStatus
} from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

interface MembershipRow {
  session_id: string
  scene_id: string
  sibling_created_seq: number
  last_user_interaction_seq: number
  created_at: number
  updated_at: number
}

interface GraphRow extends MembershipRow {
  task_id: string
  execution_context_id: string
  kind: SessionKind
  status: SessionStatus
  work_status: SessionWorkStatus
  title: string
  cwd: string
  session_created_at: number
  session_updated_at: number
  last_activity_at: number
  archived_at: number | null
  parent_session_id: string | null
  relation_kind: 'derived-from' | 'forked-from' | null
  relation_created_at: number | null
  restore_state: ProviderRestoreState | null
  restore_error: string | null
  provider_metadata_json: string | null
  fork_state: 'pending' | 'starting' | 'succeeded' | 'failed' | null
  fork_error: string | null
  fork_attempt: number | null
  worktree_path: string | null
  branch_name: string | null
  detached_window_id: string | null
  latest_lines_json: string | null
}

interface EdgeRow {
  parent_session_id: string
  child_session_id: string
  relation_kind: 'derived-from' | 'forked-from'
  created_at: number
}

interface ChildCountRow {
  parent_session_id: string
  active_count: number
  stopped_count: number
  shell_count: number
  claude_count: number
}

export class SessionGraphRepository {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  getMembership(sessionId: string): SessionCanvasMembership | undefined {
    const row = this.#database.get<MembershipRow>(
      'SELECT * FROM session_canvas_memberships WHERE session_id = ?',
      sessionId
    )
    return row ? mapMembership(row) : undefined
  }

  createMembership(
    command: DomainCommandMetadata,
    input: {
      sessionId: string
      sceneId: string
      siblingCreatedSeq: number
      lastUserInteractionSeq: number
      now: number
    }
  ): DomainCommit<SessionCanvasMembership> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const ownership = tx.get<{ session_task_id: string; scene_task_id: string }>(
        `SELECT sessions.task_id AS session_task_id, scenes.task_id AS scene_task_id
         FROM sessions CROSS JOIN scenes
         WHERE sessions.id = ? AND scenes.id = ?`,
        input.sessionId,
        input.sceneId
      )
      if (!ownership || ownership.session_task_id !== ownership.scene_task_id) {
        throw new Error('canvas membership Session and Scene must belong to the same Task')
      }
      tx.run(
        `INSERT INTO session_canvas_memberships (
           session_id, scene_id, sibling_created_seq, last_user_interaction_seq,
           created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?)`,
        input.sessionId,
        input.sceneId,
        input.siblingCreatedSeq,
        input.lastUserInteractionSeq,
        input.now,
        input.now
      )
      tx.run(
        `UPDATE runtime_sequences SET value = MAX(value, ?)
         WHERE name = 'session-sibling-created'`,
        input.siblingCreatedSeq
      )
      const membership = mapMembership(requireRow(tx.get<MembershipRow>(
        'SELECT * FROM session_canvas_memberships WHERE session_id = ?',
        input.sessionId
      ), 'SessionCanvasMembership'))
      emit({
        eventId: `${command.commandId}:canvas-membership-created`,
        eventType: 'session.canvas-membership-created',
        aggregateType: 'session',
        aggregateId: input.sessionId,
        taskId: ownership.session_task_id,
        sessionId: input.sessionId,
        payload: { membership },
        occurredAt: input.now
      })
      return membership
    })
  }

  nextSequence(name: 'session-sibling-created' | 'session-user-interaction'): number {
    return this.#database.transaction((tx) => {
      tx.run('UPDATE runtime_sequences SET value = value + 1 WHERE name = ?', name)
      return requireRow(tx.get<{ value: number }>(
        'SELECT value FROM runtime_sequences WHERE name = ?',
        name
      ), `runtime sequence ${name}`).value
    })
  }

  projectSceneGraph(sceneId: string, windowId?: string): SceneSessionGraph {
    return projectSceneGraphFrom(this.#database, sceneId, windowId)
  }
}

export function projectSceneGraphFrom(
  source: DatabaseTransaction,
  sceneId: string,
  windowId?: string
): SceneSessionGraph {
    const scene = source.get<{ id: string; layout_revision: number }>(
      'SELECT id, layout_revision FROM scenes WHERE id = ?',
      sceneId
    )
    if (!scene) throw new Error(`Scene ${sceneId} does not exist`)

    const rows = source.all<GraphRow>(
      `SELECT
         membership.*,
         sessions.task_id,
         sessions.execution_context_id,
         sessions.kind,
         sessions.status,
         sessions.work_status,
         sessions.title,
         sessions.cwd,
         sessions.created_at AS session_created_at,
         sessions.updated_at AS session_updated_at,
         sessions.last_activity_at,
         sessions.archived_at,
         structural.to_session_id AS parent_session_id,
         structural.relation_kind,
         structural.created_at AS relation_created_at,
         provider.restore_state,
         provider.restore_error,
         provider.metadata_json AS provider_metadata_json,
         fork.state AS fork_state,
         fork.error_message AS fork_error,
         fork.attempt_count AS fork_attempt,
         worktrees.worktree_path,
         worktrees.branch_name,
         detached.native_window_key AS detached_window_id
         , summary.latest_lines_json
       FROM session_canvas_memberships AS membership
       JOIN sessions ON sessions.id = membership.session_id
       LEFT JOIN session_relations_current AS structural
         ON structural.from_session_id = sessions.id
        AND structural.relation_kind IN ('derived-from', 'forked-from')
       LEFT JOIN provider_bindings AS provider
         ON provider.id = (
           SELECT binding.id FROM provider_bindings AS binding
           WHERE binding.session_id = sessions.id
           ORDER BY binding.updated_at DESC, binding.id DESC LIMIT 1
         )
       LEFT JOIN session_fork_intents AS fork ON fork.session_id = sessions.id
       LEFT JOIN worktrees ON worktrees.execution_context_id = sessions.execution_context_id
       LEFT JOIN scene_windows AS detached
         ON detached.scene_id = membership.scene_id
        AND detached.state = 'detached'
        AND EXISTS (
          SELECT 1 FROM session_mounts
          WHERE session_mounts.scene_window_id = detached.id
            AND session_mounts.session_id = sessions.id
        )
       LEFT JOIN session_graph_summaries AS summary ON summary.session_id = sessions.id
       WHERE membership.scene_id = ?
       ORDER BY
         COALESCE(structural.to_session_id, ''),
         membership.last_user_interaction_seq DESC,
         membership.sibling_created_seq ASC,
         sessions.id`,
      sceneId
    )

    const childCounts = new Map(
      source.all<ChildCountRow>(
        `SELECT
           relation.to_session_id AS parent_session_id,
           SUM(CASE WHEN child.archived_at IS NULL THEN 1 ELSE 0 END) AS active_count,
           SUM(CASE WHEN child.archived_at IS NOT NULL THEN 1 ELSE 0 END) AS stopped_count,
           SUM(CASE WHEN child.archived_at IS NULL AND child.kind = 'shell' THEN 1 ELSE 0 END) AS shell_count,
           SUM(CASE WHEN child.archived_at IS NULL
                     AND child.kind IN ('claude-code', 'agent-team-member')
                    THEN 1 ELSE 0 END) AS claude_count
         FROM session_relations_current AS relation
         JOIN sessions AS child ON child.id = relation.from_session_id
         JOIN session_canvas_memberships AS membership ON membership.session_id = child.id
         WHERE relation.relation_kind IN ('derived-from', 'forked-from')
           AND membership.scene_id = ?
         GROUP BY relation.to_session_id`,
        sceneId
      ).map((row) => [row.parent_session_id, row] as const)
    )

    const cwdUseCounts = new Map(
      source.all<{ cwd: string; count: number }>(
        `SELECT sessions.cwd, COUNT(*) AS count
         FROM sessions
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         WHERE membership.scene_id = ? AND sessions.archived_at IS NULL
         GROUP BY sessions.cwd`,
        sceneId
      ).map((row) => [row.cwd, row.count] as const)
    )

    const nodes: SessionGraphNode[] = rows.map((row) => {
      const counts = childCounts.get(row.session_id)
      return {
        sessionId: row.session_id,
        sceneId: row.scene_id,
        ...(row.parent_session_id === null ? {} : { parentSessionId: row.parent_session_id }),
        ...(row.relation_kind === null ? {} : { relationKind: row.relation_kind }),
        currentMode: row.kind,
        workStatus: row.fork_state === 'failed' || row.restore_state === 'failed'
          ? 'error'
          : row.fork_state === 'pending' || row.fork_state === 'starting'
            ? 'starting'
            : row.work_status,
        providerRestoreState: row.restore_state ?? 'none',
        ...(row.restore_error === null ? {} : { providerRestoreError: row.restore_error }),
        ...(row.fork_state === null ? {} : { forkState: row.fork_state }),
        ...(row.fork_error === null ? {} : { forkError: row.fork_error }),
        ...(row.fork_attempt === null ? {} : { forkAttempt: row.fork_attempt }),
        ...(providerSpawnRevision(row.provider_metadata_json) === undefined ? {} : {
          providerSpawnRevision: providerSpawnRevision(row.provider_metadata_json)!
        }),
        canFork: row.kind === 'claude-code' &&
          row.restore_state !== 'failed' &&
          providerCanFork(row.provider_metadata_json),
        title: row.title,
        cwd: row.cwd,
        sharedWorkingDirectory: (cwdUseCounts.get(row.cwd) ?? 0) > 1,
        ...(row.worktree_path === null || row.branch_name === null ? {} : {
          worktree: {
            branch: row.branch_name,
            path: row.worktree_path,
            shared: (cwdUseCounts.get(row.cwd) ?? 0) > 1
          }
        }),
        activeChildCount: counts?.active_count ?? 0,
        stoppedChildCount: counts?.stopped_count ?? 0,
        childModeCounts: {
          shell: counts?.shell_count ?? 0,
          claudeCode: counts?.claude_count ?? 0
        },
        latestLines: parseLatestLines(row.latest_lines_json),
        siblingCreatedSeq: row.sibling_created_seq,
        lastUserInteractionSeq: row.last_user_interaction_seq,
        lastActivityAt: row.last_activity_at,
        ...(row.archived_at === null ? {} : { archivedAt: row.archived_at }),
        ...(row.detached_window_id === null ? {} : { detachedWindowId: row.detached_window_id })
      }
    })

    const edges = source.all<EdgeRow>(
      `SELECT
         relation.to_session_id AS parent_session_id,
         relation.from_session_id AS child_session_id,
         relation.relation_kind,
         relation.created_at
       FROM session_relations_current AS relation
       JOIN session_canvas_memberships AS membership
         ON membership.session_id = relation.from_session_id
       WHERE membership.scene_id = ?
         AND relation.relation_kind IN ('derived-from', 'forked-from')
       ORDER BY relation.created_at, relation.relation_id`,
      sceneId
    ).map(mapEdge)

    const focusedSessionId = windowId === undefined ? undefined : source.get<{ active_session_id: string | null }>(
      `SELECT active_session_id FROM window_scene_focus
       WHERE window_id = ? AND scene_id = ?`,
      windowId,
      sceneId
    )?.active_session_id ?? undefined

    return {
      sceneId,
      layoutRevision: scene.layout_revision,
      nodes,
      edges,
      ...(focusedSessionId === undefined ? {} : { focusedSessionId })
    }
}

function mapMembership(row: MembershipRow): SessionCanvasMembership {
  return {
    sessionId: row.session_id,
    sceneId: row.scene_id,
    siblingCreatedSeq: row.sibling_created_seq,
    lastUserInteractionSeq: row.last_user_interaction_seq,
    createdAt: row.created_at,
    updatedAt: row.updated_at
  }
}

function mapEdge(row: EdgeRow): SessionGraphEdge {
  return {
    parentSessionId: row.parent_session_id,
    childSessionId: row.child_session_id,
    relationKind: row.relation_kind,
    createdAt: row.created_at
  }
}

function providerCanFork(metadataJson: string | null): boolean {
  if (metadataJson === null) return false
  try {
    const metadata = JSON.parse(metadataJson) as unknown
    return typeof metadata === 'object' && metadata !== null &&
      !Array.isArray(metadata) &&
      (metadata as Record<string, unknown>).canFork === true
  } catch {
    return false
  }
}

function providerSpawnRevision(metadataJson: string | null): number | undefined {
  if (metadataJson === null) return undefined
  try {
    const metadata = JSON.parse(metadataJson) as unknown
    if (typeof metadata !== 'object' || metadata === null || Array.isArray(metadata)) return undefined
    const value = (metadata as Record<string, unknown>).spawnRevision
    return typeof value === 'number' && Number.isSafeInteger(value) ? value : undefined
  } catch {
    return undefined
  }
}

function parseLatestLines(value: string | null): string[] {
  if (value === null) return []
  try {
    const parsed = JSON.parse(value) as unknown
    return Array.isArray(parsed)
      ? parsed.filter((line): line is string => typeof line === 'string').slice(-4)
      : []
  } catch {
    return []
  }
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (row === undefined) throw new Error(`${label} does not exist`)
  return row
}
