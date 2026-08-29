import { randomUUID } from 'node:crypto'

import type {
  DomainCommandMetadata,
  SceneSessionGraph,
  SessionCanvasMembership
} from '@matou/domain'

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
  now: number
}

export interface SetFocusedSessionInput {
  windowId: string
  sceneId: string
  sessionId: string
  now: number
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
         JOIN session_mounts ON session_mounts.session_id = sessions.id
         WHERE sessions.id = ? AND session_mounts.scene_id = ?
           AND sessions.archived_at IS NULL`,
        input.sourceSessionId, input.sceneId
      ), 'Session')
      const sourceMount = requireRow(tx.get<SourceMountRow>(
        `SELECT id, scene_node_id, scene_window_id FROM session_mounts
         WHERE scene_id = ? AND session_id = ?
         ORDER BY created_at, id LIMIT 1`,
        input.sceneId, input.sourceSessionId
      ), 'SessionMount')
      if (sourceMount.scene_window_id !== null || sourceMount.scene_node_id === null) {
        throw new Error('独立窗口中的会话需要先回到原会话列表')
      }
      const sourceNode = requireRow(tx.get<SceneNodeRow>(
        `SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?`,
        sourceMount.scene_node_id
      ), 'SceneNode')
      const structuralParent = tx.get<{ parent_session_id: string }>(
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
        ids.sessionId, task.id, source.execution_context_id, source.cwd,
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

function requireRow<T>(row: T | undefined, label: string): T {
  if (row === undefined) throw new Error(`${label} does not exist`)
  return row
}
