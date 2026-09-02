import type { DomainEventWireEnvelope } from '@matou/contracts'

type Entity = { id: string; [key: string]: unknown }

export interface RuntimeProjectionSnapshot {
  runtimeGeneration: string
  eventSequence: number
  workspaces: Entity[]
  tasks: Entity[]
  sessions: Entity[]
  relations: Entity[]
  scenes: Entity[]
  sessionGraphs?: Record<string, SessionGraphProjection>
  hierarchy?: HierarchyProjection
}

export interface SessionGraphProjection {
  sceneId: string
  focusedSessionId?: string
  nodes: Array<Record<string, unknown> & { sessionId: string }>
  edges: Array<Record<string, unknown>>
}

export interface HierarchyProjection {
  windowId: string
  workspaces: Entity[]
  tasks: Entity[]
  sessions: Entity[]
  scenes: Entity[]
  closedScenes?: Entity[]
  navigation: Record<string, unknown>
  pathStates?: unknown[]
  taskPlacements?: unknown[]
  sceneSnapshots?: unknown[]
  unreadByTask?: Record<string, number>
  sessionHuds?: unknown[]
  sessionGraphs?: Record<string, SessionGraphProjection>
}

export interface SceneSnapshotProjection {
  scene: Entity
  nodes: unknown[]
  mounts: unknown[]
  windows: unknown[]
  geometry?: unknown[]
}

export interface RuntimeProjectionView extends Omit<RuntimeProjectionSnapshot, 'sessionGraphs'> {
  sessionGraphs: Record<string, SessionGraphProjection>
  hierarchy: HierarchyProjection
}

export class RuntimeProjectionStore {
  #runtimeGeneration: string | undefined
  #eventSequence = 0
  readonly #workspaces = new Map<string, Entity>()
  readonly #tasks = new Map<string, Entity>()
  readonly #sessions = new Map<string, Entity>()
  readonly #relations = new Map<string, Entity>()
  readonly #scenes = new Map<string, Entity>()
  #sessionGraphs: Record<string, SessionGraphProjection> = {}
  #hierarchyMeta: Omit<HierarchyProjection, 'workspaces' | 'tasks' | 'sessions' | 'scenes'> = {
    windowId: 'window-1', navigation: { windowId: 'window-1' }
  }

  get eventSequence(): number {
    return this.#eventSequence
  }

  replace(snapshot: RuntimeProjectionSnapshot): void {
    this.#runtimeGeneration = snapshot.runtimeGeneration
    this.#eventSequence = snapshot.eventSequence
    replaceMap(this.#workspaces, snapshot.workspaces)
    replaceMap(this.#tasks, snapshot.tasks)
    replaceMap(this.#sessions, snapshot.sessions)
    replaceMap(this.#relations, snapshot.relations)
    replaceMap(this.#scenes, snapshot.scenes)
    this.#sessionGraphs = structuredClone(snapshot.sessionGraphs ?? {})
    if (snapshot.hierarchy) {
      const { workspaces: _workspaces, tasks: _tasks, sessions: _sessions, scenes: _scenes, ...meta } = snapshot.hierarchy
      this.#hierarchyMeta = structuredClone(meta)
    }
  }

  applyBatch(runtimeGeneration: string, events: DomainEventWireEnvelope[]): void {
    if (this.#runtimeGeneration !== runtimeGeneration) {
      throw new Error('runtime generation changed; a fresh projection snapshot is required')
    }
    for (const event of events) {
      if (event.sequence <= this.#eventSequence) continue
      if (event.sequence !== this.#eventSequence + 1) {
        throw new Error(
          `projection event gap: expected ${this.#eventSequence + 1}, received ${event.sequence}`
        )
      }
      this.#apply(event)
      this.#eventSequence = event.sequence
    }
  }

  applyCommandResult(
    result: unknown,
    context?: { type: string; input: Record<string, unknown> }
  ): void {
    if (Array.isArray(result)) {
      const target = context?.type === 'hierarchy.reorder-pinned-workspace'
        ? this.#workspaces
        : context?.type === 'hierarchy.reorder-pinned-task'
          ? this.#tasks
          : undefined
      if (target) {
        for (const value of result) {
          const entity = asEntity(value)
          if (entity) upsertEntity(target, entity)
        }
      }
      return
    }
    const object = asObject(result)
    if (!object) return
    const entities = [
      ['workspace', this.#workspaces],
      ['task', this.#tasks],
      ['session', this.#sessions],
      ['scene', this.#scenes]
    ] as const
    for (const [key, target] of entities) {
      const entity = asEntity(object[key])
      if (entity) upsertEntity(target, entity)
    }
    const directTarget = commandEntityTarget(context?.type, this.#workspaces, this.#tasks, this.#sessions, this.#scenes)
    const directEntity = asEntity(object)
    if (directTarget && directEntity) upsertEntity(directTarget, directEntity)
    const navigation = asObject(object.navigation)
    if (navigation) {
      this.#hierarchyMeta = {
        ...this.#hierarchyMeta,
        navigation: structuredClone(navigation)
      }
      const sessionByScene = asObject(navigation.sessionByScene)
      if (sessionByScene) {
        for (const [sceneId, sessionId] of Object.entries(sessionByScene)) {
          const graph = this.#sessionGraphs[sceneId]
          if (graph && typeof sessionId === 'string') graph.focusedSessionId = sessionId
        }
      }
    }
    const graph = asSessionGraph(object.graph) ?? asSessionGraph(object)
    if (graph) {
      this.#sessionGraphs[graph.sceneId] = structuredClone(graph)
      const sceneId = typeof context?.input.sceneId === 'string'
        ? context.input.sceneId
        : graph.sceneId
      if (graph.focusedSessionId && sceneId) {
        const currentNavigation = asObject(this.#hierarchyMeta.navigation) ?? {}
        const sessionByScene = {
          ...(asObject(currentNavigation.sessionByScene) ?? {}),
          [sceneId]: graph.focusedSessionId
        }
        this.#hierarchyMeta = {
          ...this.#hierarchyMeta,
          navigation: { ...currentNavigation, sessionByScene }
        }
      }
    }
    const pathState = asObject(object.pathState) ?? (
      context?.type === 'hierarchy.validate-workspace-path' ? object : undefined
    )
    if (pathState && typeof pathState.workspaceId === 'string') {
      this.#replacePathState(pathState)
    }
    if (context?.type === 'hierarchy.record-session-interaction' &&
      typeof object.workspaceId === 'string' && typeof object.taskId === 'string' &&
      typeof object.lastOpenedAt === 'number') {
      patchEntity(this.#workspaces, object.workspaceId, { lastOpenedAt: object.lastOpenedAt })
      patchEntity(this.#tasks, object.taskId, { lastOpenedAt: object.lastOpenedAt })
    }
    const removedSessionIds = Array.isArray(object.removedSessionIds)
      ? object.removedSessionIds
      : []
    for (const sessionId of removedSessionIds) {
      if (typeof sessionId === 'string') {
        patchEntity(this.#sessions, sessionId, { status: 'archived' })
      }
    }
    this.#archiveCommandTarget(context)
    this.#mergeActiveSceneSnapshot(object)
  }

  applySceneSnapshot(snapshot: SceneSnapshotProjection): void {
    upsertEntity(this.#scenes, snapshot.scene)
    const snapshots = Array.isArray(this.#hierarchyMeta.sceneSnapshots)
      ? [...this.#hierarchyMeta.sceneSnapshots]
      : []
    const index = snapshots.findIndex((candidate) =>
      asObject(asObject(candidate)?.scene)?.id === snapshot.scene.id)
    const next = structuredClone(snapshot)
    if (index >= 0) snapshots[index] = next
    else snapshots.push(next)
    this.#hierarchyMeta = { ...this.#hierarchyMeta, sceneSnapshots: snapshots }
  }

  applySceneGraph(graph: SessionGraphProjection): void {
    this.#sessionGraphs[graph.sceneId] = structuredClone(graph)
  }

  view(): RuntimeProjectionView {
    if (!this.#runtimeGeneration) throw new Error('projection snapshot has not been loaded')
    const workspaces = [...this.#workspaces.values()]
    const tasks = [...this.#tasks.values()]
    const sessions = [...this.#sessions.values()]
    const scenes = [...this.#scenes.values()]
    return {
      runtimeGeneration: this.#runtimeGeneration,
      eventSequence: this.#eventSequence,
      workspaces,
      tasks,
      sessions,
      relations: [...this.#relations.values()],
      scenes,
      sessionGraphs: structuredClone(this.#sessionGraphs),
      hierarchy: {
        ...structuredClone(this.#hierarchyMeta),
        workspaces: workspaces.filter(isActive),
        tasks: tasks.filter(isActive),
        sessions: sessions.filter(isActive),
        scenes: scenes.filter(isActive),
        closedScenes: scenes.filter((scene) => !isActive(scene)),
        sessionGraphs: structuredClone(this.#sessionGraphs)
      }
    }
  }

  #apply(event: DomainEventWireEnvelope): void {
    const payload = asEntity(event.payload)
    if (event.eventType === 'workspace.created' || event.eventType === 'workspace.updated') {
      if (payload) this.#workspaces.set(event.aggregateId, payload)
    } else if (event.eventType === 'workspace.archived') {
      patchEntity(this.#workspaces, event.aggregateId, event.payload)
    } else if (event.eventType === 'workspace.renamed' || event.eventType === 'workspace.relinked' ||
      event.eventType === 'workspace.pin-changed' || event.eventType === 'workspace.task-order-changed') {
      patchEntity(this.#workspaces, event.aggregateId, event.payload)
    } else if (event.eventType === 'workspace.pin-order-changed') {
      this.#applyOrder(this.#workspaces, asObject(event.payload)?.workspaceIds, 'pinSortKey')
    } else if (event.eventType === 'workspace.path-status-changed') {
      const state = asObject(event.payload)
      if (state) this.#replacePathState({ workspaceId: event.aggregateId, ...state })
    } else if (event.eventType === 'task.created' || event.eventType === 'task.updated') {
      if (payload) this.#tasks.set(event.aggregateId, payload)
    } else if (event.eventType === 'task.renamed' || event.eventType === 'task.pin-changed' ||
      event.eventType === 'task.scene-order-changed') {
      patchEntity(this.#tasks, event.aggregateId, event.payload)
    } else if (event.eventType === 'task.pin-order-changed') {
      this.#applyOrder(this.#tasks, asObject(event.payload)?.taskIds, 'pinSortKey')
    } else if (event.eventType === 'task.archived') {
      patchEntity(this.#tasks, event.aggregateId, { ...(asObject(event.payload) ?? {}), status: 'archived' })
    } else if (event.eventType === 'navigation.recency-changed') {
      const recency = asObject(event.payload)
      if (typeof recency?.workspaceId === 'string' && typeof recency.taskId === 'string' &&
        typeof recency.lastOpenedAt === 'number') {
        patchEntity(this.#workspaces, recency.workspaceId, { lastOpenedAt: recency.lastOpenedAt })
        patchEntity(this.#tasks, recency.taskId, { lastOpenedAt: recency.lastOpenedAt })
      }
    } else if (event.eventType === 'session.created') {
      if (payload) this.#sessions.set(event.aggregateId, payload)
    } else if (event.eventType === 'session.updated') {
      const session = asEntity(asObject(event.payload)?.session)
      if (session) this.#sessions.set(event.aggregateId, session)
    } else if (event.eventType === 'session.cwd-updated') {
      patchEntity(this.#sessions, event.aggregateId, event.payload)
      for (const graph of Object.values(this.#sessionGraphs)) {
        graph.nodes = graph.nodes.map((node) => node.sessionId === event.aggregateId
          ? { ...node, ...(asObject(event.payload) ?? {}) }
          : node)
      }
    } else if (event.eventType === 'session.archived') {
      patchEntity(this.#sessions, event.aggregateId, { ...(asObject(event.payload) ?? {}), status: 'archived' })
    } else if (event.eventType === 'session-relation.created' || event.eventType === 'session-relation.restored') {
      if (payload) this.#relations.set(event.aggregateId, payload)
    } else if (event.eventType === 'session-relation.revoked') {
      this.#relations.delete(event.aggregateId)
    } else if (event.eventType === 'scene.created') {
      const scene = asEntity(asObject(event.payload)?.scene) ?? payload
      if (scene) this.#scenes.set(event.aggregateId, scene)
    } else if (event.eventType === 'scene.renamed') {
      patchEntity(this.#scenes, event.aggregateId, event.payload)
    } else if (event.eventType === 'scene.reopened') {
      unarchiveEntity(this.#scenes, event.aggregateId, event.payload)
    } else if (event.eventType === 'scene.mode-changed' || event.eventType === 'scene.archived') {
      patchEntity(this.#scenes, event.aggregateId, event.payload)
    } else if (isSessionGraphEvent(event.eventType)) {
      const session = asEntity(asObject(event.payload)?.session)
      if (session) this.#sessions.set(event.sessionId ?? event.aggregateId, session)
      const graph = asSessionGraph(asObject(event.payload)?.graph)
      if (graph) this.#sessionGraphs[graph.sceneId] = structuredClone(graph)
    }
  }

  #replacePathState(pathState: Record<string, unknown>): void {
    if (typeof pathState.workspaceId !== 'string') return
    const pathStates = Array.isArray(this.#hierarchyMeta.pathStates)
      ? [...this.#hierarchyMeta.pathStates]
      : []
    const index = pathStates.findIndex((candidate) =>
      asObject(candidate)?.workspaceId === pathState.workspaceId)
    if (index >= 0) pathStates[index] = structuredClone(pathState)
    else pathStates.push(structuredClone(pathState))
    this.#hierarchyMeta = { ...this.#hierarchyMeta, pathStates }
  }

  #applyOrder(target: Map<string, Entity>, ids: unknown, key: string): void {
    if (!Array.isArray(ids)) return
    ids.forEach((id, index) => {
      if (typeof id === 'string') patchEntity(target, id, { [key]: projectionSortKey(index) })
    })
  }

  #archiveCommandTarget(context?: { type: string; input: Record<string, unknown> }): void {
    if (!context) return
    const target = context.type === 'hierarchy.remove-workspace'
      ? [this.#workspaces, context.input.workspaceId]
      : context.type === 'hierarchy.delete-task'
        ? [this.#tasks, context.input.taskId]
        : context.type === 'hierarchy.close-scene'
          ? [this.#scenes, context.input.sceneId]
          : context.type === 'hierarchy.delete-session'
            ? [this.#sessions, context.input.sessionId]
            : undefined
    if (target && typeof target[1] === 'string') {
      patchEntity(target[0] as Map<string, Entity>, target[1], { status: 'archived' })
    }
  }

  #mergeActiveSceneSnapshot(result: Record<string, unknown>): void {
    const scene = asEntity(result.scene)
    const mount = asEntity(result.mount)
    if (!scene && !mount) return
    const snapshots = Array.isArray(this.#hierarchyMeta.sceneSnapshots)
      ? [...this.#hierarchyMeta.sceneSnapshots]
      : []
    const sceneId = scene?.id ?? (typeof mount?.sceneId === 'string' ? mount.sceneId : undefined)
    if (!sceneId) return
    const index = snapshots.findIndex((candidate) => asObject(asObject(candidate)?.scene)?.id === sceneId)
    const existing = index >= 0 ? asObject(snapshots[index]) : undefined
    const currentScene = scene ?? asEntity(existing?.scene)
    if (!currentScene) return
    const mounts = Array.isArray(existing?.mounts) ? [...existing.mounts] : []
    if (mount && !mounts.some((candidate) => asObject(candidate)?.id === mount.id)) {
      mounts.push(structuredClone(mount))
    }
    const nodes = Array.isArray(existing?.nodes) ? [...existing.nodes] : []
    const sceneNodeId = typeof mount?.sceneNodeId === 'string' ? mount.sceneNodeId : undefined
    if (sceneNodeId && !nodes.some((candidate) => asObject(candidate)?.id === sceneNodeId)) {
      nodes.push({ id: sceneNodeId, sceneId, kind: 'mount', ordinal: nodes.length })
    }
    const snapshot = {
      ...(existing ?? {}), scene: structuredClone(currentScene), nodes, mounts,
      windows: Array.isArray(existing?.windows) ? existing.windows : []
    }
    if (index >= 0) snapshots[index] = snapshot
    else snapshots.push(snapshot)
    this.#hierarchyMeta = { ...this.#hierarchyMeta, sceneSnapshots: snapshots }
  }
}

function isSessionGraphEvent(eventType: string): boolean {
  return eventType === 'scene.canvas-created' ||
    eventType === 'session.canvas-membership-created' ||
    eventType === 'session.structural-relation-created' ||
    eventType === 'session.user-interacted' ||
    eventType === 'session.mode-changed' ||
    eventType === 'session.restore-state-changed' ||
    eventType === 'session.graph-summary-changed' ||
    eventType === 'session.stopped-state-changed'
}

function asSessionGraph(value: unknown): SessionGraphProjection | undefined {
  const object = asObject(value)
  if (!object || typeof object.sceneId !== 'string' || !Array.isArray(object.nodes) || !Array.isArray(object.edges)) {
    return undefined
  }
  return object as unknown as SessionGraphProjection
}

function isActive(entity: Entity): boolean {
  return entity.archivedAt === undefined && entity.status !== 'archived'
}

function replaceMap(target: Map<string, Entity>, entities: Entity[]): void {
  target.clear()
  for (const entity of entities) target.set(entity.id, structuredClone(entity))
}
function upsertEntity(target: Map<string, Entity>, entity: Entity): void {
  target.set(entity.id, { ...(target.get(entity.id) ?? {}), ...structuredClone(entity) })
}
function patchEntity(target: Map<string, Entity>, id: string, patch: unknown): void {
  const current = target.get(id)
  const object = asObject(patch)
  if (current && object) target.set(id, { ...current, ...object })
}
function projectionSortKey(index: number): string {
  return `a${index.toString().padStart(8, '0')}`
}
function commandEntityTarget(
  type: string | undefined,
  workspaces: Map<string, Entity>,
  tasks: Map<string, Entity>,
  sessions: Map<string, Entity>,
  scenes: Map<string, Entity>
): Map<string, Entity> | undefined {
  if (!type) return undefined
  if ([
    'hierarchy.rename-workspace', 'hierarchy.relink-workspace', 'hierarchy.set-workspace-pinned'
  ].includes(type)) return workspaces
  if (['hierarchy.rename-task', 'hierarchy.set-task-pinned'].includes(type)) return tasks
  if (['hierarchy.rename-scene'].includes(type)) return scenes
  if (['session.set-permission-mode', 'session.set-model'].includes(type)) return sessions
  return undefined
}
function unarchiveEntity(target: Map<string, Entity>, id: string, patch: unknown): void {
  const current = target.get(id)
  const object = asObject(patch)
  if (!current) return
  const next = { ...current, ...(object ?? {}) }
  delete next.archivedAt
  target.set(id, next)
}
function asObject(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}
function asEntity(value: unknown): Entity | undefined {
  const object = asObject(value)
  return object && typeof object.id === 'string' ? object as Entity : undefined
}
