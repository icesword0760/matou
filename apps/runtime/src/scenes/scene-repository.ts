import type {
  DomainCommandMetadata,
  DomainCommit,
  Scene,
  SceneMode,
  SceneNode,
  SceneWindow,
  SessionMount
} from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainMutationContext, DomainTransactionManager } from '../storage/domain-transaction'

interface SceneRow {
  id: string
  task_id: string
  name: string
  mode: SceneMode
  root_node_id: string | null
  title_pinned: number
  sort_key: string
  layout_revision: number
  created_at: number
  updated_at: number
  archived_at: number | null
}
interface NodeRow {
  id: string
  scene_id: string
  parent_node_id: string | null
  kind: SceneNode['kind']
  direction: 'horizontal' | 'vertical' | null
  ordinal: number
  created_at: number
}
interface MountRow {
  id: string
  scene_id: string
  scene_node_id: string | null
  scene_window_id: string | null
  session_id: string
  created_at: number
}
interface WindowRow {
  id: string
  scene_id: string
  native_window_key: string
  state: SceneWindow['state']
  created_at: number
  updated_at: number
}

export interface SceneSnapshot {
  scene: Scene
  nodes: SceneNode[]
  mounts: SessionMount[]
  windows: SceneWindow[]
}

export class SceneRepository {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  createScene(
    command: DomainCommandMetadata,
    input: {
      id: string
      rootNodeId: string
      taskId: string
      name: string
      mode: SceneMode
      now: number
    }
  ): DomainCommit<Scene> {
    const name = input.name.trim()
    if (!name) throw new Error('Scene name must not be empty')
    return this.#transactions.execute(command, ({ tx, emit }) => {
      if (!tx.get('SELECT id FROM tasks WHERE id = ?', input.taskId)) {
        throw new Error(`Task ${input.taskId} does not exist`)
      }
      tx.run(
        `INSERT INTO scenes (id, task_id, name, mode, root_node_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)`,
        input.id,
        input.taskId,
        name,
        input.mode,
        input.rootNodeId,
        input.now,
        input.now
      )
      tx.run(
        `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
         VALUES (?, ?, 'root', 0, ?)`,
        input.rootNodeId,
        input.id,
        input.now
      )
      const scene = mapScene(requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', input.id), 'Scene'))
      emitScene(emit, command.commandId, 'scene.created', scene, input.now, { scene })
      return scene
    })
  }

  setMode(
    command: DomainCommandMetadata,
    sceneId: string,
    mode: SceneMode,
    now: number
  ): DomainCommit<Scene> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', sceneId), 'Scene')
      tx.run('UPDATE scenes SET mode = ?, updated_at = ? WHERE id = ?', mode, now, sceneId)
      const scene = mapScene({ ...before, mode, updated_at: now })
      emitScene(emit, command.commandId, 'scene.mode-changed', scene, now, { mode })
      return scene
    })
  }

  addNode(
    command: DomainCommandMetadata,
    input: {
      id: string
      sceneId: string
      parentNodeId: string
      kind: Exclude<SceneNode['kind'], 'root'>
      direction?: 'horizontal' | 'vertical'
      ordinal: number
      now: number
    }
  ): DomainCommit<SceneNode> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const scene = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', input.sceneId), 'Scene')
      const parent = tx.get<NodeRow>('SELECT * FROM scene_nodes WHERE id = ? AND scene_id = ?', input.parentNodeId, input.sceneId)
      if (!parent) throw new Error('parent SceneNode must belong to the Scene')
      if (input.kind === 'split' && input.direction === undefined) {
        throw new Error('split SceneNode requires a direction')
      }
      tx.run(
        `INSERT INTO scene_nodes (
           id, scene_id, parent_node_id, kind, direction, ordinal, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)`,
        input.id,
        input.sceneId,
        input.parentNodeId,
        input.kind,
        input.direction ?? null,
        input.ordinal,
        input.now
      )
      const node = mapNode(requireRow(tx.get<NodeRow>('SELECT * FROM scene_nodes WHERE id = ?', input.id), 'SceneNode'))
      emitScene(emit, command.commandId, 'scene.node-added', mapScene(scene), input.now, { node })
      return node
    })
  }

  attachWindow(
    command: DomainCommandMetadata,
    input: {
      id: string
      sceneId: string
      nativeWindowKey: string
      state: 'attached' | 'detached'
      now: number
    }
  ): DomainCommit<SceneWindow> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const scene = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', input.sceneId), 'Scene')
      tx.run(
        `INSERT INTO scene_windows (
           id, scene_id, native_window_key, state, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?)`,
        input.id,
        input.sceneId,
        input.nativeWindowKey,
        input.state,
        input.now,
        input.now
      )
      const window = mapWindow(requireRow(tx.get<WindowRow>('SELECT * FROM scene_windows WHERE id = ?', input.id), 'SceneWindow'))
      emitScene(emit, command.commandId, 'scene.window-attached', mapScene(scene), input.now, { window })
      return window
    })
  }

  mountSession(
    command: DomainCommandMetadata,
    input: {
      id: string
      sceneId: string
      sceneNodeId: string
      sceneWindowId?: string
      sessionId: string
      now: number
    }
  ): DomainCommit<SessionMount> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const scene = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', input.sceneId), 'Scene')
      const session = tx.get<{ task_id: string }>('SELECT task_id FROM sessions WHERE id = ?', input.sessionId)
      if (!session || session.task_id !== scene.task_id) {
        throw new Error('mounted Session must belong to the Scene Task')
      }
      if (!tx.get('SELECT id FROM scene_nodes WHERE id = ? AND scene_id = ?', input.sceneNodeId, input.sceneId)) {
        throw new Error('mount SceneNode must belong to the Scene')
      }
      if (
        input.sceneWindowId !== undefined &&
        !tx.get('SELECT id FROM scene_windows WHERE id = ? AND scene_id = ?', input.sceneWindowId, input.sceneId)
      ) {
        throw new Error('mount SceneWindow must belong to the Scene')
      }
      tx.run(
        `INSERT INTO session_mounts (
           id, scene_id, scene_node_id, scene_window_id, session_id, created_at
         ) VALUES (?, ?, ?, ?, ?, ?)`,
        input.id,
        input.sceneId,
        input.sceneNodeId,
        input.sceneWindowId ?? null,
        input.sessionId,
        input.now
      )
      const mount = mapMount(requireRow(tx.get<MountRow>('SELECT * FROM session_mounts WHERE id = ?', input.id), 'SessionMount'))
      emitScene(emit, command.commandId, 'scene.session-mounted', mapScene(scene), input.now, { mount })
      return mount
    })
  }

  unmountSession(
    command: DomainCommandMetadata,
    mountId: string,
    now: number
  ): DomainCommit<SessionMount> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const mountRow = requireRow(tx.get<MountRow>('SELECT * FROM session_mounts WHERE id = ?', mountId), 'SessionMount')
      const scene = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', mountRow.scene_id), 'Scene')
      tx.run('DELETE FROM session_mounts WHERE id = ?', mountId)
      const mount = mapMount(mountRow)
      emitScene(emit, command.commandId, 'scene.session-unmounted', mapScene(scene), now, { mount })
      return mount
    })
  }

  removeNode(
    command: DomainCommandMetadata,
    nodeId: string,
    now: number
  ): DomainCommit<SceneNode> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const nodeRow = requireRow(tx.get<NodeRow>('SELECT * FROM scene_nodes WHERE id = ?', nodeId), 'SceneNode')
      const scene = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', nodeRow.scene_id), 'Scene')
      if (scene.root_node_id === nodeId || nodeRow.kind === 'root') throw new Error('Scene root node cannot be removed')
      if (tx.get('SELECT id FROM scene_nodes WHERE parent_node_id = ? LIMIT 1', nodeId)) {
        throw new Error('SceneNode children must be moved or removed first')
      }
      if (tx.get('SELECT id FROM session_mounts WHERE scene_node_id = ? LIMIT 1', nodeId)) {
        throw new Error('SessionMount must be moved or removed before its SceneNode')
      }
      tx.run('DELETE FROM scene_nodes WHERE id = ?', nodeId)
      const node = mapNode(nodeRow)
      emitScene(emit, command.commandId, 'scene.node-removed', mapScene(scene), now, { node })
      return node
    })
  }

  detachWindow(
    command: DomainCommandMetadata,
    windowId: string,
    now: number
  ): DomainCommit<SceneWindow> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<WindowRow>('SELECT * FROM scene_windows WHERE id = ?', windowId), 'SceneWindow')
      const scene = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', before.scene_id), 'Scene')
      tx.run("UPDATE scene_windows SET state = 'detached', updated_at = ? WHERE id = ?", now, windowId)
      const window = mapWindow({ ...before, state: 'detached', updated_at: now })
      emitScene(emit, command.commandId, 'scene.window-detached', mapScene(scene), now, { window })
      return window
    })
  }

  archiveScene(
    command: DomainCommandMetadata,
    sceneId: string,
    now: number
  ): DomainCommit<Scene> {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const before = requireRow(tx.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', sceneId), 'Scene')
      tx.run('UPDATE scenes SET archived_at = ?, updated_at = ? WHERE id = ?', now, now, sceneId)
      const scene = mapScene({ ...before, archived_at: now, updated_at: now })
      emitScene(emit, command.commandId, 'scene.archived', scene, now, { archivedAt: now })
      return scene
    })
  }

  snapshot(sceneId: string): SceneSnapshot | undefined {
    const scene = this.#database.get<SceneRow>('SELECT * FROM scenes WHERE id = ?', sceneId)
    if (!scene) return undefined
    return {
      scene: mapScene(scene),
      nodes: this.#database.all<NodeRow>('SELECT * FROM scene_nodes WHERE scene_id = ? ORDER BY created_at, ordinal', sceneId).map(mapNode),
      mounts: this.#database.all<MountRow>('SELECT * FROM session_mounts WHERE scene_id = ? ORDER BY created_at', sceneId).map(mapMount),
      windows: this.#database.all<WindowRow>('SELECT * FROM scene_windows WHERE scene_id = ? ORDER BY created_at', sceneId).map(mapWindow)
    }
  }
}

function emitScene(
  emit: DomainMutationContext['emit'],
  eventId: string,
  eventType: string,
  scene: Scene,
  occurredAt: number,
  payload: unknown
): void {
  emit({
    eventId,
    eventType,
    aggregateType: 'scene',
    aggregateId: scene.id,
    taskId: scene.taskId,
    payload,
    occurredAt
  })
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
function mapNode(row: NodeRow): SceneNode {
  return {
    id: row.id,
    sceneId: row.scene_id,
    ...(row.parent_node_id === null ? {} : { parentNodeId: row.parent_node_id }),
    kind: row.kind,
    ...(row.direction === null ? {} : { direction: row.direction }),
    ordinal: row.ordinal,
    createdAt: row.created_at
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
function mapWindow(row: WindowRow): SceneWindow {
  return {
    id: row.id,
    sceneId: row.scene_id,
    nativeWindowKey: row.native_window_key,
    state: row.state,
    createdAt: row.created_at,
    updatedAt: row.updated_at
  }
}
function requireRow<T>(row: T | undefined, label: string): T {
  if (!row) throw new Error(`${label} does not exist`)
  return row
}
