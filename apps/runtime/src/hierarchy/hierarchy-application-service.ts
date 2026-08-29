import { randomUUID } from 'node:crypto'
import { resolve } from 'node:path'

import type {
  DomainCommandMetadata,
  PlainDirectoryContext,
  Scene,
  Session,
  SessionMount,
  Task,
  WindowNavigation,
  Workspace
} from '@matou/domain'

import { createHierarchyIds, type HierarchyIds } from './hierarchy-ids'
import { WORKSPACE_PATH_INVALID_MESSAGE } from './workspace-path-service'
import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type {
  DomainMutationContext,
  DomainTransactionManager
} from '../storage/domain-transaction'

interface WorkspaceRow {
  id: string
  name: string
  root_directory: string
  path_identity: string | null
  task_order_json: string
  is_default: number
  is_pinned: number
  pin_sort_key: string
  last_opened_at: number
  created_at: number
  updated_at: number
  archived_at: number | null
  version: number
}

interface TaskRow {
  id: string
  workspace_id: string
  parent_task_id: string | null
  title: string
  status: Task['status']
  execution_context_id: string
  sort_key: string
  is_pinned: number
  pin_sort_key: string
  last_opened_at: number
  created_at: number
  updated_at: number
  archived_at: number | null
  version: number
}

interface SceneRow {
  id: string
  task_id: string
  name: string
  mode: Scene['mode']
  root_node_id: string | null
  title_pinned: number
  sort_key: string
  layout_revision: number
  created_at: number
  updated_at: number
  archived_at: number | null
}

interface SessionRow {
  id: string
  task_id: string
  execution_context_id: string
  kind: Session['kind']
  status: Session['status']
  title: string
  cwd: string
  created_at: number
  updated_at: number
  last_activity_at: number
  archived_at: number | null
  version: number
}

interface ContextRow {
  id: string
  workspace_id: string
  cwd: string
  created_at: number
  archived_at: number | null
}

interface MountRow {
  id: string
  scene_id: string
  scene_node_id: string | null
  scene_window_id: string | null
  session_id: string
  created_at: number
}

export interface BootstrapWindowInput {
  windowId: string
  defaultRootDirectory: string
  defaultName: string
  now: number
}

export interface CreateWorkspaceInput {
  windowId: string
  name: string
  rootDirectory: string
  now: number
}

export interface RenameWorkspaceInput {
  workspaceId: string
  name: string
  now: number
}
export interface RelinkWorkspaceInput { workspaceId: string; rootDirectory: string; now: number }

export interface RemoveWorkspaceInput {
  windowId: string
  workspaceId: string
  confirmedIntent: string
  now: number
}

export interface ActivateWorkspaceInput {
  windowId: string
  workspaceId: string
  now: number
}

export interface SetWorkspacePinnedInput { workspaceId: string; pinned: boolean; now: number }
export interface ReorderPinnedWorkspaceInput { workspaceId: string; beforeWorkspaceId?: string; now: number }
export interface SetTaskPinnedInput { taskId: string; pinned: boolean; now: number }
export interface ReorderPinnedTaskInput { workspaceId: string; taskId: string; beforeTaskId?: string; now: number }

export interface WorkspaceHierarchyResult {
  workspace: Workspace | null
  executionContext: PlainDirectoryContext | null
  task: Task | null
  scene: Scene | null
  session: Session | null
  mount: SessionMount | null
  navigation: WindowNavigation
}

export interface CreateTaskWorkflowInput {
  windowId: string
  workspaceId: string
  now: number
}

export interface RenameTaskWorkflowInput {
  taskId: string
  title: string
  now: number
}

export interface ReorderTaskWorkflowInput {
  windowId: string
  workspaceId: string
  taskId: string
  beforeTaskId?: string
  now: number
}

export interface DeleteTaskWorkflowInput {
  windowId: string
  taskId: string
  confirmedIntent: string
  now: number
}

export interface ActivateTaskInput {
  windowId: string
  taskId: string
  now: number
}

export interface RecordSessionInteractionInput {
  sessionId: string
  now: number
}

export interface TaskOrderResult extends WorkspaceHierarchyResult {
  taskOrder: string[]
}

export interface HierarchyMutationResult extends WorkspaceHierarchyResult {
  disposedSessionIds: string[]
}

export interface CreateSceneWorkflowInput {
  windowId: string
  taskId: string
  now: number
}

export interface RenameSceneWorkflowInput {
  sceneId: string
  name: string
  now: number
}

export interface ReorderSceneWorkflowInput {
  windowId: string
  sceneId: string
  beforeSceneId?: string
  now: number
}

export interface CloseSceneWorkflowInput {
  windowId: string
  sceneId: string
  confirmedIntent?: string
  now: number
}

export interface ActivateSceneInput {
  windowId: string
  sceneId: string
  now: number
}

export interface ReopenSceneWorkflowInput {
  windowId: string
  sceneId: string
  now: number
}

export interface CloseSceneResult extends HierarchyMutationResult {
  action: 'hide-window' | 'closed'
}

export interface SplitSessionWorkflowInput {
  windowId: string
  sceneId: string
  sourceSessionId: string
  direction: 'horizontal' | 'vertical'
  now: number
}

export interface ForkSessionWorkflowInput {
  windowId: string
  sceneId: string
  sourceSessionId: string
  now: number
}

export interface DeleteSessionWorkflowInput {
  windowId: string
  sessionId: string
  confirmedIntent?: string
  now: number
}

export interface ActivateSessionInput {
  windowId: string
  sessionId: string
  now: number
}

export interface DeleteSessionResult extends HierarchyMutationResult {
  outcome: 'scene-remains' | 'scene-archives' | 'default-task-created'
}

export class HierarchyApplicationService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  bootstrapWindow(
    command: DomainCommandMetadata,
    input: BootstrapWindowInput
  ): WorkspaceHierarchyResult {
    const ids = createHierarchyIds()
    return this.#transactions.execute(command, (context) => {
      registerWindow(context.tx, input.windowId, input.now)
      const activeBefore = context.tx.get<WorkspaceRow>(
        `SELECT workspaces.* FROM window_navigation
         JOIN workspaces ON workspaces.id = window_navigation.active_workspace_id
         WHERE window_navigation.window_id = ? AND workspaces.archived_at IS NULL`,
        input.windowId
      )
      let workspace = context.tx.get<WorkspaceRow>(
        'SELECT * FROM workspaces WHERE root_directory = ? AND archived_at IS NULL LIMIT 1',
        resolve(input.defaultRootDirectory)
      )
      if (!workspace) {
        const created = this.#createCompleteHierarchy(context, {
          ids,
          windowId: input.windowId,
          name: input.defaultName,
          rootDirectory: input.defaultRootDirectory,
          taskTitle: '默认',
          commandId: command.commandId,
          now: input.now,
          isDefault: true,
          isPinned: true
        })
        writeBootstrapFlag(
          context.tx,
          'default-workspace-created',
          created.id,
          input.now
        )
        workspace = context.tx.get<WorkspaceRow>('SELECT * FROM workspaces WHERE id = ?', created.id)
      } else {
        context.tx.run(
          `UPDATE workspaces SET is_default = 0, updated_at = ?, version = version + 1
           WHERE is_default = 1 AND id <> ?`,
          input.now, workspace.id
        )
        context.tx.run(
          `UPDATE workspaces SET is_default = 1, updated_at = ?, version = version + 1 WHERE id = ?`,
          input.now, workspace.id
        )
      }
      activateWorkspaceInTransaction(
        context.tx, input.windowId, activeBefore?.id ?? workspace!.id, input.now
      )
      return readHierarchyResult(context.tx, input.windowId)
    }).result
  }

  createWorkspace(
    command: DomainCommandMetadata,
    input: CreateWorkspaceInput
  ): WorkspaceHierarchyResult {
    const rootDirectory = resolve(input.rootDirectory)
    const ids = createHierarchyIds()
    return this.#transactions.execute(command, (context) => {
      registerWindow(context.tx, input.windowId, input.now)
      const existing = context.tx.get<WorkspaceRow>(
        `SELECT * FROM workspaces
         WHERE root_directory = ? AND archived_at IS NULL
         ORDER BY created_at LIMIT 1`,
        rootDirectory
      )
      if (existing) {
        activateWorkspaceInTransaction(context.tx, input.windowId, existing.id, input.now)
      } else {
        this.#createCompleteHierarchy(context, {
          ids,
          windowId: input.windowId,
          name: input.name,
          rootDirectory,
          taskTitle: '默认',
          commandId: command.commandId,
          now: input.now
        })
      }
      return readHierarchyResult(context.tx, input.windowId)
    }).result
  }

  renameWorkspace(
    _command: DomainCommandMetadata,
    input: RenameWorkspaceInput
  ): Workspace {
    const row = this.#database.get<WorkspaceRow>(
      'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL', input.workspaceId
    )
    if (!row) throw new Error(`Workspace ${input.workspaceId} does not exist`)
    throw new Error('工作空间名称跟随目录名称')
  }

  relinkWorkspace(command: DomainCommandMetadata, input: RelinkWorkspaceInput): Workspace {
    const rootDirectory = resolve(input.rootDirectory)
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const row = requireRow<WorkspaceRow>(tx.get(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL', input.workspaceId
      ), 'Workspace')
      if (row.is_default === 1) throw new Error('默认工作空间始终指向 macOS 用户目录')
      const duplicate = tx.get<{ id: string }>(
        'SELECT id FROM workspaces WHERE root_directory = ? AND archived_at IS NULL AND id <> ?',
        rootDirectory, input.workspaceId
      )
      if (duplicate) throw new Error('该目录已经属于另一个工作空间')
      tx.run(`UPDATE workspaces SET root_directory = ?, path_identity = ?, updated_at = ?, version = version + 1 WHERE id = ?`,
        rootDirectory, `path:${rootDirectory}`, input.now, input.workspaceId)
      tx.run('UPDATE execution_contexts SET cwd = ? WHERE workspace_id = ? AND archived_at IS NULL', rootDirectory, input.workspaceId)
      tx.run(`UPDATE sessions SET cwd = ?, updated_at = ?, version = version + 1
        WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?) AND archived_at IS NULL`,
        rootDirectory, input.now, input.workspaceId)
      tx.run(`UPDATE scenes SET name = ?, updated_at = ?
        WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?) AND archived_at IS NULL AND title_pinned = 0`,
        `Shell · ${rootDirectory}`, input.now, input.workspaceId)
      tx.run('DELETE FROM workspace_path_state WHERE workspace_id = ?', input.workspaceId)
      const workspace = mapWorkspace({ ...row, root_directory: rootDirectory, path_identity: `path:${rootDirectory}`,
        updated_at: input.now, version: row.version + 1 })
      emit({ eventId: `${command.commandId}:workspace-relinked`, eventType: 'workspace.relinked',
        aggregateType: 'workspace', aggregateId: row.id, workspaceId: row.id,
        payload: { rootDirectory, pathIdentity: `path:${rootDirectory}` }, occurredAt: input.now })
      return workspace
    }).result
  }

  removeWorkspace(
    command: DomainCommandMetadata,
    input: RemoveWorkspaceInput
  ): HierarchyMutationResult {
    if (input.confirmedIntent !== `remove-workspace:${input.workspaceId}`) {
      throw new Error('Workspace removal intent is stale')
    }
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const workspace = tx.get<WorkspaceRow>(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL',
        input.workspaceId
      )
      if (!workspace) throw new Error(`Workspace ${input.workspaceId} does not exist`)
      if (workspace.is_default === 1) throw new Error('默认工作空间会保留在侧栏中')
      const disposedSessionIds = tx.all<{ id: string }>(
        `SELECT sessions.id FROM sessions
         JOIN tasks ON tasks.id = sessions.task_id
         WHERE tasks.workspace_id = ? AND sessions.archived_at IS NULL`,
        input.workspaceId
      ).map(({ id }) => id)

      tx.run(
        `UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?) AND archived_at IS NULL`,
        input.now,
        input.now,
        input.workspaceId
      )
      tx.run(
        `UPDATE scenes SET archived_at = ?, updated_at = ?
         WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?) AND archived_at IS NULL`,
        input.now,
        input.now,
        input.workspaceId
      )
      tx.run(
        `UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE workspace_id = ? AND archived_at IS NULL`,
        input.now,
        input.now,
        input.workspaceId
      )
      tx.run(
        'UPDATE execution_contexts SET archived_at = ? WHERE workspace_id = ? AND archived_at IS NULL',
        input.now,
        input.workspaceId
      )
      tx.run(
        `UPDATE workspaces SET archived_at = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        input.now,
        input.now,
        input.workspaceId
      )

      tx.run(
        `DELETE FROM window_scene_focus
         WHERE scene_id IN (
           SELECT scenes.id FROM scenes JOIN tasks ON tasks.id = scenes.task_id
           WHERE tasks.workspace_id = ?
         )`,
        input.workspaceId
      )
      tx.run(
        `DELETE FROM window_task_focus
         WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?)`,
        input.workspaceId
      )
      tx.run('DELETE FROM window_workspace_focus WHERE workspace_id = ?', input.workspaceId)
      tx.run(
        `DELETE FROM window_task_placements
         WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = ?)`,
        input.workspaceId
      )

      const createdDefaultId = readBootstrapFlag(tx, 'default-workspace-created')
      if (createdDefaultId === input.workspaceId) {
        writeBootstrapFlag(tx, 'default-workspace-removed', true, input.now)
      }

      const next = firstActiveWorkspace(tx)
      const affectedWindows = tx.all<{ id: string }>(
        `SELECT app_windows.id FROM app_windows
         LEFT JOIN window_navigation ON window_navigation.window_id = app_windows.id
         WHERE window_navigation.active_workspace_id = ? OR app_windows.id = ?`,
        input.workspaceId,
        input.windowId
      )
      for (const { id } of affectedWindows) {
        if (next) activateWorkspaceInTransaction(tx, id, next.id, input.now)
        else clearWindowNavigation(tx, id, input.now)
      }

      emit({
        eventId: `${command.commandId}:workspace-removed`,
        eventType: 'workspace.archived',
        aggregateType: 'workspace',
        aggregateId: input.workspaceId,
        workspaceId: input.workspaceId,
        payload: { archivedAt: input.now },
        occurredAt: input.now
      })
      return { ...readHierarchyResult(tx, input.windowId), disposedSessionIds }
    }).result
  }

  activateWorkspace(input: ActivateWorkspaceInput): WorkspaceHierarchyResult {
    return this.#database.transaction((tx) => {
      registerWindow(tx, input.windowId, input.now)
      const workspace = tx.get<{ id: string }>(
        'SELECT id FROM workspaces WHERE id = ? AND archived_at IS NULL',
        input.workspaceId
      )
      if (!workspace) throw new Error(`Workspace ${input.workspaceId} does not exist`)
      activateWorkspaceInTransaction(tx, input.windowId, input.workspaceId, input.now)
      return readHierarchyResult(tx, input.windowId)
    })
  }

  setWorkspacePinned(command: DomainCommandMetadata, input: SetWorkspacePinnedInput): Workspace {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const row = requireRow<WorkspaceRow>(tx.get(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL', input.workspaceId
      ), 'Workspace')
      const nextKey = input.pinned ? nextPinnedKey(tx, 'workspaces') : ''
      tx.run(`UPDATE workspaces SET is_pinned = ?, pin_sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?`,
        input.pinned ? 1 : 0, nextKey, input.now, input.workspaceId)
      const workspace = mapWorkspace({ ...row, is_pinned: input.pinned ? 1 : 0,
        pin_sort_key: nextKey, updated_at: input.now, version: row.version + 1 })
      emit({ eventId: `${command.commandId}:workspace-pinned`, eventType: 'workspace.pin-changed',
        aggregateType: 'workspace', aggregateId: row.id, workspaceId: row.id,
        payload: { isPinned: input.pinned, pinSortKey: nextKey }, occurredAt: input.now })
      return workspace
    }).result
  }

  reorderPinnedWorkspace(command: DomainCommandMetadata, input: ReorderPinnedWorkspaceInput): Workspace[] {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const rows = tx.all<WorkspaceRow>(
        `SELECT * FROM workspaces WHERE archived_at IS NULL AND is_pinned = 1 ORDER BY pin_sort_key, id`
      )
      const order = moveBefore(rows.map(({ id }) => id), input.workspaceId, input.beforeWorkspaceId)
      order.forEach((id, index) => tx.run(
        'UPDATE workspaces SET pin_sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?',
        sortKey(index), input.now, id
      ))
      emit({ eventId: `${command.commandId}:workspace-pin-order`, eventType: 'workspace.pin-order-changed',
        aggregateType: 'workspace-navigation', aggregateId: 'global',
        payload: { workspaceIds: order }, occurredAt: input.now })
      return order.map((id) => mapWorkspace(requireRow<WorkspaceRow>(tx.get('SELECT * FROM workspaces WHERE id = ?', id), 'Workspace')))
    }).result
  }

  createTask(
    command: DomainCommandMetadata,
    input: CreateTaskWorkflowInput
  ): WorkspaceHierarchyResult {
    const ids = createHierarchyIds()
    return this.#transactions.execute(command, (context) => {
      registerWindow(context.tx, input.windowId, input.now)
      const workspace = requireRow<WorkspaceRow>(context.tx.get(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL',
        input.workspaceId
      ), 'Workspace')
      assertWorkspacePathAvailable(context.tx, input.workspaceId)
      const title = nextTaskTitle(context.tx, input.workspaceId)
      this.#createTaskHierarchy(context, {
        ids,
        windowId: input.windowId,
        workspace,
        title,
        commandId: command.commandId,
        now: input.now,
        lastOpenedAt: 0
      })
      return readHierarchyResult(context.tx, input.windowId)
    }).result
  }

  renameTask(
    command: DomainCommandMetadata,
    input: RenameTaskWorkflowInput
  ): Task {
    const title = requiredTrimmed(input.title, 'Task title')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const row = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL',
        input.taskId
      ), 'Task')
      const duplicate = tx.get(
        `SELECT id FROM tasks
         WHERE workspace_id = ? AND title = ? AND archived_at IS NULL AND id <> ?`,
        row.workspace_id,
        title,
        input.taskId
      )
      if (duplicate) {
        throw new Error(`an active Task named "${title}" already exists in this Workspace`)
      }
      tx.run(
        `UPDATE tasks SET title = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        title,
        input.now,
        input.taskId
      )
      const task = mapTask({
        ...row,
        title,
        updated_at: input.now,
        version: row.version + 1
      })
      emit({
        eventId: `${command.commandId}:task-renamed`,
        eventType: 'task.renamed',
        aggregateType: 'task',
        aggregateId: task.id,
        workspaceId: task.workspaceId,
        taskId: task.id,
        payload: { title },
        occurredAt: input.now
      })
      return task
    }).result
  }

  reorderTask(
    command: DomainCommandMetadata,
    input: ReorderTaskWorkflowInput
  ): TaskOrderResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const workspace = requireRow<WorkspaceRow>(tx.get(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL',
        input.workspaceId
      ), 'Workspace')
      const activeIds = tx.all<{ id: string }>(
        `SELECT id FROM tasks WHERE workspace_id = ? AND archived_at IS NULL`,
        input.workspaceId
      ).map(({ id }) => id)
      if (!activeIds.includes(input.taskId)) throw new Error('Task must belong to the Workspace')
      if (input.beforeTaskId !== undefined && !activeIds.includes(input.beforeTaskId)) {
        throw new Error('before Task must belong to the Workspace')
      }
      const order = parseStringArray(workspace.task_order_json)
        .filter((id) => activeIds.includes(id))
      const sourceIndex = order.indexOf(input.taskId)
      const targetIndex = input.beforeTaskId === undefined
        ? order.length - 1
        : order.indexOf(input.beforeTaskId)
      if (sourceIndex < 0 || targetIndex < 0 || sourceIndex === targetIndex) {
        return { ...readHierarchyResult(tx, input.windowId), taskOrder: order }
      }
      order.splice(sourceIndex, 1)
      order.splice(targetIndex, 0, input.taskId)
      for (const [index, taskId] of order.entries()) {
        tx.run(
          'UPDATE tasks SET sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?',
          sortKey(index),
          input.now,
          taskId
        )
        tx.run(
          'UPDATE window_task_placements SET ordinal = ?, updated_at = ? WHERE task_id = ?',
          index,
          input.now,
          taskId
        )
      }
      tx.run(
        `UPDATE workspaces SET task_order_json = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        JSON.stringify(order),
        input.now,
        input.workspaceId
      )
      emit({
        eventId: `${command.commandId}:task-order-changed`,
        eventType: 'workspace.task-order-changed',
        aggregateType: 'workspace',
        aggregateId: input.workspaceId,
        workspaceId: input.workspaceId,
        payload: { taskOrder: order },
        occurredAt: input.now
      })
      return {
        ...readHierarchyResult(tx, input.windowId),
        taskOrder: order
      }
    }).result
  }

  deleteTask(
    command: DomainCommandMetadata,
    input: DeleteTaskWorkflowInput
  ): HierarchyMutationResult {
    if (input.confirmedIntent !== `delete-task:${input.taskId}`) {
      throw new Error('Task deletion intent is stale')
    }
    const replacementIds = createHierarchyIds()
    return this.#transactions.execute(command, (context) => {
      const { tx, emit } = context
      registerWindow(tx, input.windowId, input.now)
      const task = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL',
        input.taskId
      ), 'Task')
      const workspace = requireRow<WorkspaceRow>(tx.get(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL',
        task.workspace_id
      ), 'Workspace')
      const disposedSessionIds = tx.all<{ id: string }>(
        'SELECT id FROM sessions WHERE task_id = ? AND archived_at IS NULL ORDER BY created_at',
        input.taskId
      ).map(({ id }) => id)
      const affectedWindowIds = tx.all<{ window_id: string }>(
        `SELECT window_id FROM window_workspace_focus
         WHERE workspace_id = ? AND active_task_id = ?`,
        task.workspace_id,
        input.taskId
      ).map(({ window_id }) => window_id)

      tx.run(
        `UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE task_id = ? AND archived_at IS NULL`,
        input.now,
        input.now,
        input.taskId
      )
      tx.run(
        `UPDATE scenes SET archived_at = ?, updated_at = ?
         WHERE task_id = ? AND archived_at IS NULL`,
        input.now,
        input.now,
        input.taskId
      )
      tx.run(
        `UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        input.now,
        input.now,
        input.taskId
      )
      tx.run('DELETE FROM window_task_placements WHERE task_id = ?', input.taskId)
      tx.run(
        `DELETE FROM window_scene_focus WHERE scene_id IN (
           SELECT id FROM scenes WHERE task_id = ?
         )`,
        input.taskId
      )
      tx.run('DELETE FROM window_task_focus WHERE task_id = ?', input.taskId)
      tx.run('DELETE FROM task_status_entries WHERE task_id = ?', input.taskId)
      tx.run('DELETE FROM task_progress WHERE task_id = ?', input.taskId)
      tx.run('DELETE FROM task_logs WHERE task_id = ?', input.taskId)

      const order = parseStringArray(workspace.task_order_json)
        .filter((taskId) => taskId !== input.taskId)
      tx.run(
        `UPDATE workspaces SET task_order_json = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        JSON.stringify(order),
        input.now,
        task.workspace_id
      )
      for (const [index, taskId] of order.entries()) {
        tx.run(
          'UPDATE tasks SET sort_key = ? WHERE id = ?',
          sortKey(index),
          taskId
        )
        tx.run(
          'UPDATE window_task_placements SET ordinal = ?, updated_at = ? WHERE task_id = ?',
          index,
          input.now,
          taskId
        )
      }
      emit({
        eventId: `${command.commandId}:task-archived`,
        eventType: 'task.archived',
        aggregateType: 'task',
        aggregateId: input.taskId,
        workspaceId: task.workspace_id,
        taskId: input.taskId,
        payload: { archivedAt: input.now, disposedSessionIds },
        occurredAt: input.now
      })

      const nextTask = preferredTask(tx, input.windowId, task.workspace_id)
      if (nextTask) {
        activateTaskInTransaction(tx, input.windowId, nextTask.id, input.now)
      } else if (workspacePathIsAvailable(tx, task.workspace_id)) {
        const refreshedWorkspace = requireRow<WorkspaceRow>(tx.get(
          'SELECT * FROM workspaces WHERE id = ?', task.workspace_id
        ), 'Workspace')
        this.#createTaskHierarchy(context, {
          ids: replacementIds,
          windowId: input.windowId,
          workspace: refreshedWorkspace,
          title: '默认',
          commandId: command.commandId,
          now: input.now
        })
      } else {
        tx.run(
          `INSERT INTO window_workspace_focus (
             window_id, workspace_id, active_task_id, updated_at
           ) VALUES (?, ?, NULL, ?)
           ON CONFLICT(window_id, workspace_id) DO UPDATE SET
             active_task_id = NULL, updated_at = excluded.updated_at`,
          input.windowId,
          task.workspace_id,
          input.now
        )
      }
      for (const windowId of affectedWindowIds) {
        const fallback = preferredTask(tx, windowId, task.workspace_id)
        if (fallback) {
          activateTaskInTransaction(tx, windowId, fallback.id, input.now)
        } else {
          tx.run(
            `UPDATE window_workspace_focus
             SET active_task_id = NULL, updated_at = ?
             WHERE window_id = ? AND workspace_id = ?`,
            input.now,
            windowId,
            task.workspace_id
          )
        }
      }
      return {
        ...readHierarchyResult(tx, input.windowId),
        disposedSessionIds
      }
    }).result
  }

  activateTask(input: ActivateTaskInput): WorkspaceHierarchyResult {
    return this.#database.transaction((tx) => {
      registerWindow(tx, input.windowId, input.now)
      activateTaskInTransaction(tx, input.windowId, input.taskId, input.now)
      return readHierarchyResult(tx, input.windowId)
    })
  }

  recordSessionInteraction(
    command: DomainCommandMetadata,
    input: RecordSessionInteractionInput
  ): { sessionId: string; taskId: string; workspaceId: string; lastOpenedAt: number } {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const owner = requireRow<{ task_id: string; workspace_id: string }>(tx.get(
        `SELECT sessions.task_id, tasks.workspace_id
         FROM sessions JOIN tasks ON tasks.id = sessions.task_id
         WHERE sessions.id = ? AND sessions.archived_at IS NULL AND tasks.archived_at IS NULL`,
        input.sessionId
      ), 'Session')
      tx.run('UPDATE tasks SET last_opened_at = ? WHERE id = ?', input.now, owner.task_id)
      tx.run('UPDATE workspaces SET last_opened_at = ? WHERE id = ?', input.now, owner.workspace_id)
      const result = {
        sessionId: input.sessionId,
        taskId: owner.task_id,
        workspaceId: owner.workspace_id,
        lastOpenedAt: input.now
      }
      emit({
        eventId: `${command.commandId}:navigation-recency`,
        eventType: 'navigation.recency-changed',
        aggregateType: 'task-navigation',
        aggregateId: owner.task_id,
        workspaceId: owner.workspace_id,
        taskId: owner.task_id,
        sessionId: input.sessionId,
        payload: result,
        occurredAt: input.now
      })
      return result
    }).result
  }

  setTaskPinned(command: DomainCommandMetadata, input: SetTaskPinnedInput): Task {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const row = requireRow<TaskRow>(tx.get('SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL', input.taskId), 'Task')
      const nextKey = input.pinned ? nextPinnedKey(tx, 'tasks', row.workspace_id) : ''
      tx.run(`UPDATE tasks SET is_pinned = ?, pin_sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?`,
        input.pinned ? 1 : 0, nextKey, input.now, input.taskId)
      const task = mapTask({ ...row, is_pinned: input.pinned ? 1 : 0, pin_sort_key: nextKey,
        updated_at: input.now, version: row.version + 1 })
      emit({ eventId: `${command.commandId}:task-pinned`, eventType: 'task.pin-changed', aggregateType: 'task',
        aggregateId: row.id, workspaceId: row.workspace_id, taskId: row.id,
        payload: { isPinned: input.pinned, pinSortKey: nextKey }, occurredAt: input.now })
      return task
    }).result
  }

  reorderPinnedTask(command: DomainCommandMetadata, input: ReorderPinnedTaskInput): Task[] {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const rows = tx.all<TaskRow>(
        `SELECT * FROM tasks WHERE workspace_id = ? AND archived_at IS NULL AND is_pinned = 1 ORDER BY pin_sort_key, id`,
        input.workspaceId
      )
      const order = moveBefore(rows.map(({ id }) => id), input.taskId, input.beforeTaskId)
      order.forEach((id, index) => tx.run(
        'UPDATE tasks SET pin_sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?',
        sortKey(index), input.now, id
      ))
      emit({ eventId: `${command.commandId}:task-pin-order`, eventType: 'task.pin-order-changed',
        aggregateType: 'workspace', aggregateId: input.workspaceId, workspaceId: input.workspaceId,
        payload: { taskIds: order }, occurredAt: input.now })
      return order.map((id) => mapTask(requireRow<TaskRow>(tx.get('SELECT * FROM tasks WHERE id = ?', id), 'Task')))
    }).result
  }

  createScene(
    command: DomainCommandMetadata,
    input: CreateSceneWorkflowInput
  ): WorkspaceHierarchyResult {
    const ids = createHierarchyIds()
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const task = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL',
        input.taskId
      ), 'Task')
      const workspace = requireRow<WorkspaceRow>(tx.get(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL',
        task.workspace_id
      ), 'Workspace')
      assertWorkspacePathAvailable(tx, workspace.id)
      const sceneOrdinal = tx.get<{ count: number }>(
        'SELECT COUNT(*) AS count FROM scenes WHERE task_id = ? AND archived_at IS NULL',
        input.taskId
      )?.count ?? 0
      const sceneName = `Shell · ${workspace.root_directory}`
      tx.run(
        `INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, title_pinned, sort_key,
           layout_revision, created_at, updated_at
         ) VALUES (?, ?, ?, 'tile', ?, 0, ?, 1, ?, ?)`,
        ids.sceneId,
        input.taskId,
        sceneName,
        ids.rootNodeId,
        sortKey(sceneOrdinal),
        input.now,
        input.now
      )
      tx.run(
        `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
         VALUES (?, ?, 'root', 0, ?)`,
        ids.rootNodeId,
        ids.sceneId,
        input.now
      )
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, 1)`,
        ids.sessionId,
        input.taskId,
        task.execution_context_id,
        input.now,
        input.now,
        input.now
      )
      tx.run(
        `INSERT INTO session_mounts (
           id, scene_id, scene_node_id, session_id, created_at
         ) VALUES (?, ?, ?, ?, ?)`,
        ids.mountId,
        ids.sceneId,
        ids.rootNodeId,
        ids.sessionId,
        input.now
      )
      activateSceneInTransaction(tx, input.windowId, ids.sceneId, input.now)
      const scene = mapScene(requireRow<SceneRow>(
        tx.get('SELECT * FROM scenes WHERE id = ?', ids.sceneId), 'Scene'
      ))
      const session = mapSession(requireRow<SessionRow>(
        tx.get('SELECT * FROM sessions WHERE id = ?', ids.sessionId), 'Session'
      ))
      const mount = mapMount(requireRow<MountRow>(
        tx.get('SELECT * FROM session_mounts WHERE id = ?', ids.mountId), 'SessionMount'
      ))
      emit({
        eventId: `${command.commandId}:scene-created`, eventType: 'scene.created',
        aggregateType: 'scene', aggregateId: scene.id,
        workspaceId: workspace.id, taskId: task.id, payload: scene,
        occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:session-created`, eventType: 'session.created',
        aggregateType: 'session', aggregateId: session.id,
        workspaceId: workspace.id, taskId: task.id, sessionId: session.id,
        payload: session, occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:session-mounted`, eventType: 'scene.session-mounted',
        aggregateType: 'scene', aggregateId: scene.id,
        workspaceId: workspace.id, taskId: task.id, sessionId: session.id,
        payload: mount, occurredAt: input.now
      })
      return readHierarchyResult(tx, input.windowId)
    }).result
  }

  renameScene(
    command: DomainCommandMetadata,
    input: RenameSceneWorkflowInput
  ): Scene {
    const name = requiredTrimmed(input.name, 'Scene title')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const scene = requireRow<SceneRow>(tx.get(
        'SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL',
        input.sceneId
      ), 'Scene')
      if (tx.get(
        `SELECT id FROM scenes
         WHERE task_id = ? AND name = ? AND title_pinned = 1
           AND archived_at IS NULL AND id <> ?`,
        scene.task_id,
        name,
        input.sceneId
      )) {
        throw new Error('当前事项下已存在同名页签')
      }
      tx.run(
        `UPDATE scenes SET name = ?, title_pinned = 1, updated_at = ? WHERE id = ?`,
        name,
        input.now,
        input.sceneId
      )
      const renamed = mapScene({
        ...scene,
        name,
        title_pinned: 1,
        updated_at: input.now
      })
      emit({
        eventId: `${command.commandId}:scene-renamed`, eventType: 'scene.renamed',
        aggregateType: 'scene', aggregateId: scene.id, taskId: scene.task_id,
        payload: { name, titlePinned: true }, occurredAt: input.now
      })
      return renamed
    }).result
  }

  reorderScene(
    command: DomainCommandMetadata,
    input: ReorderSceneWorkflowInput
  ): WorkspaceHierarchyResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const scene = requireRow<{ task_id: string }>(tx.get(
        'SELECT task_id FROM scenes WHERE id = ? AND archived_at IS NULL', input.sceneId
      ), 'Scene')
      const taskId = scene.task_id
      const scenes = tx.all<{ id: string }>(
        `SELECT id FROM scenes WHERE task_id = ? AND archived_at IS NULL
         ORDER BY sort_key, created_at, id`,
        taskId
      ).map(({ id }) => id)
      if (!scenes.includes(input.sceneId)) throw new Error('Scene must belong to the Task')
      if (input.beforeSceneId !== undefined && !scenes.includes(input.beforeSceneId)) {
        throw new Error('before Scene must belong to the Task')
      }
      const order = scenes.filter((id) => id !== input.sceneId)
      const target = input.beforeSceneId === undefined ? order.length : order.indexOf(input.beforeSceneId)
      order.splice(target < 0 ? order.length : target, 0, input.sceneId)
      for (const [index, sceneId] of order.entries()) {
        tx.run(
          'UPDATE scenes SET sort_key = ?, updated_at = ? WHERE id = ?',
          sortKey(index), input.now, sceneId
        )
      }
      const task = requireRow<{ workspace_id: string }>(tx.get(
        'SELECT workspace_id FROM tasks WHERE id = ?', taskId
      ), 'Task')
      emit({
        eventId: `${command.commandId}:scene-order-changed`,
        eventType: 'task.scene-order-changed',
        aggregateType: 'task', aggregateId: taskId,
        workspaceId: task.workspace_id, taskId,
        payload: { sceneOrder: order }, occurredAt: input.now
      })
      activateSceneInTransaction(tx, input.windowId, input.sceneId, input.now)
      return readHierarchyResult(tx, input.windowId)
    }).result
  }

  closeScene(
    command: DomainCommandMetadata,
    input: CloseSceneWorkflowInput
  ): CloseSceneResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const scene = requireRow<SceneRow>(tx.get(
        'SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL',
        input.sceneId
      ), 'Scene')
      const task = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL',
        scene.task_id
      ), 'Task')
      const scenes = tx.all<{ id: string }>(
        `SELECT id FROM scenes WHERE task_id = ? AND archived_at IS NULL
         ORDER BY sort_key, created_at, id`,
        task.id
      ).map(({ id }) => id)
      const activeTaskCount = tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM tasks
         WHERE workspace_id = ? AND archived_at IS NULL`,
        task.workspace_id
      )?.count ?? 0
      if (scenes.length === 1 && activeTaskCount === 1) {
        return {
          ...readHierarchyResult(tx, input.windowId),
          action: 'hide-window' as const,
          disposedSessionIds: []
        }
      }
      const affectedWorkCount = tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM session_mounts
         JOIN sessions ON sessions.id = session_mounts.session_id
         WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL
           AND sessions.work_status IN ('starting', 'running', 'needs-input')`,
        input.sceneId
      )?.count ?? 0
      if (
        affectedWorkCount > 0 &&
        input.confirmedIntent !== `close-scene:${input.sceneId}`
      ) {
        throw new Error('Scene close intent is stale')
      }

      const disposedSessionIds = tx.all<{ id: string }>(
        `SELECT sessions.id FROM session_mounts
         JOIN sessions ON sessions.id = session_mounts.session_id
         WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL`,
        input.sceneId
      ).map(({ id }) => id)
      tx.run(
        `UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE id IN (SELECT session_id FROM session_mounts WHERE scene_id = ?)
           AND archived_at IS NULL`,
        input.now,
        input.now,
        input.sceneId
      )
      tx.run(
        'UPDATE scenes SET archived_at = ?, updated_at = ? WHERE id = ?',
        input.now,
        input.now,
        input.sceneId
      )
      tx.run('DELETE FROM window_scene_focus WHERE scene_id = ?', input.sceneId)
      emit({
        eventId: `${command.commandId}:scene-archived`, eventType: 'scene.archived',
        aggregateType: 'scene', aggregateId: scene.id,
        workspaceId: task.workspace_id, taskId: task.id,
        payload: { archivedAt: input.now, disposedSessionIds }, occurredAt: input.now
      })

      if (scenes.length > 1) {
        const remaining = scenes.filter((id) => id !== input.sceneId)
        const closedIndex = scenes.indexOf(input.sceneId)
        const nextSceneId = remaining[Math.min(closedIndex, remaining.length - 1)]!
        activateSceneInTransaction(tx, input.windowId, nextSceneId, input.now)
      } else {
        tx.run(
          `UPDATE window_task_focus SET active_scene_id = NULL, updated_at = ?
           WHERE task_id = ? AND active_scene_id = ?`,
          input.now, task.id, input.sceneId
        )
        const nextTask = preferredTask(tx, input.windowId, task.workspace_id)
        if (nextTask && nextTask.id !== task.id) {
          activateTaskInTransaction(tx, input.windowId, nextTask.id, input.now)
        }
      }
      return {
        ...readHierarchyResult(tx, input.windowId),
        action: 'closed' as const,
        disposedSessionIds
      }
    }).result
  }

  activateScene(input: ActivateSceneInput): WorkspaceHierarchyResult {
    return this.#database.transaction((tx) => {
      registerWindow(tx, input.windowId, input.now)
      activateSceneInTransaction(tx, input.windowId, input.sceneId, input.now)
      return readHierarchyResult(tx, input.windowId)
    })
  }

  reopenScene(
    command: DomainCommandMetadata,
    input: ReopenSceneWorkflowInput
  ): WorkspaceHierarchyResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const scene = requireRow<SceneRow>(tx.get(
        'SELECT * FROM scenes WHERE id = ? AND archived_at IS NOT NULL', input.sceneId
      ), 'Closed Scene')
      const task = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL', scene.task_id
      ), 'Task')
      tx.run(
        'UPDATE scenes SET archived_at = NULL, updated_at = ? WHERE id = ?',
        input.now, scene.id
      )
      activateSceneInTransaction(tx, input.windowId, scene.id, input.now)
      emit({
        eventId: `${command.commandId}:scene-reopened`, eventType: 'scene.reopened',
        aggregateType: 'scene', aggregateId: scene.id,
        workspaceId: task.workspace_id, taskId: task.id,
        payload: { reopenedAt: input.now }, occurredAt: input.now
      })
      return readHierarchyResult(tx, input.windowId)
    }).result
  }

  splitSession(
    command: DomainCommandMetadata,
    input: SplitSessionWorkflowInput
  ): WorkspaceHierarchyResult {
    const ids = createHierarchyIds()
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const scene = requireRow<SceneRow>(tx.get(
        'SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL', input.sceneId
      ), 'Scene')
      const source = requireRow<SessionRow>(tx.get(
        `SELECT sessions.* FROM sessions
         JOIN session_mounts ON session_mounts.session_id = sessions.id
         WHERE sessions.id = ? AND session_mounts.scene_id = ? AND sessions.archived_at IS NULL`,
        input.sourceSessionId, input.sceneId
      ), 'Session')
      const sourceMount = requireRow<MountRow>(tx.get(
        `SELECT * FROM session_mounts
         WHERE scene_id = ? AND session_id = ? ORDER BY created_at LIMIT 1`,
        input.sceneId, input.sourceSessionId
      ), 'SessionMount')
      const sourceNode = requireRow<{
        id: string; parent_node_id: string | null; ordinal: number
      }>(tx.get(
        'SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?',
        sourceMount.scene_node_id
      ), 'SceneNode')
      const task = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL', scene.task_id
      ), 'Task')
      assertWorkspacePathAvailable(tx, task.workspace_id)
      tx.run(
        `INSERT INTO scene_nodes (
           id, scene_id, parent_node_id, kind, direction, ordinal, created_at
         ) VALUES (?, ?, ?, 'split', ?, ?, ?)`,
        ids.secondaryNodeId, input.sceneId, sourceNode.parent_node_id,
        input.direction, sourceNode.ordinal, input.now
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
         ) VALUES (?, ?, ?, 'mount', ?, ?)`,
        ids.rootNodeId, input.sceneId, ids.secondaryNodeId, 1, input.now
      )
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, 1)`,
        ids.sessionId, task.id, source.execution_context_id,
        input.now, input.now, input.now
      )
      tx.run(
        `INSERT INTO session_mounts (
           id, scene_id, scene_node_id, session_id, created_at
         ) VALUES (?, ?, ?, ?, ?)`,
        ids.mountId, input.sceneId, ids.rootNodeId, ids.sessionId, input.now
      )
      tx.run(
        `UPDATE scenes SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
           layout_revision = layout_revision + 1, updated_at = ? WHERE id = ?`,
        sourceNode.id, ids.secondaryNodeId, input.now, input.sceneId
      )
      activateSessionInTransaction(tx, input.windowId, ids.sessionId, input.now)
      const session = mapSession(requireRow<SessionRow>(tx.get(
        'SELECT * FROM sessions WHERE id = ?', ids.sessionId
      ), 'Session'))
      emit({
        eventId: `${command.commandId}:session-created`, eventType: 'session.created',
        aggregateType: 'session', aggregateId: session.id,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: session.id,
        payload: session, occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:session-split`, eventType: 'scene.session-split',
        aggregateType: 'scene', aggregateId: scene.id,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: session.id,
        payload: { mountId: ids.mountId, sourceSessionId: source.id, direction: input.direction },
        occurredAt: input.now
      })
      return readHierarchyResult(tx, input.windowId)
    }).result
  }

  forkSession(
    command: DomainCommandMetadata,
    input: ForkSessionWorkflowInput
  ): WorkspaceHierarchyResult {
    const ids = createHierarchyIds()
    const relationId = randomUUID()
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const scene = requireRow<SceneRow>(tx.get(
        'SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL', input.sceneId
      ), 'Scene')
      const source = requireRow<SessionRow>(tx.get(
        `SELECT sessions.* FROM sessions
         JOIN session_mounts ON session_mounts.session_id = sessions.id
         WHERE sessions.id = ? AND session_mounts.scene_id = ? AND sessions.archived_at IS NULL`,
        input.sourceSessionId, input.sceneId
      ), 'Session')
      const sourceMount = requireRow<MountRow>(tx.get(
        `SELECT * FROM session_mounts
         WHERE scene_id = ? AND session_id = ? ORDER BY created_at LIMIT 1`,
        input.sceneId, input.sourceSessionId
      ), 'SessionMount')
      const sourceBinding = tx.get<{
        provider_session_id: string
      }>(
        `SELECT provider_session_id FROM provider_bindings
         WHERE session_id = ? AND provider = 'claude-code'
           AND resume_state IN ('available', 'resumed')
           AND validated_at IS NOT NULL AND invalidated_at IS NULL
         ORDER BY validated_at DESC LIMIT 1`,
        input.sourceSessionId
      )
      if (
        source.kind !== 'claude-code' ||
        sourceMount.scene_window_id !== null ||
        !sourceBinding
      ) {
        throw new Error('only resumable Claude Sessions can be forked')
      }
      const sourceNode = requireRow<{
        id: string; parent_node_id: string | null; ordinal: number
      }>(tx.get(
        'SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?',
        sourceMount.scene_node_id
      ), 'SceneNode')
      const task = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL', scene.task_id
      ), 'Task')
      assertWorkspacePathAvailable(tx, task.workspace_id)

      tx.run(
        `INSERT INTO scene_nodes (
           id, scene_id, parent_node_id, kind, direction, ordinal, created_at
         ) VALUES (?, ?, ?, 'split', 'horizontal', ?, ?)`,
        ids.secondaryNodeId, input.sceneId, sourceNode.parent_node_id,
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
        ids.rootNodeId, input.sceneId, ids.secondaryNodeId, input.now
      )
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, 'claude-code', 'created', 'Claude', ?, ?, ?, ?, 1)`,
        ids.sessionId, task.id, source.execution_context_id,
        source.cwd, input.now, input.now, input.now
      )
      tx.run(
        `INSERT INTO session_mounts (
           id, scene_id, scene_node_id, session_id, created_at
         ) VALUES (?, ?, ?, ?, ?)`,
        ids.mountId, input.sceneId, ids.rootNodeId, ids.sessionId, input.now
      )
      tx.run(
        `UPDATE scenes SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
           layout_revision = layout_revision + 1, updated_at = ? WHERE id = ?`,
        sourceNode.id, ids.secondaryNodeId, input.now, input.sceneId
      )
      tx.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', ?)`,
        ids.sessionId, source.id, sourceBinding.provider_session_id, input.now
      )
      const relationInsertion = tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id,
           to_session_id, relation_kind, metadata_json, command_id, occurred_at
         ) VALUES (?, ?, 'created', ?, ?, ?, 'forked-from', '{}', ?, ?)`,
        `${command.commandId}:fork-relation-created`, relationId, task.id,
        ids.sessionId, source.id, command.commandId, input.now
      )
      tx.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           metadata_json, created_at, updated_at, source_event_sequence
         ) VALUES (?, ?, ?, ?, 'forked-from', '{}', ?, ?, ?)`,
        relationId, task.id, ids.sessionId, source.id, input.now, input.now,
        Number(relationInsertion.lastInsertRowid)
      )
      activateSessionInTransaction(tx, input.windowId, ids.sessionId, input.now)
      const session = mapSession(requireRow<SessionRow>(tx.get(
        'SELECT * FROM sessions WHERE id = ?', ids.sessionId
      ), 'Session'))
      emit({
        eventId: `${command.commandId}:session-created`, eventType: 'session.created',
        aggregateType: 'session', aggregateId: session.id,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: session.id,
        payload: session, occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:session-forked`, eventType: 'scene.session-forked',
        aggregateType: 'scene', aggregateId: scene.id,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: session.id,
        payload: {
          mountId: ids.mountId, sourceSessionId: source.id,
          direction: 'horizontal', relationId
        },
        occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:domain-relation-created`,
        eventType: 'session-relation.created', aggregateType: 'session-relation',
        aggregateId: relationId, workspaceId: task.workspace_id, taskId: task.id,
        sessionId: session.id,
        payload: {
          id: relationId, taskId: task.id, fromSessionId: session.id,
          toSessionId: source.id, kind: 'forked-from', metadata: {},
          createdAt: input.now, updatedAt: input.now
        },
        occurredAt: input.now
      })
      return readHierarchyResult(tx, input.windowId)
    }).result
  }

  deleteSession(
    command: DomainCommandMetadata,
    input: DeleteSessionWorkflowInput
  ): DeleteSessionResult {
    const replacementIds = createHierarchyIds()
    return this.#transactions.execute(command, (context) => {
      const { tx, emit } = context
      registerWindow(tx, input.windowId, input.now)
      const session = requireRow<SessionRow>(tx.get(
        'SELECT * FROM sessions WHERE id = ? AND archived_at IS NULL', input.sessionId
      ), 'Session')
      const task = requireRow<TaskRow>(tx.get(
        'SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL', session.task_id
      ), 'Task')
      const workspaceSessionCount = tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM sessions
         JOIN tasks ON tasks.id = sessions.task_id
         WHERE tasks.workspace_id = ? AND sessions.archived_at IS NULL
           AND tasks.archived_at IS NULL`,
        task.workspace_id
      )?.count ?? 0
      const structuralChildCount = tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM session_relations_current
         WHERE to_session_id = ? AND relation_kind IN ('derived-from', 'forked-from')`,
        input.sessionId
      )?.count ?? 0
      const hasActiveWork = tx.get<{ work_status: string }>(
        'SELECT work_status FROM sessions WHERE id = ?', input.sessionId
      )?.work_status
      const requiresConfirmation = workspaceSessionCount === 1 || structuralChildCount > 0 ||
        hasActiveWork === 'starting' || hasActiveWork === 'running' || hasActiveWork === 'needs-input'
      if (
        requiresConfirmation &&
        input.confirmedIntent !== `delete-session:${input.sessionId}`
      ) {
        throw new Error('Session deletion intent is stale')
      }
      const mount = requireRow<MountRow>(tx.get(
        'SELECT * FROM session_mounts WHERE session_id = ? ORDER BY created_at LIMIT 1',
        input.sessionId
      ), 'SessionMount')
      const scene = requireRow<SceneRow>(tx.get(
        'SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL', mount.scene_id
      ), 'Scene')
      const sceneSessionCount = tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM session_mounts
         JOIN sessions ON sessions.id = session_mounts.session_id
         WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL`,
        scene.id
      )?.count ?? 0
      tx.run(
        `UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE id = ?`, input.now, input.now, input.sessionId
      )
      emit({
        eventId: `${command.commandId}:session-archived`, eventType: 'session.archived',
        aggregateType: 'session', aggregateId: session.id,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: session.id,
        payload: { archivedAt: input.now }, occurredAt: input.now
      })

      let outcome: DeleteSessionResult['outcome'] = 'scene-remains'
      if (sceneSessionCount > 1) {
        tx.run('DELETE FROM session_mounts WHERE id = ?', mount.id)
        const sibling = preferredSession(tx, input.windowId, scene.id)
        if (sibling) activateSessionInTransaction(tx, input.windowId, sibling.id, input.now)
      } else {
        outcome = 'scene-archives'
        tx.run('UPDATE scenes SET archived_at = ?, updated_at = ? WHERE id = ?', input.now, input.now, scene.id)
        tx.run('DELETE FROM window_scene_focus WHERE scene_id = ?', scene.id)
        const otherScene = preferredScene(tx, input.windowId, task.id)
        if (otherScene) {
          activateSceneInTransaction(tx, input.windowId, otherScene.id, input.now)
        } else {
          tx.run(
            `UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
             WHERE id = ?`, input.now, input.now, task.id
          )
          tx.run('DELETE FROM window_task_focus WHERE task_id = ?', task.id)
          tx.run('DELETE FROM window_task_placements WHERE task_id = ?', task.id)
          const workspace = requireRow<WorkspaceRow>(tx.get(
            'SELECT * FROM workspaces WHERE id = ?', task.workspace_id
          ), 'Workspace')
          const order = parseStringArray(workspace.task_order_json).filter((id) => id !== task.id)
          tx.run(
            `UPDATE workspaces SET task_order_json = ?, updated_at = ?, version = version + 1
             WHERE id = ?`, JSON.stringify(order), input.now, task.workspace_id
          )
          const otherTask = preferredTask(tx, input.windowId, task.workspace_id)
          if (otherTask) {
            activateTaskInTransaction(tx, input.windowId, otherTask.id, input.now)
          } else if (workspacePathIsAvailable(tx, task.workspace_id)) {
            const refreshed = requireRow<WorkspaceRow>(tx.get(
              'SELECT * FROM workspaces WHERE id = ?', task.workspace_id
            ), 'Workspace')
            this.#createTaskHierarchy(context, {
              ids: replacementIds, windowId: input.windowId, workspace: refreshed,
              title: '默认', commandId: command.commandId, now: input.now
            })
            outcome = 'default-task-created'
          }
        }
      }
      return {
        ...readHierarchyResult(tx, input.windowId),
        outcome,
        disposedSessionIds: [input.sessionId]
      }
    }).result
  }

  activateSession(input: ActivateSessionInput): WorkspaceHierarchyResult {
    return this.#database.transaction((tx) => {
      registerWindow(tx, input.windowId, input.now)
      activateSessionInTransaction(tx, input.windowId, input.sessionId, input.now)
      return readHierarchyResult(tx, input.windowId)
    })
  }

  #createCompleteHierarchy(
    context: DomainMutationContext,
    input: {
      ids: HierarchyIds
      windowId: string
      name: string
      rootDirectory: string
      taskTitle: string
      commandId: string
      now: number
      lastOpenedAt?: number
      isDefault?: boolean
      isPinned?: boolean
    }
  ): Workspace {
    const { tx, emit } = context
    const name = requiredTrimmed(input.name, 'Workspace name')
    const rootDirectory = resolve(input.rootDirectory)
    const { ids } = input

    tx.run(
      `INSERT INTO workspaces (
         id, name, root_directory, path_identity, task_order_json,
         created_at, updated_at, version, is_default, is_pinned, pin_sort_key, last_opened_at
       ) VALUES (?, ?, ?, ?, ?, ?, ?, 2, ?, ?, ?, ?)`,
      ids.workspaceId,
      name,
      rootDirectory,
      `path:${rootDirectory}`,
      JSON.stringify([ids.taskId]),
      input.now,
      input.now,
      input.isDefault ? 1 : 0,
      input.isPinned ? 1 : 0,
      input.isPinned ? 'a0' : '',
      input.lastOpenedAt ?? input.now
    )
    tx.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES (?, ?, 'plain-directory', ?, ?)`,
      ids.executionContextId,
      ids.workspaceId,
      rootDirectory,
      input.now
    )
    tx.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, sort_key,
         created_at, updated_at, version, last_opened_at
       ) VALUES (?, ?, ?, ?, 'active', 'a0', ?, ?, 1, ?)`,
      ids.taskId,
      ids.workspaceId,
      ids.executionContextId,
      input.taskTitle,
      input.now,
      input.now,
      input.now
    )
    tx.run(
      `INSERT INTO scenes (
         id, task_id, name, mode, root_node_id, title_pinned, sort_key,
         layout_revision, created_at, updated_at
       ) VALUES (?, ?, ?, 'tile', ?, 0, 'a0', 1, ?, ?)`,
      ids.sceneId,
      ids.taskId,
      'Shell · ' + rootDirectory,
      ids.rootNodeId,
      input.now,
      input.now
    )
    tx.run(
      `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
       VALUES (?, ?, 'root', 0, ?)`,
      ids.rootNodeId,
      ids.sceneId,
      input.now
    )
    tx.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title,
         created_at, updated_at, last_activity_at, version
       ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, 1)`,
      ids.sessionId,
      ids.taskId,
      ids.executionContextId,
      input.now,
      input.now,
      input.now
    )
    tx.run(
      `INSERT INTO session_mounts (
         id, scene_id, scene_node_id, session_id, created_at
       ) VALUES (?, ?, ?, ?, ?)`,
      ids.mountId,
      ids.sceneId,
      ids.rootNodeId,
      ids.sessionId,
      input.now
    )
    tx.run(
      `INSERT INTO window_task_placements (window_id, task_id, ordinal, updated_at)
       VALUES (?, ?, 0, ?)`,
      input.windowId,
      ids.taskId,
      input.now
    )
    activateWorkspaceInTransaction(tx, input.windowId, ids.workspaceId, input.now)

    const workspace = mapWorkspace(requireRow<WorkspaceRow>(
      tx.get('SELECT * FROM workspaces WHERE id = ?', ids.workspaceId),
      'Workspace'
    ))
    const task = mapTask(requireRow<TaskRow>(
      tx.get('SELECT * FROM tasks WHERE id = ?', ids.taskId),
      'Task'
    ))
    const scene = mapScene(requireRow<SceneRow>(
      tx.get('SELECT * FROM scenes WHERE id = ?', ids.sceneId),
      'Scene'
    ))
    const session = mapSession(requireRow<SessionRow>(
      tx.get('SELECT * FROM sessions WHERE id = ?', ids.sessionId),
      'Session'
    ))
    const mount = mapMount(requireRow<MountRow>(
      tx.get('SELECT * FROM session_mounts WHERE id = ?', ids.mountId),
      'SessionMount'
    ))

    emitHierarchyCreated(input.commandId, {
      emit,
      workspace,
      task,
      scene,
      session,
      mount,
      now: input.now
    })
    return workspace
  }

  #createTaskHierarchy(
    context: DomainMutationContext,
    input: {
      ids: HierarchyIds
      windowId: string
      workspace: WorkspaceRow
      title: string
      commandId: string
      now: number
      lastOpenedAt?: number
    }
  ): Task {
    const { tx } = context
    const { ids } = input
    const activeOrder = parseStringArray(input.workspace.task_order_json)
      .filter((taskId) => Boolean(tx.get(
        'SELECT id FROM tasks WHERE id = ? AND archived_at IS NULL', taskId
      )))
    activeOrder.push(ids.taskId)
    const ordinal = activeOrder.length - 1

    tx.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES (?, ?, 'plain-directory', ?, ?)`,
      ids.executionContextId,
      input.workspace.id,
      input.workspace.root_directory,
      input.now
    )
    tx.run(
      `INSERT INTO tasks (
         id, workspace_id, execution_context_id, title, status, sort_key,
         created_at, updated_at, version, last_opened_at
       ) VALUES (?, ?, ?, ?, 'active', ?, ?, ?, 1, ?)`,
      ids.taskId,
      input.workspace.id,
      ids.executionContextId,
      requiredTrimmed(input.title, 'Task title'),
      sortKey(ordinal),
      input.now,
      input.now,
      input.lastOpenedAt ?? input.now
    )
    tx.run(
      `INSERT INTO scenes (
         id, task_id, name, mode, root_node_id, title_pinned, sort_key,
         layout_revision, created_at, updated_at
       ) VALUES (?, ?, ?, 'tile', ?, 0, 'a0', 1, ?, ?)`,
      ids.sceneId,
      ids.taskId,
      `Shell · ${input.workspace.root_directory}`,
      ids.rootNodeId,
      input.now,
      input.now
    )
    tx.run(
      `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
       VALUES (?, ?, 'root', 0, ?)`,
      ids.rootNodeId,
      ids.sceneId,
      input.now
    )
    tx.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title,
         created_at, updated_at, last_activity_at, version
       ) VALUES (?, ?, ?, 'shell', 'created', 'Shell', ?, ?, ?, 1)`,
      ids.sessionId,
      ids.taskId,
      ids.executionContextId,
      input.now,
      input.now,
      input.now
    )
    tx.run(
      `INSERT INTO session_mounts (
         id, scene_id, scene_node_id, session_id, created_at
       ) VALUES (?, ?, ?, ?, ?)`,
      ids.mountId,
      ids.sceneId,
      ids.rootNodeId,
      ids.sessionId,
      input.now
    )
    tx.run(
      `INSERT INTO window_task_placements (window_id, task_id, ordinal, updated_at)
       VALUES (?, ?, ?, ?)`,
      input.windowId,
      ids.taskId,
      ordinal,
      input.now
    )
    tx.run(
      `UPDATE workspaces SET task_order_json = ?, updated_at = ?, version = version + 1
       WHERE id = ?`,
      JSON.stringify(activeOrder),
      input.now,
      input.workspace.id
    )
    activateTaskInTransaction(tx, input.windowId, ids.taskId, input.now)

    const task = mapTask(requireRow<TaskRow>(
      tx.get('SELECT * FROM tasks WHERE id = ?', ids.taskId), 'Task'
    ))
    const scene = mapScene(requireRow<SceneRow>(
      tx.get('SELECT * FROM scenes WHERE id = ?', ids.sceneId), 'Scene'
    ))
    const session = mapSession(requireRow<SessionRow>(
      tx.get('SELECT * FROM sessions WHERE id = ?', ids.sessionId), 'Session'
    ))
    const mount = mapMount(requireRow<MountRow>(
      tx.get('SELECT * FROM session_mounts WHERE id = ?', ids.mountId), 'SessionMount'
    ))
    emitTaskHierarchyCreated(input.commandId, {
      emit: context.emit,
      workspaceId: input.workspace.id,
      task,
      scene,
      session,
      mount,
      now: input.now
    })
    context.emit({
      eventId: `${input.commandId}:workspace-task-order`,
      eventType: 'workspace.task-order-changed',
      aggregateType: 'workspace',
      aggregateId: input.workspace.id,
      workspaceId: input.workspace.id,
      payload: { taskOrder: activeOrder },
      occurredAt: input.now
    })
    return task
  }
}

function emitHierarchyCreated(
  prefix: string,
  input: {
    emit: DomainMutationContext['emit']
    workspace: Workspace
    task: Task
    scene: Scene
    session: Session
    mount: SessionMount
    now: number
  }
): void {
  const common = { workspaceId: input.workspace.id, occurredAt: input.now }
  input.emit({
    eventId: `${prefix}:${input.workspace.id}:created`, eventType: 'workspace.created',
    aggregateType: 'workspace', aggregateId: input.workspace.id,
    ...common, payload: input.workspace
  })
  input.emit({
    eventId: `${prefix}:${input.task.id}:created`, eventType: 'task.created',
    aggregateType: 'task', aggregateId: input.task.id, taskId: input.task.id,
    ...common, payload: input.task
  })
  input.emit({
    eventId: `${prefix}:${input.scene.id}:created`, eventType: 'scene.created',
    aggregateType: 'scene', aggregateId: input.scene.id, taskId: input.task.id,
    ...common, payload: input.scene
  })
  input.emit({
    eventId: `${prefix}:${input.session.id}:created`, eventType: 'session.created',
    aggregateType: 'session', aggregateId: input.session.id, taskId: input.task.id,
    sessionId: input.session.id, ...common, payload: input.session
  })
  input.emit({
    eventId: `${prefix}:${input.mount.id}:created`, eventType: 'scene.session-mounted',
    aggregateType: 'scene', aggregateId: input.scene.id, taskId: input.task.id,
    sessionId: input.session.id, ...common, payload: input.mount
  })
}

function emitTaskHierarchyCreated(
  prefix: string,
  input: {
    emit: DomainMutationContext['emit']
    workspaceId: string
    task: Task
    scene: Scene
    session: Session
    mount: SessionMount
    now: number
  }
): void {
  const common = { workspaceId: input.workspaceId, occurredAt: input.now }
  input.emit({
    eventId: `${prefix}:${input.task.id}:created`, eventType: 'task.created',
    aggregateType: 'task', aggregateId: input.task.id, taskId: input.task.id,
    ...common, payload: input.task
  })
  input.emit({
    eventId: `${prefix}:${input.scene.id}:created`, eventType: 'scene.created',
    aggregateType: 'scene', aggregateId: input.scene.id, taskId: input.task.id,
    ...common, payload: input.scene
  })
  input.emit({
    eventId: `${prefix}:${input.session.id}:created`, eventType: 'session.created',
    aggregateType: 'session', aggregateId: input.session.id, taskId: input.task.id,
    sessionId: input.session.id, ...common, payload: input.session
  })
  input.emit({
    eventId: `${prefix}:${input.mount.id}:created`, eventType: 'scene.session-mounted',
    aggregateType: 'scene', aggregateId: input.scene.id, taskId: input.task.id,
    sessionId: input.session.id, ...common, payload: input.mount
  })
}

export function registerWindow(tx: DatabaseTransaction, windowId: string, now: number): void {
  tx.run(
    `INSERT INTO app_windows (id, kind, state, created_at, updated_at)
     VALUES (?, 'main', 'visible', ?, ?)
     ON CONFLICT(id) DO UPDATE SET state = 'visible', updated_at = excluded.updated_at`,
    windowId,
    now,
    now
  )
  tx.run(
    `INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
     VALUES (?, NULL, ?)
     ON CONFLICT(window_id) DO NOTHING`,
    windowId,
    now
  )
}

function activateWorkspaceInTransaction(
  tx: DatabaseTransaction,
  windowId: string,
  workspaceId: string,
  now: number
): void {
  tx.run(
    `INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
     VALUES (?, ?, ?)
     ON CONFLICT(window_id) DO UPDATE SET
       active_workspace_id = excluded.active_workspace_id,
       updated_at = excluded.updated_at`,
    windowId,
    workspaceId,
    now
  )
  const task = preferredTask(tx, windowId, workspaceId)
  tx.run(
    `INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, workspace_id) DO UPDATE SET
       active_task_id = excluded.active_task_id,
       updated_at = excluded.updated_at`,
    windowId,
    workspaceId,
    task?.id ?? null,
    now
  )
  if (!task) return
  const scene = preferredScene(tx, windowId, task.id)
  tx.run(
    `INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, task_id) DO UPDATE SET
       active_scene_id = excluded.active_scene_id,
       updated_at = excluded.updated_at`,
    windowId,
    task.id,
    scene?.id ?? null,
    now
  )
  if (!scene) return
  const session = preferredSession(tx, windowId, scene.id)
  tx.run(
    `INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, scene_id) DO UPDATE SET
       active_session_id = excluded.active_session_id,
       updated_at = excluded.updated_at`,
    windowId,
    scene.id,
    session?.id ?? null,
    now
  )
}

function activateTaskInTransaction(
  tx: DatabaseTransaction,
  windowId: string,
  taskId: string,
  now: number
): void {
  const task = tx.get<{ workspace_id: string }>(
    'SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL',
    taskId
  )
  if (!task) throw new Error(`Task ${taskId} does not exist`)
  tx.run(
    `INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
     VALUES (?, ?, ?)
     ON CONFLICT(window_id) DO UPDATE SET
       active_workspace_id = excluded.active_workspace_id,
       updated_at = excluded.updated_at`,
    windowId,
    task.workspace_id,
    now
  )
  tx.run(
    `INSERT INTO window_workspace_focus (
       window_id, workspace_id, active_task_id, updated_at
     ) VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, workspace_id) DO UPDATE SET
       active_task_id = excluded.active_task_id,
       updated_at = excluded.updated_at`,
    windowId,
    task.workspace_id,
    taskId,
    now
  )
  const scene = preferredScene(tx, windowId, taskId)
  tx.run(
    `INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, task_id) DO UPDATE SET
       active_scene_id = excluded.active_scene_id,
       updated_at = excluded.updated_at`,
    windowId,
    taskId,
    scene?.id ?? null,
    now
  )
  if (!scene) return
  const session = preferredSession(tx, windowId, scene.id)
  tx.run(
    `INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, scene_id) DO UPDATE SET
       active_session_id = excluded.active_session_id,
       updated_at = excluded.updated_at`,
    windowId,
    scene.id,
    session?.id ?? null,
    now
  )
}

function activateSceneInTransaction(
  tx: DatabaseTransaction,
  windowId: string,
  sceneId: string,
  now: number
): void {
  const scene = tx.get<{ task_id: string; workspace_id: string }>(
    `SELECT scenes.task_id, tasks.workspace_id FROM scenes
     JOIN tasks ON tasks.id = scenes.task_id
     WHERE scenes.id = ? AND scenes.archived_at IS NULL AND tasks.archived_at IS NULL`,
    sceneId
  )
  if (!scene) throw new Error(`Scene ${sceneId} does not exist`)
  tx.run(
    `INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
     VALUES (?, ?, ?)
     ON CONFLICT(window_id) DO UPDATE SET
       active_workspace_id = excluded.active_workspace_id,
       updated_at = excluded.updated_at`,
    windowId,
    scene.workspace_id,
    now
  )
  tx.run(
    `INSERT INTO window_workspace_focus (
       window_id, workspace_id, active_task_id, updated_at
     ) VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, workspace_id) DO UPDATE SET
       active_task_id = excluded.active_task_id,
       updated_at = excluded.updated_at`,
    windowId,
    scene.workspace_id,
    scene.task_id,
    now
  )
  tx.run(
    `INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, task_id) DO UPDATE SET
       active_scene_id = excluded.active_scene_id,
       updated_at = excluded.updated_at`,
    windowId,
    scene.task_id,
    sceneId,
    now
  )
  const session = preferredSession(tx, windowId, sceneId)
  tx.run(
    `INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, scene_id) DO UPDATE SET
       active_session_id = excluded.active_session_id,
       updated_at = excluded.updated_at`,
    windowId,
    sceneId,
    session?.id ?? null,
    now
  )
}

export function activateSessionInTransaction(
  tx: DatabaseTransaction,
  windowId: string,
  sessionId: string,
  now: number
): void {
  const owner = tx.get<{ scene_id: string }>(
    `SELECT session_mounts.scene_id FROM session_mounts
     JOIN sessions ON sessions.id = session_mounts.session_id
     JOIN scenes ON scenes.id = session_mounts.scene_id
     WHERE sessions.id = ? AND sessions.archived_at IS NULL AND scenes.archived_at IS NULL
     ORDER BY session_mounts.created_at LIMIT 1`,
    sessionId
  )
  if (!owner) throw new Error(`Session ${sessionId} does not exist in an active Scene`)
  activateSceneInTransaction(tx, windowId, owner.scene_id, now)
  tx.run(
    `UPDATE window_scene_focus SET active_session_id = ?, updated_at = ?
     WHERE window_id = ? AND scene_id = ?`,
    sessionId, now, windowId, owner.scene_id
  )
}

function clearWindowNavigation(tx: DatabaseTransaction, windowId: string, now: number): void {
  tx.run(
    'UPDATE window_navigation SET active_workspace_id = NULL, updated_at = ? WHERE window_id = ?',
    now,
    windowId
  )
}

function preferredTask(
  tx: DatabaseTransaction,
  windowId: string,
  workspaceId: string
): { id: string } | undefined {
  return tx.get<{ id: string }>(
    `SELECT tasks.id FROM tasks
     LEFT JOIN window_workspace_focus focus
       ON focus.window_id = ? AND focus.workspace_id = tasks.workspace_id
     WHERE tasks.workspace_id = ? AND tasks.archived_at IS NULL
     ORDER BY (tasks.id = focus.active_task_id) DESC, tasks.sort_key, tasks.created_at
     LIMIT 1`,
    windowId,
    workspaceId
  )
}

function preferredScene(
  tx: DatabaseTransaction,
  windowId: string,
  taskId: string
): { id: string } | undefined {
  return tx.get<{ id: string }>(
    `SELECT scenes.id FROM scenes
     LEFT JOIN window_task_focus focus
       ON focus.window_id = ? AND focus.task_id = scenes.task_id
     WHERE scenes.task_id = ? AND scenes.archived_at IS NULL
     ORDER BY (scenes.id = focus.active_scene_id) DESC, scenes.sort_key, scenes.created_at
     LIMIT 1`,
    windowId,
    taskId
  )
}

function preferredSession(
  tx: DatabaseTransaction,
  windowId: string,
  sceneId: string
): { id: string } | undefined {
  return tx.get<{ id: string }>(
    `SELECT sessions.id FROM session_mounts
     JOIN sessions ON sessions.id = session_mounts.session_id
     LEFT JOIN window_scene_focus focus
       ON focus.window_id = ? AND focus.scene_id = session_mounts.scene_id
     WHERE session_mounts.scene_id = ? AND sessions.archived_at IS NULL
     ORDER BY (sessions.id = focus.active_session_id) DESC, session_mounts.created_at
     LIMIT 1`,
    windowId,
    sceneId
  )
}

export function readHierarchyResult(
  tx: DatabaseTransaction,
  windowId: string
): WorkspaceHierarchyResult {
  const navigation = readNavigation(tx, windowId)
  const workspace = navigation.activeWorkspaceId === undefined
    ? undefined
    : tx.get<WorkspaceRow>(
      'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL',
      navigation.activeWorkspaceId
    )
  const activeTaskId = workspace === undefined
    ? undefined
    : navigation.taskByWorkspace[workspace.id]
  const task = activeTaskId === undefined
    ? undefined
    : tx.get<TaskRow>('SELECT * FROM tasks WHERE id = ? AND archived_at IS NULL', activeTaskId)
  const context = task === undefined
    ? undefined
    : tx.get<ContextRow>(
      `SELECT id, workspace_id, cwd, created_at, archived_at FROM execution_contexts
       WHERE id = ? AND kind = 'plain-directory'`,
      task.execution_context_id
    )
  const activeSceneId = task === undefined ? undefined : navigation.sceneByTask[task.id]
  const scene = activeSceneId === undefined
    ? undefined
    : tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ? AND archived_at IS NULL', activeSceneId)
  const activeSessionId = scene === undefined ? undefined : navigation.sessionByScene[scene.id]
  const session = activeSessionId === undefined
    ? undefined
    : tx.get<SessionRow>('SELECT * FROM sessions WHERE id = ? AND archived_at IS NULL', activeSessionId)
  const mount = session === undefined || scene === undefined
    ? undefined
    : tx.get<MountRow>(
      'SELECT * FROM session_mounts WHERE scene_id = ? AND session_id = ? ORDER BY created_at LIMIT 1',
      scene.id,
      session.id
    )
  return {
    workspace: workspace ? mapWorkspace(workspace) : null,
    executionContext: context ? mapPlainContext(context) : null,
    task: task ? mapTask(task) : null,
    scene: scene ? mapScene(scene) : null,
    session: session ? mapSession(session) : null,
    mount: mount ? mapMount(mount) : null,
    navigation
  }
}

function readNavigation(tx: DatabaseTransaction, windowId: string): WindowNavigation {
  const row = tx.get<{ active_workspace_id: string | null }>(
    'SELECT active_workspace_id FROM window_navigation WHERE window_id = ?',
    windowId
  )
  return {
    windowId,
    ...(row?.active_workspace_id == null ? {} : { activeWorkspaceId: row.active_workspace_id }),
    taskByWorkspace: Object.fromEntries(
      tx.all<{ workspace_id: string; active_task_id: string }>(
        `SELECT workspace_id, active_task_id FROM window_workspace_focus
         WHERE window_id = ? AND active_task_id IS NOT NULL`,
        windowId
      ).map((focus) => [focus.workspace_id, focus.active_task_id])
    ),
    sceneByTask: Object.fromEntries(
      tx.all<{ task_id: string; active_scene_id: string }>(
        `SELECT task_id, active_scene_id FROM window_task_focus
         WHERE window_id = ? AND active_scene_id IS NOT NULL`,
        windowId
      ).map((focus) => [focus.task_id, focus.active_scene_id])
    ),
    sessionByScene: Object.fromEntries(
      tx.all<{ scene_id: string; active_session_id: string }>(
        `SELECT scene_id, active_session_id FROM window_scene_focus
         WHERE window_id = ? AND active_session_id IS NOT NULL`,
        windowId
      ).map((focus) => [focus.scene_id, focus.active_session_id])
    )
  }
}

function firstActiveWorkspace(tx: DatabaseTransaction): WorkspaceRow | undefined {
  return tx.get<WorkspaceRow>(
    `SELECT * FROM workspaces WHERE archived_at IS NULL
     ORDER BY is_pinned DESC,
       CASE WHEN is_pinned = 1 THEN pin_sort_key END,
       CASE WHEN is_pinned = 0 THEN last_opened_at END DESC,
       created_at DESC, id LIMIT 1`
  )
}

function nextPinnedKey(
  tx: DatabaseTransaction,
  table: 'workspaces' | 'tasks',
  workspaceId?: string
): string {
  const count = table === 'tasks'
    ? tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM tasks WHERE workspace_id = ? AND archived_at IS NULL AND is_pinned = 1`,
        workspaceId!
      )?.count ?? 0
    : tx.get<{ count: number }>(
        `SELECT COUNT(*) AS count FROM workspaces WHERE archived_at IS NULL AND is_pinned = 1`
      )?.count ?? 0
  return sortKey(count)
}

function moveBefore(order: string[], sourceId: string, beforeId?: string): string[] {
  if (!order.includes(sourceId)) throw new Error('Pinned item is outside the requested group')
  if (beforeId !== undefined && !order.includes(beforeId)) {
    throw new Error('Pinned destination is outside the requested group')
  }
  const next = order.filter((id) => id !== sourceId)
  const target = beforeId === undefined ? next.length : next.indexOf(beforeId)
  next.splice(target, 0, sourceId)
  return next
}

function writeBootstrapFlag(
  tx: DatabaseTransaction,
  key: string,
  value: unknown,
  now: number
): void {
  tx.run(
    `INSERT INTO bootstrap_state (key, value_json, updated_at) VALUES (?, ?, ?)
     ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`,
    key,
    JSON.stringify(value),
    now
  )
}

function readBootstrapFlag(tx: DatabaseTransaction, key: string): unknown {
  const row = tx.get<{ value_json: string }>(
    'SELECT value_json FROM bootstrap_state WHERE key = ?',
    key
  )
  if (row === undefined) return undefined
  try {
    return JSON.parse(row.value_json) as unknown
  } catch {
    return undefined
  }
}

function mapWorkspace(row: WorkspaceRow): Workspace {
  return {
    id: row.id,
    name: row.name,
    rootDirectory: row.root_directory,
    ...(row.path_identity === null ? {} : { pathIdentity: row.path_identity }),
    taskOrder: parseStringArray(row.task_order_json),
    isDefault: row.is_default === 1,
    isPinned: row.is_pinned === 1,
    pinSortKey: row.pin_sort_key,
    lastOpenedAt: row.last_opened_at,
    ...(row.archived_at === null ? {} : { archivedAt: row.archived_at }),
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    version: row.version
  }
}

function mapTask(row: TaskRow): Task {
  return {
    id: row.id,
    workspaceId: row.workspace_id,
    ...(row.parent_task_id === null ? {} : { parentTaskId: row.parent_task_id }),
    title: row.title,
    status: row.status,
    executionContextId: row.execution_context_id,
    sortKey: row.sort_key,
    isPinned: row.is_pinned === 1,
    pinSortKey: row.pin_sort_key,
    lastOpenedAt: row.last_opened_at,
    ...(row.archived_at === null ? {} : { archivedAt: row.archived_at }),
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    version: row.version
  }
}

function mapScene(row: SceneRow): Scene {
  return {
    id: row.id,
    taskId: row.task_id,
    name: row.name,
    mode: row.mode,
    ...(row.root_node_id === null ? {} : { rootNodeId: row.root_node_id }),
    titlePinned: row.title_pinned === 1,
    sortKey: row.sort_key,
    layoutRevision: row.layout_revision,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    ...(row.archived_at === null ? {} : { archivedAt: row.archived_at })
  }
}

function mapSession(row: SessionRow): Session {
  return {
    id: row.id,
    taskId: row.task_id,
    executionContextId: row.execution_context_id,
    kind: row.kind,
    status: row.status,
    title: row.title,
    cwd: row.cwd,
    createdAt: row.created_at,
    updatedAt: row.updated_at,
    lastActivityAt: row.last_activity_at,
    ...(row.archived_at === null ? {} : { archivedAt: row.archived_at }),
    version: row.version
  }
}

function mapPlainContext(row: ContextRow): PlainDirectoryContext {
  return {
    kind: 'plain-directory',
    id: row.id,
    workspaceId: row.workspace_id,
    cwd: row.cwd,
    createdAt: row.created_at,
    ...(row.archived_at === null ? {} : { archivedAt: row.archived_at })
  }
}

function mapMount(row: MountRow): SessionMount {
  return {
    id: row.id,
    sceneId: row.scene_id,
    ...(row.scene_node_id === null ? {} : { sceneNodeId: row.scene_node_id }),
    ...(row.scene_window_id === null ? {} : { sceneWindowId: row.scene_window_id }),
    sessionId: row.session_id,
    createdAt: row.created_at
  }
}

function requiredTrimmed(value: string, label: string): string {
  const trimmed = value.trim()
  if (!trimmed) throw new Error(`${label} must not be empty`)
  return trimmed
}

function nextTaskTitle(tx: DatabaseTransaction, workspaceId: string): string {
  const titles = new Set(tx.all<{ title: string }>(
    'SELECT title FROM tasks WHERE workspace_id = ? AND archived_at IS NULL',
    workspaceId
  ).map(({ title }) => title))
  if (!titles.has('新事项')) return '新事项'
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `新事项 ${suffix}`
    if (!titles.has(candidate)) return candidate
  }
}

function sortKey(index: number): string {
  return `a${index.toString().padStart(8, '0')}`
}

function workspacePathIsAvailable(tx: DatabaseTransaction, workspaceId: string): boolean {
  return tx.get<{ status: 'valid' | 'invalid' }>(
    'SELECT status FROM workspace_path_state WHERE workspace_id = ?',
    workspaceId
  )?.status !== 'invalid'
}

export function assertWorkspacePathAvailable(tx: DatabaseTransaction, workspaceId: string): void {
  if (!workspacePathIsAvailable(tx, workspaceId)) {
    throw new Error(WORKSPACE_PATH_INVALID_MESSAGE)
  }
}

function parseStringArray(value: string): string[] {
  try {
    const parsed = JSON.parse(value) as unknown
    return Array.isArray(parsed) && parsed.every((item) => typeof item === 'string')
      ? parsed
      : []
  } catch {
    return []
  }
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (!row) throw new Error(`${label} does not exist`)
  return row
}
