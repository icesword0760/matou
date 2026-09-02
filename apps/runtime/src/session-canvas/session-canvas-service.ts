import { randomUUID } from 'node:crypto'

import type {
  DomainCommandMetadata,
  SceneSessionGraph,
  SessionCanvasMembership
} from '@matou/domain'
import type { RemoveNodeScope } from '@matou/contracts'

import {
  activateSessionInTransaction,
  assertWorkspacePathAvailable,
  readHierarchyResult,
  registerWindow,
  type WorkspaceHierarchyResult
} from '../hierarchy/hierarchy-application-service'
import { createHierarchyIds } from '../hierarchy/hierarchy-ids'
import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'
import { projectSceneGraphFrom } from './session-graph-repository'

interface TaskRow {
  id: string
  workspace_id: string
  execution_context_id: string
}

interface WorkspaceRow {
  id: string
  root_directory: string
}

interface SceneRow {
  id: string
  task_id: string
}

interface SourceSessionRow {
  id: string
  task_id: string
  execution_context_id: string
  cwd: string
}

interface SourceMountRow {
  id: string
  scene_node_id: string | null
  scene_window_id: string | null
}

interface SceneNodeRow {
  id: string
  parent_node_id: string | null
  ordinal: number
}

interface MembershipRow {
  session_id: string
  scene_id: string
  sibling_created_seq: number
  last_user_interaction_seq: number
  created_at: number
  updated_at: number
}

export interface CreateCanvasInput {
  windowId: string
  taskId: string
  now: number
}

export interface CreateShellSiblingInput {
  windowId: string
  sceneId: string
  sourceSessionId: string
  parentSessionId?: string
  executionContextId?: string
  now: number
}

export interface SetFocusedSessionInput {
  windowId: string
  sceneId: string
  sessionId: string
  now: number
}

export interface RestartStoppedSessionInput {
  windowId: string
  sessionId: string
  now: number
}

export interface RemoveSessionBranchInput {
  windowId: string
  sceneId: string
  sessionId: string
  scope: RemoveNodeScope
  now: number
}

export interface RemoveSessionBranchResult {
  graph: SceneSessionGraph
  removedSessionIds: string[]
  disposedSessionIds: string[]
}

export interface UpsertAgentTeamMemberInput {
  leadSessionId: string
  teammateId: string
  teamId: string
  name: string
  workStatus: 'running' | 'idle' | 'needs-input' | 'error'
  latestLines: string[]
  now: number
}

export interface UpsertAgentTeamMemberResult {
  sessionId: string
  created: boolean
  graph: SceneSessionGraph
}

export interface SessionCanvasMutationResult extends WorkspaceHierarchyResult {
  graph: SceneSessionGraph
}

export class SessionCanvasService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  createCanvas(
    command: DomainCommandMetadata,
    input: CreateCanvasInput
  ): SessionCanvasMutationResult {
    const ids = createHierarchyIds()
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const task = requireRow(tx.get<TaskRow>(
        `SELECT id, workspace_id, execution_context_id FROM tasks
         WHERE id = ? AND archived_at IS NULL`,
        input.taskId
      ), 'Task')
      const workspace = requireRow(tx.get<WorkspaceRow>(
        `SELECT id, root_directory FROM workspaces
         WHERE id = ? AND archived_at IS NULL`,
        task.workspace_id
      ), 'Workspace')
      assertWorkspacePathAvailable(tx, workspace.id)

      const sceneName = nextCanvasName(tx, task.id)
      const sceneOrdinal = tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM scenes
         WHERE task_id = ? AND archived_at IS NULL`,
        task.id
      )?.count ?? 0
      tx.run(
        `INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, title_pinned, sort_key,
           layout_revision, created_at, updated_at
         ) VALUES (?, ?, ?, 'tile', ?, 0, ?, 1, ?, ?)`,
        ids.sceneId, task.id, sceneName, ids.rootNodeId,
        sortKey(sceneOrdinal), input.now, input.now
      )
      tx.run(
        `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
         VALUES (?, ?, 'root', 0, ?)`,
        ids.rootNodeId, ids.sceneId, input.now
      )
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, ?, 1)`,
        ids.sessionId, task.id, task.execution_context_id, workspace.root_directory,
        input.now, input.now, input.now
      )
      tx.run(
        `INSERT INTO session_mounts (
           id, scene_id, scene_node_id, session_id, created_at
         ) VALUES (?, ?, ?, ?, ?)`,
        ids.mountId, ids.sceneId, ids.rootNodeId, ids.sessionId, input.now
      )
      activateSessionInTransaction(tx, input.windowId, ids.sessionId, input.now)

      const hierarchy = readHierarchyResult(tx, input.windowId)
      const membership = readMembership(tx, ids.sessionId)
      const graph = projectSceneGraphFrom(tx, ids.sceneId, input.windowId)
      emit({
        eventId: `${command.commandId}:scene-created`,
        eventType: 'scene.created', aggregateType: 'scene', aggregateId: ids.sceneId,
        workspaceId: workspace.id, taskId: task.id,
        payload: hierarchy.scene, occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:session-created`,
        eventType: 'session.created', aggregateType: 'session', aggregateId: ids.sessionId,
        workspaceId: workspace.id, taskId: task.id, sessionId: ids.sessionId,
        payload: hierarchy.session, occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:session-mounted`,
        eventType: 'scene.session-mounted', aggregateType: 'scene', aggregateId: ids.sceneId,
        workspaceId: workspace.id, taskId: task.id, sessionId: ids.sessionId,
        payload: hierarchy.mount, occurredAt: input.now
      })
      emitMembership(command.commandId, membership, workspace.id, task.id, emit, input.now)
      emit({
        eventId: `${command.commandId}:canvas-created`,
        eventType: 'scene.canvas-created', aggregateType: 'scene', aggregateId: ids.sceneId,
        workspaceId: workspace.id, taskId: task.id, sessionId: ids.sessionId,
        payload: { graph }, occurredAt: input.now
      })
      return { ...hierarchy, graph }
    }).result
  }

  createShellSibling(
    command: DomainCommandMetadata,
    input: CreateShellSiblingInput
  ): SessionCanvasMutationResult {
    const ids = createHierarchyIds()
    const relationId = randomUUID()
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const scene = requireRow(tx.get<SceneRow>(
        `SELECT id, task_id FROM scenes
         WHERE id = ? AND archived_at IS NULL`,
        input.sceneId
      ), 'Scene')
      const task = requireRow(tx.get<TaskRow>(
        `SELECT id, workspace_id, execution_context_id FROM tasks
         WHERE id = ? AND archived_at IS NULL`,
        scene.task_id
      ), 'Task')
      assertWorkspacePathAvailable(tx, task.workspace_id)
      const source = requireRow(tx.get<SourceSessionRow>(
        `SELECT sessions.id, sessions.task_id, sessions.execution_context_id, sessions.cwd
         FROM sessions
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         WHERE sessions.id = ? AND membership.scene_id = ?
           AND (sessions.archived_at IS NULL OR ? IS NOT NULL)`,
        input.sourceSessionId, input.sceneId, input.parentSessionId ?? null
      ), 'Session')
      const executionContext = input.executionContextId
        ? requireRow(tx.get<{ id: string; cwd: string }>(
            `SELECT id, cwd FROM execution_contexts
             WHERE id = ? AND workspace_id = ? AND archived_at IS NULL`,
            input.executionContextId, task.workspace_id
          ), 'ExecutionContext')
        : { id: source.execution_context_id, cwd: source.cwd }
      const sourceMount = requireRow(tx.get<SourceMountRow>(
        `SELECT mounts.id, mounts.scene_node_id, mounts.scene_window_id
         FROM session_mounts AS mounts
         JOIN sessions ON sessions.id = mounts.session_id
         WHERE mounts.scene_id = ? AND mounts.scene_node_id IS NOT NULL
           AND mounts.scene_window_id IS NULL AND sessions.archived_at IS NULL
         ORDER BY
           CASE WHEN mounts.session_id = ? THEN 0
                WHEN mounts.session_id = ? THEN 1 ELSE 2 END,
           sessions.last_activity_at DESC, mounts.created_at, mounts.id LIMIT 1`,
        input.sceneId, input.sourceSessionId, input.parentSessionId ?? null
      ), 'SessionMount')
      if (sourceMount.scene_window_id !== null || sourceMount.scene_node_id === null) {
        throw new Error('独立窗口中的会话需要先回到原会话列表')
      }
      const sourceNode = requireRow(tx.get<SceneNodeRow>(
        `SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?`,
        sourceMount.scene_node_id
      ), 'SceneNode')
      const structuralParent = input.parentSessionId
        ? { parent_session_id: input.parentSessionId }
        : tx.get<{ parent_session_id: string }>(
        `SELECT to_session_id AS parent_session_id
         FROM session_relations_current
         WHERE from_session_id = ?
           AND relation_kind IN ('derived-from', 'forked-from')`,
        source.id
      )

      // Keep the legacy split tree renderable while the new horizontal graph UI becomes authoritative.
      tx.run(
        `INSERT INTO scene_nodes (
           id, scene_id, parent_node_id, kind, direction, ordinal, created_at
         ) VALUES (?, ?, ?, 'split', 'horizontal', ?, ?)`,
        ids.secondaryNodeId, scene.id, sourceNode.parent_node_id,
        sourceNode.ordinal, input.now
      )
      tx.run(
        `UPDATE scene_nodes
         SET parent_node_id = ?, kind = 'mount', direction = NULL, ordinal = 0
         WHERE id = ?`,
        ids.secondaryNodeId, sourceNode.id
      )
      tx.run(
        `INSERT INTO scene_nodes (
           id, scene_id, parent_node_id, kind, ordinal, created_at
         ) VALUES (?, ?, ?, 'mount', 1, ?)`,
        ids.rootNodeId, scene.id, ids.secondaryNodeId, input.now
      )
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, ?, 1)`,
        ids.sessionId, task.id, executionContext.id, executionContext.cwd,
        input.now, input.now, input.now
      )
      tx.run(
        `INSERT INTO session_mounts (
           id, scene_id, scene_node_id, session_id, created_at
         ) VALUES (?, ?, ?, ?, ?)`,
        ids.mountId, scene.id, ids.rootNodeId, ids.sessionId, input.now
      )
      tx.run(
        `UPDATE scenes
         SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
             layout_revision = layout_revision + 1, updated_at = ?
         WHERE id = ?`,
        sourceNode.id, ids.secondaryNodeId, input.now, scene.id
      )

      if (structuralParent) {
        const relationEvent = tx.run(
          `INSERT INTO session_relation_events (
             event_id, relation_id, operation, task_id, from_session_id, to_session_id,
             relation_kind, metadata_json, command_id, occurred_at
           ) VALUES (?, ?, 'created', ?, ?, ?, 'derived-from', '{}', ?, ?)`,
          `${command.commandId}:derived-relation-created`, relationId, task.id,
          ids.sessionId, structuralParent.parent_session_id, command.commandId, input.now
        )
        tx.run(
          `INSERT INTO session_relations_current (
             relation_id, task_id, from_session_id, to_session_id, relation_kind,
             metadata_json, created_at, updated_at, source_event_sequence
           ) VALUES (?, ?, ?, ?, 'derived-from', '{}', ?, ?, ?)`,
          relationId, task.id, ids.sessionId, structuralParent.parent_session_id,
          input.now, input.now, Number(relationEvent.lastInsertRowid)
        )
      }

      activateSessionInTransaction(tx, input.windowId, ids.sessionId, input.now)
      const hierarchy = readHierarchyResult(tx, input.windowId)
      const membership = readMembership(tx, ids.sessionId)
      const graph = projectSceneGraphFrom(tx, scene.id, input.windowId)
      emit({
        eventId: `${command.commandId}:session-created`,
        eventType: 'session.created', aggregateType: 'session', aggregateId: ids.sessionId,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: ids.sessionId,
        payload: hierarchy.session, occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:session-mounted`,
        eventType: 'scene.session-mounted', aggregateType: 'scene', aggregateId: scene.id,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: ids.sessionId,
        payload: hierarchy.mount, occurredAt: input.now
      })
      emitMembership(command.commandId, membership, task.workspace_id, task.id, emit, input.now)
      if (structuralParent) {
        emit({
          eventId: `${command.commandId}:domain-relation-created`,
          eventType: 'session-relation.created', aggregateType: 'session-relation',
          aggregateId: relationId, workspaceId: task.workspace_id, taskId: task.id,
          sessionId: ids.sessionId,
          payload: {
            id: relationId, taskId: task.id, fromSessionId: ids.sessionId,
            toSessionId: structuralParent.parent_session_id,
            kind: 'derived-from', metadata: {}, createdAt: input.now, updatedAt: input.now
          },
          occurredAt: input.now
        })
      }
      emit({
        eventId: `${command.commandId}:shell-sibling-created`,
        eventType: 'session.graph-summary-changed', aggregateType: 'scene',
        aggregateId: scene.id, workspaceId: task.workspace_id, taskId: task.id,
        sessionId: ids.sessionId,
        payload: { graph, sourceSessionId: source.id }, occurredAt: input.now
      })
      return { ...hierarchy, graph }
    }).result
  }

  restartStoppedSession(
    command: DomainCommandMetadata,
    input: RestartStoppedSessionInput
  ): SessionCanvasMutationResult {
    const ids = createHierarchyIds()
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const source = requireRow(tx.get<{
        id: string
        task_id: string
        execution_context_id: string
        kind: 'shell' | 'claude-code' | 'codex' | 'agent-team-member'
        title: string
        cwd: string
        scene_id: string
      }>(
        `SELECT sessions.id, sessions.task_id, sessions.execution_context_id,
                sessions.kind, sessions.title, sessions.cwd, membership.scene_id
         FROM sessions
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         WHERE sessions.id = ? AND sessions.archived_at IS NOT NULL`,
        input.sessionId
      ), 'Stopped Session')
      const scene = requireRow(tx.get<SceneRow>(
        'SELECT id, task_id FROM scenes WHERE id = ? AND archived_at IS NULL', source.scene_id
      ), 'Scene')
      const task = requireRow(tx.get<TaskRow>(
        `SELECT id, workspace_id, execution_context_id FROM tasks
         WHERE id = ? AND archived_at IS NULL`, scene.task_id
      ), 'Task')
      assertWorkspacePathAvailable(tx, task.workspace_id)
      const providerBinding = source.kind === 'claude-code' ? tx.get<{ id: string }>(
        `SELECT id FROM provider_bindings
         WHERE session_id = ? AND provider = 'claude-code'
           AND resume_state IN ('available', 'resumed')
           AND validated_at IS NOT NULL AND invalidated_at IS NULL
         ORDER BY updated_at DESC, id DESC LIMIT 1`,
        source.id
      ) : undefined
      const nextKind = providerBinding ? 'claude-code' : 'shell'

      tx.run(
        `UPDATE sessions SET kind = ?, status = 'created', archived_at = NULL,
           updated_at = ?, last_activity_at = ?, version = version + 1
         WHERE id = ?`,
        nextKind, input.now, input.now, source.id
      )
      const existingMount = tx.get<{ id: string }>(
        'SELECT id FROM session_mounts WHERE scene_id = ? AND session_id = ? LIMIT 1',
        scene.id, source.id
      )
      if (!existingMount) {
        const anchor = tx.get<{ scene_node_id: string }>(
          `SELECT mounts.scene_node_id
           FROM session_mounts AS mounts
           JOIN sessions ON sessions.id = mounts.session_id
           WHERE mounts.scene_id = ? AND mounts.scene_node_id IS NOT NULL
             AND sessions.archived_at IS NULL AND sessions.id <> ?
           ORDER BY sessions.last_activity_at DESC, mounts.created_at, mounts.id LIMIT 1`,
          scene.id, source.id
        )
        if (anchor) {
          const sourceNode = requireRow(tx.get<SceneNodeRow>(
            'SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?', anchor.scene_node_id
          ), 'SceneNode')
          insertHorizontalNode(tx, scene.id, sourceNode, ids, input.now)
          tx.run(
            `INSERT INTO session_mounts (
               id, scene_id, scene_node_id, session_id, created_at
             ) VALUES (?, ?, ?, ?, ?)`,
            ids.mountId, scene.id, ids.rootNodeId, source.id, input.now
          )
          tx.run(
            `UPDATE scenes SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
               layout_revision = layout_revision + 1, updated_at = ? WHERE id = ?`,
            sourceNode.id, ids.secondaryNodeId, input.now, scene.id
          )
        } else {
          tx.run(
            `INSERT INTO scene_nodes (
               id, scene_id, parent_node_id, kind, ordinal, created_at
             ) VALUES (?, ?, NULL, 'mount', 0, ?)`,
            ids.rootNodeId, scene.id, input.now
          )
          tx.run(
            `INSERT INTO session_mounts (
               id, scene_id, scene_node_id, session_id, created_at
             ) VALUES (?, ?, ?, ?, ?)`,
            ids.mountId, scene.id, ids.rootNodeId, source.id, input.now
          )
          tx.run(
            `UPDATE scenes SET root_node_id = ?, layout_revision = layout_revision + 1,
               updated_at = ? WHERE id = ?`,
            ids.rootNodeId, input.now, scene.id
          )
        }
      }
      if (providerBinding) {
        tx.run(
          `UPDATE provider_bindings SET resume_state = 'available',
             restore_state = 'none', restore_error = NULL, user_exited_at = NULL,
             updated_at = ? WHERE id = ?`,
          input.now, providerBinding.id
        )
      }
      activateSessionInTransaction(tx, input.windowId, source.id, input.now)
      const hierarchy = readHierarchyResult(tx, input.windowId)
      const graph = projectSceneGraphFrom(tx, scene.id, input.windowId)
      emit({
        eventId: `${command.commandId}:stopped-session-restarted`,
        eventType: 'session.stopped-state-changed', aggregateType: 'session',
        aggregateId: source.id, workspaceId: task.workspace_id, taskId: task.id,
        sessionId: source.id,
        payload: { session: hierarchy.session, graph },
        occurredAt: input.now
      })
      return { ...hierarchy, graph }
    }).result
  }

  removeSessionBranch(
    command: DomainCommandMetadata,
    input: RemoveSessionBranchInput
  ): RemoveSessionBranchResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const target = requireRow(tx.get<{
        session_id: string
        task_id: string
        workspace_id: string
        archived_at: number | null
      }>(
        `SELECT sessions.id AS session_id, sessions.task_id, tasks.workspace_id,
                sessions.archived_at
         FROM sessions
         JOIN tasks ON tasks.id = sessions.task_id
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         WHERE sessions.id = ? AND membership.scene_id = ?`,
        input.sessionId, input.sceneId
      ), 'Session')
      const descendants = tx.all<{ session_id: string }>(
        `WITH RECURSIVE branch(session_id) AS (
           SELECT ?
           UNION ALL
           SELECT relation.from_session_id
           FROM session_relations_current AS relation
           JOIN branch ON relation.to_session_id = branch.session_id
           JOIN session_canvas_memberships AS membership
             ON membership.session_id = relation.from_session_id
            AND membership.scene_id = ?
           WHERE relation.relation_kind IN ('derived-from', 'forked-from')
         )
         SELECT session_id FROM branch`,
        input.sessionId, input.sceneId
      ).map(({ session_id }) => session_id)
      const includeDescendants = input.scope === 'node-and-descendants'
      const removedSessionIds = includeDescendants
        ? descendants
        : [input.sessionId]
      const placeholders = removedSessionIds.map(() => '?').join(', ')
      const directChildren = includeDescendants ? [] : tx.all<{
        relation_id: string
        task_id: string
        from_session_id: string
        relation_kind: 'derived-from' | 'forked-from'
        metadata_json: string
      }>(
        `SELECT relation.relation_id, relation.task_id, relation.from_session_id,
                relation.relation_kind, relation.metadata_json
         FROM session_relations_current AS relation
         JOIN session_canvas_memberships AS membership
           ON membership.session_id = relation.from_session_id
          AND membership.scene_id = ?
         WHERE relation.to_session_id = ?
           AND relation.relation_kind IN ('derived-from', 'forked-from')
         ORDER BY membership.sibling_created_seq, relation.from_session_id`,
        input.sceneId, input.sessionId
      )
      const structuralParent = includeDescendants ? undefined : tx.get<{
        to_session_id: string
      }>(
        `SELECT relation.to_session_id
         FROM session_relations_current AS relation
         JOIN session_canvas_memberships AS membership
           ON membership.session_id = relation.to_session_id
          AND membership.scene_id = ?
         WHERE relation.from_session_id = ?
           AND relation.relation_kind IN ('derived-from', 'forked-from')
         ORDER BY relation.updated_at DESC, relation.relation_id
         LIMIT 1`,
        input.sceneId, input.sessionId
      )
      const focusedSessionId = tx.get<{ active_session_id: string | null }>(
        `SELECT active_session_id FROM window_scene_focus
         WHERE window_id = ? AND scene_id = ?`,
        input.windowId, input.sceneId
      )?.active_session_id ?? undefined
      const removesFocus = focusedSessionId !== undefined && removedSessionIds.includes(focusedSessionId)
      const survivingParent = removesFocus ? tx.get<{ id: string }>(
        `SELECT parent.id
         FROM session_relations_current AS relation
         JOIN sessions AS parent ON parent.id = relation.to_session_id
         JOIN session_canvas_memberships AS membership ON membership.session_id = parent.id
         WHERE relation.from_session_id = ?
           AND relation.relation_kind IN ('derived-from', 'forked-from')
           AND membership.scene_id = ? AND parent.archived_at IS NULL
         LIMIT 1`,
        input.sessionId, input.sceneId
      ) : undefined
      const survivingChild = removesFocus && !survivingParent && !includeDescendants
        ? tx.get<{ id: string }>(
          `SELECT child.id
           FROM session_relations_current AS relation
           JOIN sessions AS child ON child.id = relation.from_session_id
           JOIN session_canvas_memberships AS membership
             ON membership.session_id = child.id AND membership.scene_id = ?
           WHERE relation.to_session_id = ?
             AND relation.relation_kind IN ('derived-from', 'forked-from')
             AND child.archived_at IS NULL
           ORDER BY membership.sibling_created_seq, child.id
           LIMIT 1`,
          input.sceneId, input.sessionId
        )
        : undefined
      const survivingPeer = removesFocus && !survivingParent && !survivingChild ? tx.get<{ id: string }>(
        `SELECT sessions.id
         FROM session_canvas_memberships AS membership
         JOIN sessions ON sessions.id = membership.session_id
         WHERE membership.scene_id = ? AND membership.session_id NOT IN (${placeholders})
           AND sessions.archived_at IS NULL
         ORDER BY membership.last_user_interaction_seq DESC,
           membership.sibling_created_seq ASC, sessions.id
         LIMIT 1`,
        input.sceneId, ...removedSessionIds
      ) : undefined
      const replacementSessionId = survivingParent?.id ?? survivingChild?.id ?? survivingPeer?.id
      const disposedSessionIds = tx.all<{ id: string }>(
        `SELECT id FROM sessions WHERE id IN (${placeholders}) AND archived_at IS NULL
         ORDER BY created_at, id`,
        ...removedSessionIds
      ).map(({ id }) => id)
      tx.run(
        `UPDATE sessions SET status = 'archived', archived_at = COALESCE(archived_at, ?),
           updated_at = ?, version = version + 1
         WHERE id IN (${placeholders})`,
        input.now, input.now, ...removedSessionIds
      )
      tx.run(
        `DELETE FROM session_mounts WHERE session_id IN (${placeholders})`,
        ...removedSessionIds
      )
      const revokedRelations = tx.all<{
        relation_id: string
        task_id: string
        from_session_id: string
        to_session_id: string
        relation_kind: string
        metadata_json: string
      }>(
        `SELECT relation_id, task_id, from_session_id, to_session_id,
                relation_kind, metadata_json
         FROM session_relations_current
         WHERE from_session_id IN (${placeholders}) OR to_session_id IN (${placeholders})
         ORDER BY relation_id`,
        ...removedSessionIds, ...removedSessionIds
      )
      for (const relation of revokedRelations) {
        tx.run(
          `INSERT INTO session_relation_events (
             event_id, relation_id, operation, task_id, from_session_id, to_session_id,
             relation_kind, metadata_json, command_id, occurred_at
           ) VALUES (?, ?, 'revoked', ?, ?, ?, ?, ?, ?, ?)`,
          `${command.commandId}:relation-revoked:${relation.relation_id}`,
          relation.relation_id, relation.task_id, relation.from_session_id,
          relation.to_session_id, relation.relation_kind, relation.metadata_json,
          command.commandId, input.now
        )
        tx.run('DELETE FROM session_relations_current WHERE relation_id = ?', relation.relation_id)
      }
      if (structuralParent) {
        for (const child of directChildren) {
          const relationId = randomUUID()
          const insertion = tx.run(
            `INSERT INTO session_relation_events (
               event_id, relation_id, operation, task_id, from_session_id, to_session_id,
               relation_kind, metadata_json, command_id, occurred_at
             ) VALUES (?, ?, 'created', ?, ?, ?, ?, ?, ?, ?)`,
            `${command.commandId}:relation-reconnected:${child.relation_id}`,
            relationId, child.task_id, child.from_session_id, structuralParent.to_session_id,
            child.relation_kind, child.metadata_json, command.commandId, input.now
          )
          tx.run(
            `INSERT INTO session_relations_current (
               relation_id, task_id, from_session_id, to_session_id, relation_kind,
               metadata_json, created_at, updated_at, source_event_sequence
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
            relationId, child.task_id, child.from_session_id, structuralParent.to_session_id,
            child.relation_kind, child.metadata_json, input.now, input.now,
            Number(insertion.lastInsertRowid)
          )
        }
      }
      tx.run(
        `DELETE FROM session_canvas_memberships WHERE session_id IN (${placeholders})`,
        ...removedSessionIds
      )
      if (removesFocus) {
        if (replacementSessionId) {
          activateSessionInTransaction(tx, input.windowId, replacementSessionId, input.now)
        } else {
          tx.run(
            `UPDATE window_scene_focus SET active_session_id = NULL, updated_at = ?
             WHERE window_id = ? AND scene_id = ?`,
            input.now, input.windowId, input.sceneId
          )
        }
      }
      const graph = projectSceneGraphFrom(tx, input.sceneId, input.windowId)
      emit({
        eventId: `${command.commandId}:session-branch-removed`,
        eventType: 'session.graph-summary-changed', aggregateType: 'scene',
        aggregateId: input.sceneId, workspaceId: target.workspace_id,
        taskId: target.task_id, sessionId: input.sessionId,
        payload: { graph, removedSessionIds, disposedSessionIds }, occurredAt: input.now
      })
      return { graph, removedSessionIds, disposedSessionIds }
    }).result
  }

  upsertAgentTeamMember(
    command: DomainCommandMetadata,
    input: UpsertAgentTeamMemberInput
  ): UpsertAgentTeamMemberResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const lead = requireRow(tx.get<{
        id: string
        task_id: string
        execution_context_id: string
        cwd: string
        workspace_id: string
        scene_id: string
        scene_node_id: string
      }>(
        `SELECT sessions.id, sessions.task_id, sessions.execution_context_id, sessions.cwd,
                tasks.workspace_id, membership.scene_id, mounts.scene_node_id
         FROM sessions
         JOIN tasks ON tasks.id = sessions.task_id
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         JOIN session_mounts AS mounts
           ON mounts.session_id = sessions.id AND mounts.scene_id = membership.scene_id
         WHERE sessions.id = ? AND sessions.archived_at IS NULL
           AND mounts.scene_node_id IS NOT NULL AND mounts.scene_window_id IS NULL
         ORDER BY mounts.created_at, mounts.id LIMIT 1`,
        input.leadSessionId
      ), 'Agent team lead Session')
      const existing = tx.all<{
        session_id: string
        metadata_json: string
      }>(
        `SELECT relation.from_session_id AS session_id, relation.metadata_json
         FROM session_relations_current AS relation
         JOIN sessions ON sessions.id = relation.from_session_id
         WHERE relation.to_session_id = ? AND relation.relation_kind = 'derived-from'
           AND sessions.kind = 'agent-team-member' AND sessions.archived_at IS NULL`,
        lead.id
      ).find(({ metadata_json }) => relationMetadata(metadata_json).teammateId === input.teammateId)
      const latestLines = input.latestLines.map((line) => line.trim()).filter(Boolean).slice(-4)
      let sessionId = existing?.session_id
      let created = false
      if (sessionId) {
        tx.run(
          `UPDATE sessions SET title = ?, status = 'running', work_status = ?,
             updated_at = ?, last_activity_at = ?, version = version + 1
           WHERE id = ?`,
          input.name, input.workStatus, input.now, input.now, sessionId
        )
      } else {
        created = true
        const ids = createHierarchyIds()
        const relationId = randomUUID()
        sessionId = ids.sessionId
        const sourceNode = requireRow(tx.get<SceneNodeRow>(
          'SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?',
          lead.scene_node_id
        ), 'Agent team lead SceneNode')
        insertHorizontalNode(tx, lead.scene_id, sourceNode, ids, input.now)
        tx.run(
          `INSERT INTO sessions (
             id, task_id, execution_context_id, kind, status, work_status, title, cwd,
             created_at, updated_at, last_activity_at, version
           ) VALUES (?, ?, ?, 'agent-team-member', 'running', ?, ?, ?, ?, ?, ?, 1)`,
          sessionId, lead.task_id, lead.execution_context_id, input.workStatus,
          input.name, lead.cwd, input.now, input.now, input.now
        )
        tx.run(
          `INSERT INTO session_mounts (
             id, scene_id, scene_node_id, session_id, created_at
           ) VALUES (?, ?, ?, ?, ?)`,
          ids.mountId, lead.scene_id, ids.rootNodeId, sessionId, input.now
        )
        tx.run(
          `UPDATE scenes
           SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
               layout_revision = layout_revision + 1, updated_at = ?
           WHERE id = ?`,
          sourceNode.id, ids.secondaryNodeId, input.now, lead.scene_id
        )
        const metadataJson = JSON.stringify({
          teamId: input.teamId,
          teammateId: input.teammateId,
          role: 'teammate'
        })
        const relationInsertion = tx.run(
          `INSERT INTO session_relation_events (
             event_id, relation_id, operation, task_id, from_session_id, to_session_id,
             relation_kind, metadata_json, command_id, occurred_at
           ) VALUES (?, ?, 'created', ?, ?, ?, 'derived-from', ?, ?, ?)`,
          `${command.commandId}:team-relation-created`, relationId, lead.task_id,
          sessionId, lead.id, metadataJson, command.commandId, input.now
        )
        tx.run(
          `INSERT INTO session_relations_current (
             relation_id, task_id, from_session_id, to_session_id, relation_kind,
             metadata_json, created_at, updated_at, source_event_sequence
           ) VALUES (?, ?, ?, ?, 'derived-from', ?, ?, ?, ?)`,
          relationId, lead.task_id, sessionId, lead.id, metadataJson,
          input.now, input.now, Number(relationInsertion.lastInsertRowid)
        )
      }
      tx.run(
        `INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
         VALUES (?, ?, ?)
         ON CONFLICT(session_id) DO UPDATE SET
           latest_lines_json = excluded.latest_lines_json,
           updated_at = excluded.updated_at`,
        sessionId, JSON.stringify(latestLines), input.now
      )
      const windowId = tx.get<{ window_id: string }>(
        `SELECT window_id FROM window_scene_focus
         WHERE scene_id = ? ORDER BY updated_at DESC, window_id LIMIT 1`,
        lead.scene_id
      )?.window_id
      const graph = projectSceneGraphFrom(tx, lead.scene_id, windowId)
      emit({
        eventId: `${command.commandId}:team-member-observed`,
        eventType: 'session.team-member-observed',
        aggregateType: 'session', aggregateId: sessionId,
        workspaceId: lead.workspace_id, taskId: lead.task_id, sessionId,
        payload: {
          graph, leadSessionId: lead.id, teamId: input.teamId,
          teammateId: input.teammateId, created
        },
        occurredAt: input.now
      })
      return { sessionId, created, graph }
    }).result
  }

  projectSceneGraph(sceneId: string, windowId?: string): SceneSessionGraph {
    return projectSceneGraphFrom(this.#database, sceneId, windowId)
  }

  setFocusedSession(input: SetFocusedSessionInput): SceneSessionGraph {
    return this.#database.transaction((tx) => {
      registerWindow(tx, input.windowId, input.now)
      const owner = tx.get<{ scene_id: string }>(
        `SELECT scene_id FROM session_canvas_memberships
         WHERE session_id = ? AND scene_id = ?`,
        input.sessionId, input.sceneId
      )
      if (!owner) throw new Error('会话不在当前画布中')
      activateSessionInTransaction(tx, input.windowId, input.sessionId, input.now)
      return projectSceneGraphFrom(tx, input.sceneId, input.windowId)
    })
  }
}

function emitMembership(
  commandId: string,
  membership: SessionCanvasMembership,
  workspaceId: string,
  taskId: string,
  emit: (event: {
    eventId: string
    eventType: string
    aggregateType: string
    aggregateId: string
    workspaceId: string
    taskId: string
    sessionId: string
    payload: unknown
    occurredAt: number
  }) => void,
  now: number
): void {
  emit({
    eventId: `${commandId}:canvas-membership-created`,
    eventType: 'session.canvas-membership-created',
    aggregateType: 'session', aggregateId: membership.sessionId,
    workspaceId, taskId, sessionId: membership.sessionId,
    payload: { membership }, occurredAt: now
  })
}

function readMembership(tx: DatabaseTransaction, sessionId: string): SessionCanvasMembership {
  const row = requireRow(tx.get<MembershipRow>(
    'SELECT * FROM session_canvas_memberships WHERE session_id = ?',
    sessionId
  ), 'SessionCanvasMembership')
  return {
    sessionId: row.session_id,
    sceneId: row.scene_id,
    siblingCreatedSeq: row.sibling_created_seq,
    lastUserInteractionSeq: row.last_user_interaction_seq,
    createdAt: row.created_at,
    updatedAt: row.updated_at
  }
}

function nextCanvasName(tx: DatabaseTransaction, taskId: string): string {
  const names = new Set(tx.all<{ name: string }>(
    `SELECT name FROM scenes WHERE task_id = ? AND archived_at IS NULL`,
    taskId
  ).map(({ name }) => name))
  if (!names.has('新画布')) return '新画布'
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `新画布 ${suffix}`
    if (!names.has(candidate)) return candidate
  }
}

function sortKey(index: number): string {
  return `a${index.toString().padStart(8, '0')}`
}

function insertHorizontalNode(
  tx: DatabaseTransaction,
  sceneId: string,
  sourceNode: SceneNodeRow,
  ids: ReturnType<typeof createHierarchyIds>,
  now: number
): void {
  tx.run(
    `INSERT INTO scene_nodes (
       id, scene_id, parent_node_id, kind, direction, ordinal, created_at
     ) VALUES (?, ?, ?, 'split', 'horizontal', ?, ?)`,
    ids.secondaryNodeId, sceneId, sourceNode.parent_node_id, sourceNode.ordinal, now
  )
  tx.run(
    `UPDATE scene_nodes SET parent_node_id = ?, kind = 'mount', direction = NULL, ordinal = 0
     WHERE id = ?`,
    ids.secondaryNodeId, sourceNode.id
  )
  tx.run(
    `INSERT INTO scene_nodes (
       id, scene_id, parent_node_id, kind, ordinal, created_at
     ) VALUES (?, ?, ?, 'mount', 1, ?)`,
    ids.rootNodeId, sceneId, ids.secondaryNodeId, now
  )
}

function relationMetadata(value: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(value) as unknown
    return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : {}
  } catch {
    return {}
  }
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (row === undefined) throw new Error(`${label} does not exist`)
  return row
}
