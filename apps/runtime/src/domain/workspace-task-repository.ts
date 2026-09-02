import { resolve } from 'node:path'

import type {
  DomainCommandMetadata,
  DomainCommit,
  PlainDirectoryContext,
  Task,
  TaskStatus,
  Workspace
} from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

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
  status: TaskStatus
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

export class WorkspaceTaskRepository {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  createWorkspace(
    command: DomainCommandMetadata,
    input: {
      id: string
      name: string
      rootDirectory: string
      pathIdentity?: string
      now: number
    }
  ): DomainCommit<Workspace> {
    const name = requiredTrimmed(input.name, 'Workspace name')
    const rootDirectory = resolve(input.rootDirectory)
    const pathIdentity = input.pathIdentity ?? `path:${rootDirectory}`
    return this.#transactions.execute(command, ({ tx, emit }) => {
      tx.run(
        `INSERT INTO workspaces (
           id, name, root_directory, path_identity, task_order_json,
           created_at, updated_at, version, last_opened_at
         ) VALUES (?, ?, ?, ?, '[]', ?, ?, 1, ?)`,
        input.id,
        name,
        rootDirectory,
        pathIdentity,
        input.now,
        input.now,
        input.now
      )
      const workspace: Workspace = {
        id: input.id,
        name,
        rootDirectory,
        pathIdentity,
        taskOrder: [],
        isDefault: false,
        isPinned: false,
        pinSortKey: '',
        lastOpenedAt: input.now,
        createdAt: input.now,
        updatedAt: input.now,
        version: 1
      }
      emit({
        eventId: `${command.commandId}:workspace-created`,
        eventType: 'workspace.created',
        aggregateType: 'workspace',
        aggregateId: input.id,
        workspaceId: input.id,
        payload: workspace,
        occurredAt: input.now
      })
      return workspace
    })
  }

  createPlainExecutionContext(
    command: DomainCommandMetadata,
    input: { id: string; workspaceId: string; cwd: string; now: number }
  ): DomainCommit<PlainDirectoryContext> {
    const cwd = resolve(input.cwd)
    return this.#transactions.execute(command, ({ tx, emit }) => {
      requireWorkspace(tx.get<{ id: string }>('SELECT id FROM workspaces WHERE id = ?', input.workspaceId))
      tx.run(
        `INSERT INTO execution_contexts (
           id, workspace_id, kind, cwd, created_at
         ) VALUES (?, ?, 'plain-directory', ?, ?)`,
        input.id,
        input.workspaceId,
        cwd,
        input.now
      )
      const context: PlainDirectoryContext = {
        kind: 'plain-directory',
        id: input.id,
        workspaceId: input.workspaceId,
        cwd,
        createdAt: input.now
      }
      emit({
        eventId: `${command.commandId}:execution-context-created`,
        eventType: 'execution-context.created',
        aggregateType: 'execution-context',
        aggregateId: input.id,
        workspaceId: input.workspaceId,
        payload: context,
        occurredAt: input.now
      })
      return context
    })
  }

  updateWorkspace(
    command: DomainCommandMetadata,
    input: {
      id: string
      name?: string
      rootDirectory?: string
      pathIdentity?: string
      now: number
    }
  ): DomainCommit<Workspace> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = tx.get<WorkspaceRow>('SELECT * FROM workspaces WHERE id = ?', input.id)
      if (!before) throw new Error(`Workspace ${input.id} does not exist`)
      const name = input.name === undefined ? before.name : requiredTrimmed(input.name, 'Workspace name')
      const rootDirectory = input.rootDirectory === undefined ? before.root_directory : resolve(input.rootDirectory)
      const pathIdentity = input.pathIdentity ?? (input.rootDirectory === undefined ? before.path_identity : `path:${rootDirectory}`)
      tx.run(
        `UPDATE workspaces SET name = ?, root_directory = ?, path_identity = ?,
         updated_at = ?, version = version + 1 WHERE id = ?`,
        name, rootDirectory, pathIdentity, input.now, input.id
      )
      const workspace = mapWorkspace({
        ...before, name, root_directory: rootDirectory, path_identity: pathIdentity,
        updated_at: input.now, version: before.version + 1
      })
      emit({
        eventId: `${command.commandId}:workspace-updated`, eventType: 'workspace.updated',
        aggregateType: 'workspace', aggregateId: input.id, workspaceId: input.id,
        payload: workspace, occurredAt: input.now
      })
      return workspace
    })
  }

  archiveWorkspace(
    command: DomainCommandMetadata,
    workspaceId: string,
    now: number
  ): DomainCommit<Workspace> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = tx.get<WorkspaceRow>('SELECT * FROM workspaces WHERE id = ?', workspaceId)
      if (!before) throw new Error(`Workspace ${workspaceId} does not exist`)
      tx.run(
        'UPDATE workspaces SET archived_at = ?, updated_at = ?, version = version + 1 WHERE id = ?',
        now, now, workspaceId
      )
      const workspace = mapWorkspace({ ...before, archived_at: now, updated_at: now, version: before.version + 1 })
      emit({
        eventId: `${command.commandId}:workspace-archived`, eventType: 'workspace.archived',
        aggregateType: 'workspace', aggregateId: workspaceId, workspaceId,
        payload: { archivedAt: now }, occurredAt: now
      })
      return workspace
    })
  }

  createTask(
    command: DomainCommandMetadata,
    input: {
      id: string
      workspaceId: string
      parentTaskId?: string
      executionContextId: string
      title: string
      status: Exclude<TaskStatus, 'archived'>
      sortKey: string
      now: number
    }
  ): DomainCommit<Task> {
    const title = requiredTrimmed(input.title, 'Task title')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const workspace = tx.get<{ task_order_json: string }>(
        'SELECT task_order_json FROM workspaces WHERE id = ? AND archived_at IS NULL',
        input.workspaceId
      )
      requireWorkspace(workspace)
      const context = tx.get<{ workspace_id: string }>(
        'SELECT workspace_id FROM execution_contexts WHERE id = ? AND archived_at IS NULL',
        input.executionContextId
      )
      if (!context || context.workspace_id !== input.workspaceId) {
        throw new Error('execution context must belong to the same Workspace')
      }
      if (input.parentTaskId !== undefined) {
        const parent = tx.get<{ workspace_id: string }>(
          'SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL',
          input.parentTaskId
        )
        if (!parent || parent.workspace_id !== input.workspaceId) {
          throw new Error('parent Task must belong to the same Workspace')
        }
      }
      const duplicate = tx.get<{ id: string }>(
        'SELECT id FROM tasks WHERE workspace_id = ? AND title = ? AND archived_at IS NULL',
        input.workspaceId,
        title
      )
      if (duplicate) {
        throw new Error(`an active Task named "${title}" already exists in this Workspace`)
      }

      tx.run(
        `INSERT INTO tasks (
           id, workspace_id, parent_task_id, execution_context_id, title,
           status, sort_key, created_at, updated_at, version, last_opened_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?)`,
        input.id,
        input.workspaceId,
        input.parentTaskId ?? null,
        input.executionContextId,
        title,
        input.status,
        input.sortKey,
        input.now,
        input.now,
        input.now
      )
      const order = parseStringArray(workspace.task_order_json)
      order.push(input.id)
      tx.run(
        `UPDATE workspaces
         SET task_order_json = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        JSON.stringify(order),
        input.now,
        input.workspaceId
      )
      const task: Task = {
        id: input.id,
        workspaceId: input.workspaceId,
        ...(input.parentTaskId === undefined ? {} : { parentTaskId: input.parentTaskId }),
        title,
        status: input.status,
        executionContextId: input.executionContextId,
        sortKey: input.sortKey,
        isPinned: false,
        pinSortKey: '',
        lastOpenedAt: input.now,
        createdAt: input.now,
        updatedAt: input.now,
        version: 1
      }
      emit({
        eventId: `${command.commandId}:task-created`,
        eventType: 'task.created',
        aggregateType: 'task',
        aggregateId: input.id,
        workspaceId: input.workspaceId,
        taskId: input.id,
        payload: task,
        occurredAt: input.now
      })
      emit({
        eventId: `${command.commandId}:workspace-task-order`,
        eventType: 'workspace.task-order-changed',
        aggregateType: 'workspace',
        aggregateId: input.workspaceId,
        workspaceId: input.workspaceId,
        payload: { taskOrder: order },
        occurredAt: input.now
      })
      return task
    })
  }

  archiveTask(
    command: DomainCommandMetadata,
    taskId: string,
    now: number
  ): DomainCommit<Task> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const row = tx.get<TaskRow>('SELECT * FROM tasks WHERE id = ?', taskId)
      if (!row) throw new Error(`Task ${taskId} does not exist`)
      tx.run(
        `UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        now,
        now,
        taskId
      )
      const task = mapTask({ ...row, status: 'archived', archived_at: now, updated_at: now, version: row.version + 1 })
      emit({
        eventId: `${command.commandId}:task-archived`,
        eventType: 'task.archived',
        aggregateType: 'task',
        aggregateId: taskId,
        workspaceId: row.workspace_id,
        taskId,
        payload: { archivedAt: now },
        occurredAt: now
      })
      return task
    })
  }

  updateTask(
    command: DomainCommandMetadata,
    input: {
      id: string
      title?: string
      status?: Exclude<TaskStatus, 'archived'>
      parentTaskId?: string | null
      executionContextId?: string
      sortKey?: string
      now: number
    }
  ): DomainCommit<Task> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = tx.get<TaskRow>('SELECT * FROM tasks WHERE id = ?', input.id)
      if (!before) throw new Error(`Task ${input.id} does not exist`)
      if (before.archived_at !== null) throw new Error('archived Task cannot be modified')
      const title = input.title === undefined ? before.title : requiredTrimmed(input.title, 'Task title')
      if (title !== before.title && tx.get(
        'SELECT id FROM tasks WHERE workspace_id = ? AND title = ? AND archived_at IS NULL AND id <> ?',
        before.workspace_id, title, input.id
      )) {
        throw new Error(`an active Task named "${title}" already exists in this Workspace`)
      }
      const executionContextId = input.executionContextId ?? before.execution_context_id
      const context = tx.get<{ workspace_id: string }>(
        'SELECT workspace_id FROM execution_contexts WHERE id = ? AND archived_at IS NULL',
        executionContextId
      )
      if (!context || context.workspace_id !== before.workspace_id) {
        throw new Error('execution context must belong to the same Workspace')
      }
      const parentTaskId = Object.prototype.hasOwnProperty.call(input, 'parentTaskId')
        ? input.parentTaskId ?? null
        : before.parent_task_id
      if (parentTaskId !== null) {
        const parent = tx.get<{ workspace_id: string }>('SELECT workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL', parentTaskId)
        if (!parent || parent.workspace_id !== before.workspace_id) {
          throw new Error('parent Task must belong to the same Workspace')
        }
        if (taskParentCreatesCycle(tx, input.id, parentTaskId)) {
          throw new Error('Task parent would introduce a cycle')
        }
      }
      const status = input.status ?? before.status
      const sortKey = input.sortKey ?? before.sort_key
      tx.run(
        `UPDATE tasks SET title = ?, status = ?, parent_task_id = ?, execution_context_id = ?,
         sort_key = ?, updated_at = ?, version = version + 1 WHERE id = ?`,
        title, status, parentTaskId, executionContextId, sortKey, input.now, input.id
      )
      const task = mapTask({
        ...before, title, status, parent_task_id: parentTaskId,
        execution_context_id: executionContextId, sort_key: sortKey,
        updated_at: input.now, version: before.version + 1
      })
      emit({
        eventId: `${command.commandId}:task-updated`, eventType: 'task.updated',
        aggregateType: 'task', aggregateId: input.id, workspaceId: before.workspace_id,
        taskId: input.id, payload: task, occurredAt: input.now
      })
      return task
    })
  }

  getWorkspace(id: string): Workspace | undefined {
    const row = this.#database.get<WorkspaceRow>('SELECT * FROM workspaces WHERE id = ?', id)
    return row ? mapWorkspace(row) : undefined
  }

  listWorkspaces(): Workspace[] {
    return this.#database.all<WorkspaceRow>(
      'SELECT * FROM workspaces ORDER BY created_at, id'
    ).map(mapWorkspace)
  }

  getTask(id: string): Task | undefined {
    const row = this.#database.get<TaskRow>('SELECT * FROM tasks WHERE id = ?', id)
    return row ? mapTask(row) : undefined
  }


  listTasks(): Task[] {
    return this.#database.all<TaskRow>(
      'SELECT * FROM tasks ORDER BY created_at, id'
    ).map(mapTask)
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

function requiredTrimmed(value: string, label: string): string {
  const trimmed = value.trim()
  if (!trimmed) throw new Error(`${label} must not be empty`)
  return trimmed
}

function requireWorkspace<T>(workspace: T | undefined): asserts workspace is T {
  if (!workspace) throw new Error('Workspace does not exist or is archived')
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

function taskParentCreatesCycle(
  tx: DatabaseTransaction,
  taskId: string,
  parentTaskId: string
): boolean {
  if (taskId === parentTaskId) return true
  return Boolean(tx.get(
    `WITH RECURSIVE ancestors(id) AS (
       SELECT parent_task_id FROM tasks WHERE id = ? AND parent_task_id IS NOT NULL
       UNION
       SELECT tasks.parent_task_id FROM tasks JOIN ancestors ON tasks.id = ancestors.id
       WHERE tasks.parent_task_id IS NOT NULL
     ) SELECT 1 AS found FROM ancestors WHERE id = ? LIMIT 1`,
    parentTaskId,
    taskId
  ))
}
