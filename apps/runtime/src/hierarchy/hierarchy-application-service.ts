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

export interface WorkspaceHierarchyResult {
  workspace: Workspace | null
  executionContext: PlainDirectoryContext | null
  task: Task | null
  scene: Scene | null
  session: Session | null
  mount: SessionMount | null
  navigation: WindowNavigation
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
      let workspace = firstActiveWorkspace(context.tx)
      if (!workspace && !readBootstrapFlag(context.tx, 'default-workspace-removed')) {
        const created = this.#createCompleteHierarchy(context, {
          ids,
          windowId: input.windowId,
          name: input.defaultName,
          rootDirectory: input.defaultRootDirectory,
          taskTitle: '默认',
          commandId: command.commandId,
          now: input.now
        })
        writeBootstrapFlag(
          context.tx,
          'default-workspace-created',
          created.id,
          input.now
        )
      } else if (workspace) {
        activateWorkspaceInTransaction(context.tx, input.windowId, workspace.id, input.now)
      } else {
        clearWindowNavigation(context.tx, input.windowId, input.now)
      }
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
    command: DomainCommandMetadata,
    input: RenameWorkspaceInput
  ): Workspace {
    const name = requiredTrimmed(input.name, 'Workspace name')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const row = tx.get<WorkspaceRow>(
        'SELECT * FROM workspaces WHERE id = ? AND archived_at IS NULL',
        input.workspaceId
      )
      if (!row) throw new Error(`Workspace ${input.workspaceId} does not exist`)
      tx.run(
        `UPDATE workspaces
         SET name = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        name,
        input.now,
        input.workspaceId
      )
      const workspace = mapWorkspace({
        ...row,
        name,
        updated_at: input.now,
        version: row.version + 1
      })
      emit({
        eventId: `${command.commandId}:workspace-renamed`,
        eventType: 'workspace.renamed',
        aggregateType: 'workspace',
        aggregateId: workspace.id,
        workspaceId: workspace.id,
        payload: { name },
        occurredAt: input.now
      })
      return workspace
    }).result
  }

  removeWorkspace(
    command: DomainCommandMetadata,
    input: RemoveWorkspaceInput
  ): WorkspaceHierarchyResult {
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
      return readHierarchyResult(tx, input.windowId)
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
    }
  ): Workspace {
    const { tx, emit } = context
    const name = requiredTrimmed(input.name, 'Workspace name')
    const rootDirectory = resolve(input.rootDirectory)
    const { ids } = input

    tx.run(
      `INSERT INTO workspaces (
         id, name, root_directory, path_identity, task_order_json,
         created_at, updated_at, version
       ) VALUES (?, ?, ?, ?, ?, ?, ?, 2)`,
      ids.workspaceId,
      name,
      rootDirectory,
      `path:${rootDirectory}`,
      JSON.stringify([ids.taskId]),
      input.now,
      input.now
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
         created_at, updated_at, version
       ) VALUES (?, ?, ?, ?, 'active', 'a0', ?, ?, 1)`,
      ids.taskId,
      ids.workspaceId,
      ids.executionContextId,
      input.taskTitle,
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

function registerWindow(tx: DatabaseTransaction, windowId: string, now: number): void {
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

function readHierarchyResult(
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
    'SELECT * FROM workspaces WHERE archived_at IS NULL ORDER BY created_at, id LIMIT 1'
  )
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
  return row === undefined ? undefined : JSON.parse(row.value_json)
}

function mapWorkspace(row: WorkspaceRow): Workspace {
  return {
    id: row.id,
    name: row.name,
    rootDirectory: row.root_directory,
    ...(row.path_identity === null ? {} : { pathIdentity: row.path_identity }),
    taskOrder: parseStringArray(row.task_order_json),
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

function parseStringArray(value: string): string[] {
  const parsed = JSON.parse(value) as unknown
  if (!Array.isArray(parsed) || parsed.some((item) => typeof item !== 'string')) {
    throw new Error('stored task order is invalid')
  }
  return parsed
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (!row) throw new Error(`${label} does not exist`)
  return row
}
