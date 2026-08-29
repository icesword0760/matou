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
  navigation: Record<string, unknown>
  pathStates?: unknown[]
  taskPlacements?: unknown[]
  sceneSnapshots?: unknown[]
  unreadByTask?: Record<string, number>
  sessionHuds?: unknown[]
  sessionGraphs?: Record<string, SessionGraphProjection>
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
    } else if (event.eventType === 'workspace.renamed') {
      patchEntity(this.#workspaces, event.aggregateId, event.payload)
    } else if (event.eventType === 'task.created' || event.eventType === 'task.updated') {
      if (payload) this.#tasks.set(event.aggregateId, payload)
    } else if (event.eventType === 'task.renamed') {
      patchEntity(this.#tasks, event.aggregateId, event.payload)
    } else if (event.eventType === 'task.archived') {
      patchEntity(this.#tasks, event.aggregateId, { ...(asObject(event.payload) ?? {}), status: 'archived' })
    } else if (event.eventType === 'session.created') {
      if (payload) this.#sessions.set(event.aggregateId, payload)
    } else if (event.eventType === 'session.updated') {
      const session = asEntity(asObject(event.payload)?.session)
      if (session) this.#sessions.set(event.aggregateId, session)
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
    } else if (event.eventType === 'scene.mode-changed' || event.eventType === 'scene.archived') {
      patchEntity(this.#scenes, event.aggregateId, event.payload)
    } else if (isSessionGraphEvent(event.eventType)) {
      const graph = asSessionGraph(asObject(event.payload)?.graph)
      if (graph) this.#sessionGraphs[graph.sceneId] = structuredClone(graph)
    }
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
    eventType === 'session.historical-state-changed'
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
function patchEntity(target: Map<string, Entity>, id: string, patch: unknown): void {
  const current = target.get(id)
  const object = asObject(patch)
  if (current && object) target.set(id, { ...current, ...object })
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
