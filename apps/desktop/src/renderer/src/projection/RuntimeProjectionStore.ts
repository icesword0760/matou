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
  readonly #workspaces = new ProjectionCollection()
  readonly #tasks = new ProjectionCollection()
  readonly #sessions = new ProjectionCollection()
  readonly #relations = new ProjectionCollection()
  readonly #scenes = new ProjectionCollection()
  #sessionGraphs: Record<string, SessionGraphProjection> = {}
  readonly #graphLocationsBySession = new Map<string, Array<{ sceneId: string; index: number }>>()
  #hierarchyMeta: Omit<HierarchyProjection, 'workspaces' | 'tasks' | 'sessions' | 'scenes'> = {
    windowId: 'window-1', navigation: { windowId: 'window-1' }
  }
  #viewCache: RuntimeProjectionView | undefined

  get eventSequence(): number {
    return this.#eventSequence
  }

  replace(snapshot: RuntimeProjectionSnapshot): void {
    this.#runtimeGeneration = snapshot.runtimeGeneration
    this.#eventSequence = snapshot.eventSequence
    this.#workspaces.replace(snapshot.workspaces)
    this.#tasks.replace(snapshot.tasks)
    this.#sessions.replace(snapshot.sessions)
    this.#relations.replace(snapshot.relations)
    this.#scenes.replace(snapshot.scenes)
    this.#replaceAllSessionGraphs(snapshot.sessionGraphs ?? {})
    if (snapshot.hierarchy) {
      const { workspaces: _workspaces, tasks: _tasks, sessions: _sessions, scenes: _scenes, ...meta } = snapshot.hierarchy
      this.#hierarchyMeta = structuredClone(meta)
    }
    this.#viewCache = undefined
  }

  applyBatch(runtimeGeneration: string, events: DomainEventWireEnvelope[]): void {
    if (this.#runtimeGeneration !== runtimeGeneration) {
      throw new Error('runtime generation changed; a fresh projection snapshot is required')
    }
    const pendingGraphs = new Map<string, SessionGraphProjection>()
    let changed = false
    try {
      for (const event of events) {
        if (event.sequence <= this.#eventSequence) continue
        if (event.sequence !== this.#eventSequence + 1) {
          throw new Error(
            `projection event gap: expected ${this.#eventSequence + 1}, received ${event.sequence}`
          )
        }
        this.#apply(event, pendingGraphs)
        this.#eventSequence = event.sequence
        changed = true
      }
    } finally {
      this.#flushPendingGraphs(pendingGraphs)
      if (changed) this.#viewCache = undefined
    }
  }

  applyCommandResult(
    result: unknown,
    context?: { type: string; input: Record<string, unknown> }
  ): void {
    this.#viewCache = undefined
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
          if (graph && typeof sessionId === 'string' && graph.focusedSessionId !== sessionId) {
            this.#sessionGraphs = {
              ...this.#sessionGraphs,
              [sceneId]: { ...graph, focusedSessionId: sessionId }
            }
          }
        }
      }
    }
    const returnedTask = asEntity(object.task)
    if (
      returnedTask &&
      (context?.type === 'hierarchy.create-task' || context?.type === 'hierarchy.create-workspace')
    ) {
      this.#ensureTaskPlacement(returnedTask)
    }
    const graph = asSessionGraph(object.graph) ?? asSessionGraph(object)
    if (graph) {
      this.#replaceSessionGraph(graph)
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
    this.#viewCache = undefined
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
    this.#viewCache = undefined
  }

  applySceneGraph(graph: SessionGraphProjection): void {
    this.#replaceSessionGraph(graph)
    this.#viewCache = undefined
  }

  view(): RuntimeProjectionView {
    if (!this.#runtimeGeneration) throw new Error('projection snapshot has not been loaded')
    if (this.#viewCache) return this.#viewCache
    const workspaces = this.#workspaces.values
    const tasks = this.#tasks.values
    const sessions = this.#sessions.values
    const scenes = this.#scenes.values
    const sessionGraphs = this.#sessionGraphs
    this.#viewCache = {
      runtimeGeneration: this.#runtimeGeneration,
      eventSequence: this.#eventSequence,
      workspaces,
      tasks,
      sessions,
      relations: this.#relations.values,
      scenes,
      sessionGraphs,
      hierarchy: {
        ...this.#hierarchyMeta,
        workspaces: this.#workspaces.activeValues,
        tasks: this.#tasks.activeValues,
        sessions: this.#sessions.activeValues,
        scenes: this.#scenes.activeValues,
        closedScenes: this.#scenes.inactiveValues,
        sessionGraphs
      }
    }
    return this.#viewCache
  }

  #apply(
    event: DomainEventWireEnvelope,
    pendingGraphs: Map<string, SessionGraphProjection>
  ): void {
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
      // Preserve event ordering when a rare batch interleaves a full graph and
      // a cwd update. Ordinary cwd events use the session-to-node location
      // index and touch only the owning graph.
      this.#flushPendingGraphs(pendingGraphs)
      this.#patchSessionGraphNodes(event.aggregateId, asObject(event.payload) ?? {})
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
      if (graph) pendingGraphs.set(graph.sceneId, graph)
    }
  }

  #flushPendingGraphs(pendingGraphs: Map<string, SessionGraphProjection>): void {
    if (pendingGraphs.size === 0) return
    this.#replaceSessionGraphs(pendingGraphs.values())
    pendingGraphs.clear()
  }

  #replaceAllSessionGraphs(graphs: Record<string, SessionGraphProjection>): void {
    this.#sessionGraphs = {}
    this.#graphLocationsBySession.clear()
    this.#replaceSessionGraphs(Object.values(graphs))
  }

  #replaceSessionGraph(graph: SessionGraphProjection): void {
    this.#replaceSessionGraphs([graph])
  }

  #replaceSessionGraphs(graphs: Iterable<SessionGraphProjection>): void {
    const nextGraphs = { ...this.#sessionGraphs }
    for (const graph of graphs) {
      const previous = nextGraphs[graph.sceneId]
      if (previous) this.#removeGraphLocations(previous)
      // RPC and MessagePort payloads already cross a structured-clone boundary.
      // Treat their arrays as immutable and clone only the graph shell; later
      // node patches use copy-on-write so published views remain stable.
      const next = { ...graph, nodes: graph.nodes, edges: graph.edges }
      nextGraphs[graph.sceneId] = next
      this.#indexGraphLocations(next)
    }
    this.#sessionGraphs = nextGraphs
  }

  #patchSessionGraphNodes(sessionId: string, patch: Record<string, unknown>): void {
    const locations = this.#graphLocationsBySession.get(sessionId)
    if (!locations || locations.length === 0) return
    const graphs = { ...this.#sessionGraphs }
    for (const { sceneId, index } of locations) {
      const current = graphs[sceneId]
      const node = current?.nodes[index]
      if (!current || !node || node.sessionId !== sessionId) continue
      const nodes = current.nodes.slice()
      nodes[index] = { ...node, ...patch }
      graphs[sceneId] = { ...current, nodes }
    }
    this.#sessionGraphs = graphs
  }

  #indexGraphLocations(graph: SessionGraphProjection): void {
    graph.nodes.forEach((node, index) => {
      const locations = this.#graphLocationsBySession.get(node.sessionId)
      const location = { sceneId: graph.sceneId, index }
      if (locations) locations.push(location)
      else this.#graphLocationsBySession.set(node.sessionId, [location])
    })
  }

  #removeGraphLocations(graph: SessionGraphProjection): void {
    for (const node of graph.nodes) {
      const locations = this.#graphLocationsBySession.get(node.sessionId)
      if (!locations) continue
      const remaining = locations.filter(({ sceneId }) => sceneId !== graph.sceneId)
      if (remaining.length > 0) this.#graphLocationsBySession.set(node.sessionId, remaining)
      else this.#graphLocationsBySession.delete(node.sessionId)
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

  #ensureTaskPlacement(task: Entity): void {
    const navigation = asObject(this.#hierarchyMeta.navigation)
    const windowId = typeof navigation?.windowId === 'string'
      ? navigation.windowId
      : this.#hierarchyMeta.windowId
    const placements = Array.isArray(this.#hierarchyMeta.taskPlacements)
      ? [...this.#hierarchyMeta.taskPlacements]
      : []
    if (placements.some((candidate) => {
      const placement = asObject(candidate)
      return placement?.windowId === windowId && placement.taskId === task.id
    })) return
    const ordinals = placements.flatMap((candidate) => {
      const placement = asObject(candidate)
      return placement?.windowId === windowId && typeof placement.ordinal === 'number'
        ? [placement.ordinal]
        : []
    })
    placements.push({
      windowId,
      taskId: task.id,
      ordinal: ordinals.length === 0 ? 0 : Math.max(...ordinals) + 1,
      updatedAt: typeof task.updatedAt === 'number' ? task.updatedAt : Date.now()
    })
    this.#hierarchyMeta = { ...this.#hierarchyMeta, taskPlacements: placements }
  }

  #applyOrder(target: ProjectionCollection, ids: unknown, key: string): void {
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
      patchEntity(target[0] as ProjectionCollection, target[1], { status: 'archived' })
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

class ProjectionCollection {
  readonly #byId = new Map<string, Entity>()
  readonly #indexById = new Map<string, number>()
  readonly #activeIndexById = new Map<string, number>()
  readonly #inactiveIndexById = new Map<string, number>()
  #values: Entity[] = []
  #activeValues: Entity[] = []
  #inactiveValues: Entity[] = []

  get values(): Entity[] { return this.#values }
  get activeValues(): Entity[] { return this.#activeValues }
  get inactiveValues(): Entity[] { return this.#inactiveValues }

  get(id: string): Entity | undefined { return this.#byId.get(id) }

  replace(entities: Entity[]): void {
    this.#byId.clear()
    this.#indexById.clear()
    this.#values = entities.map((entity) => ({ ...entity }))
    this.#values.forEach((entity, index) => {
      this.#byId.set(entity.id, entity)
      this.#indexById.set(entity.id, index)
    })
    this.#rebuildMembership()
  }

  set(id: string, entity: Entity): void {
    const next = { ...entity, id }
    const previous = this.#byId.get(id)
    const index = this.#indexById.get(id)
    this.#byId.set(id, next)
    if (index === undefined) {
      this.#indexById.set(id, this.#values.length)
      this.#values = [...this.#values, next]
      if (isActive(next)) {
        this.#activeIndexById.set(id, this.#activeValues.length)
        this.#activeValues = [...this.#activeValues, next]
      } else {
        this.#inactiveIndexById.set(id, this.#inactiveValues.length)
        this.#inactiveValues = [...this.#inactiveValues, next]
      }
      return
    }
    const values = this.#values.slice()
    values[index] = next
    this.#values = values
    if (previous && isActive(previous) === isActive(next)) {
      const memberIndex = isActive(next)
        ? this.#activeIndexById.get(id)
        : this.#inactiveIndexById.get(id)
      if (memberIndex !== undefined) {
        const members = (isActive(next) ? this.#activeValues : this.#inactiveValues).slice()
        members[memberIndex] = next
        if (isActive(next)) this.#activeValues = members
        else this.#inactiveValues = members
      }
      return
    }
    this.#rebuildMembership()
  }

  delete(id: string): void {
    const index = this.#indexById.get(id)
    if (index === undefined) return
    this.#byId.delete(id)
    this.#values = this.#values.filter((entity) => entity.id !== id)
    this.#rebuildIndexes()
    this.#rebuildMembership()
  }

  #rebuildIndexes(): void {
    this.#indexById.clear()
    this.#values.forEach((entity, index) => this.#indexById.set(entity.id, index))
  }

  #rebuildMembership(): void {
    this.#activeValues = []
    this.#inactiveValues = []
    this.#activeIndexById.clear()
    this.#inactiveIndexById.clear()
    for (const entity of this.#values) {
      if (isActive(entity)) {
        this.#activeIndexById.set(entity.id, this.#activeValues.length)
        this.#activeValues.push(entity)
      } else {
        this.#inactiveIndexById.set(entity.id, this.#inactiveValues.length)
        this.#inactiveValues.push(entity)
      }
    }
  }
}

function upsertEntity(target: ProjectionCollection, entity: Entity): void {
  target.set(entity.id, { ...(target.get(entity.id) ?? {}), ...entity })
}
function patchEntity(target: ProjectionCollection, id: string, patch: unknown): void {
  const current = target.get(id)
  const object = asObject(patch)
  if (current && object) target.set(id, { ...current, ...object })
}
function projectionSortKey(index: number): string {
  return `a${index.toString().padStart(8, '0')}`
}
function commandEntityTarget(
  type: string | undefined,
  workspaces: ProjectionCollection,
  tasks: ProjectionCollection,
  sessions: ProjectionCollection,
  scenes: ProjectionCollection
): ProjectionCollection | undefined {
  if (!type) return undefined
  if ([
    'hierarchy.rename-workspace', 'hierarchy.relink-workspace', 'hierarchy.set-workspace-pinned'
  ].includes(type)) return workspaces
  if (['hierarchy.rename-task', 'hierarchy.set-task-pinned'].includes(type)) return tasks
  if (['hierarchy.rename-scene'].includes(type)) return scenes
  if (['session.set-permission-mode', 'session.set-model'].includes(type)) return sessions
  return undefined
}
function unarchiveEntity(target: ProjectionCollection, id: string, patch: unknown): void {
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
