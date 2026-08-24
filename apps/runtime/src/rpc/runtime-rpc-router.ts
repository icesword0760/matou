import type { RpcMethod } from '@matou/contracts'
import type {
  DomainCommandMetadata,
  RelationKind,
  SceneMode,
  SessionKind,
  SessionStatus,
  TaskStatus
} from '@matou/domain'

import { SessionRepository } from '../domain/session-repository'
import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { DomainEventStore } from '../events/domain-event-store'
import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { WorkspacePathService } from '../hierarchy/workspace-path-service'
import { SessionRelationRepository } from '../relations/session-relation-repository'
import { GeometryRepository } from '../scenes/geometry-repository'
import { SceneRepository } from '../scenes/scene-repository'
import type { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'

export type RpcFaultCode =
  | 'INVALID_REQUEST'
  | 'NOT_FOUND'
  | 'CONFLICT'
  | 'INTERNAL_ERROR'

export class RpcFault extends Error {
  readonly code: RpcFaultCode
  readonly retryable: boolean

  constructor(code: RpcFaultCode, message: string, retryable = false) {
    super(message)
    this.name = 'RpcFault'
    this.code = code
    this.retryable = retryable
  }
}

export class RuntimeRpcRouter {
  readonly #database: RuntimeDatabase
  readonly #workspaces: WorkspaceTaskRepository
  readonly #sessions: SessionRepository
  readonly #relations: SessionRelationRepository
  readonly #scenes: SceneRepository
  readonly #geometry: GeometryRepository
  readonly #events: DomainEventStore
  readonly #hierarchy: HierarchyApplicationService
  readonly #workspacePaths: WorkspacePathService

  constructor(database: RuntimeDatabase) {
    this.#database = database
    const transactions = new DomainTransactionManager(database)
    this.#workspaces = new WorkspaceTaskRepository(database, transactions)
    this.#sessions = new SessionRepository(database, transactions)
    this.#relations = new SessionRelationRepository(database, transactions)
    this.#scenes = new SceneRepository(database, transactions)
    this.#geometry = new GeometryRepository(database)
    this.#events = new DomainEventStore(database)
    this.#hierarchy = new HierarchyApplicationService(database, transactions)
    this.#workspacePaths = new WorkspacePathService(database, transactions)
  }

  async handle(method: RpcMethod, payload: unknown): Promise<unknown> {
    try {
      return await this.#dispatch(method, payload)
    } catch (error) {
      if (error instanceof RpcFault) throw error
      const message = errorMessage(error)
      if (/does not exist|missing/i.test(message)) throw new RpcFault('NOT_FOUND', message)
      if (/already|conflict|stale|must|cannot|archived|cycle|belong/i.test(message)) {
        throw new RpcFault('CONFLICT', message)
      }
      throw error
    }
  }

  async #dispatch(method: RpcMethod, payload: unknown): Promise<unknown> {
    if (method === 'projection.snapshot') return this.#snapshot()
    if (method === 'events.replay') {
      const value = record(payload)
      const afterSequence = integer(value.afterSequence, 'afterSequence', 0)
      const limit = integer(value.limit, 'limit', 1)
      return { events: this.#events.readAfter(afterSequence, limit) }
    }
    if (method === 'events.ack') {
      const value = record(payload)
      this.#events.acknowledge(
        text(value.consumerId, 'consumerId'),
        integer(value.throughSequence, 'throughSequence', 0)
      )
      return { acknowledged: true }
    }
    if (method === 'geometry.put') {
      const input = record(payload)
      return this.#geometry.put({
        sceneId: text(input.sceneId, 'sceneId'),
        ownerKey: text(input.ownerKey, 'ownerKey'),
        layoutRevision: integer(input.layoutRevision, 'layoutRevision', 0),
        geometry: input.geometry,
        now: integer(input.now, 'now', 0)
      })
    }

    const envelope = record(payload)
    const command = commandMetadata(envelope.command)
    const input = record(envelope.input)
    switch (method) {
      case 'hierarchy.bootstrap-window':
        return this.#withActivePathState(this.#hierarchy.bootstrapWindow(command, {
          windowId: text(input.windowId, 'windowId'),
          defaultRootDirectory: text(input.defaultRootDirectory, 'defaultRootDirectory'),
          defaultName: text(input.defaultName, 'defaultName'),
          now: integer(input.now, 'now', 0)
        }))
      case 'hierarchy.create-workspace':
        return this.#withActivePathState(this.#hierarchy.createWorkspace(command, {
          windowId: text(input.windowId, 'windowId'),
          name: text(input.name, 'name'),
          rootDirectory: text(input.rootDirectory, 'rootDirectory'),
          now: integer(input.now, 'now', 0)
        }))
      case 'hierarchy.rename-workspace':
        return this.#hierarchy.renameWorkspace(command, {
          workspaceId: text(input.workspaceId, 'workspaceId'),
          name: text(input.name, 'name'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.remove-workspace':
        return this.#hierarchy.removeWorkspace(command, {
          windowId: text(input.windowId, 'windowId'),
          workspaceId: text(input.workspaceId, 'workspaceId'),
          confirmedIntent: text(input.confirmedIntent, 'confirmedIntent'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.activate-workspace':
        return this.#withActivePathState(this.#hierarchy.activateWorkspace({
          windowId: text(input.windowId, 'windowId'),
          workspaceId: text(input.workspaceId, 'workspaceId'),
          now: integer(input.now, 'now', 0)
        }))
      case 'hierarchy.validate-workspace-path':
        return this.#workspacePaths.validateWorkspace(
          text(input.workspaceId, 'workspaceId')
        )
      case 'hierarchy.create-task':
        return this.#hierarchy.createTask(command, {
          windowId: text(input.windowId, 'windowId'),
          workspaceId: text(input.workspaceId, 'workspaceId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.rename-task':
        return this.#hierarchy.renameTask(command, {
          taskId: text(input.taskId, 'taskId'),
          title: text(input.title, 'title'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.reorder-task':
        return this.#hierarchy.reorderTask(command, {
          windowId: text(input.windowId, 'windowId'),
          workspaceId: text(input.workspaceId, 'workspaceId'),
          taskId: text(input.taskId, 'taskId'),
          ...(optionalText(input.beforeTaskId, 'beforeTaskId') === undefined
            ? {}
            : { beforeTaskId: optionalText(input.beforeTaskId, 'beforeTaskId')! }),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.delete-task':
        return this.#hierarchy.deleteTask(command, {
          windowId: text(input.windowId, 'windowId'),
          taskId: text(input.taskId, 'taskId'),
          confirmedIntent: text(input.confirmedIntent, 'confirmedIntent'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.activate-task':
        return this.#hierarchy.activateTask({
          windowId: text(input.windowId, 'windowId'),
          taskId: text(input.taskId, 'taskId'),
          now: integer(input.now, 'now', 0)
        })
      case 'workspace.create':
        return this.#workspaces.createWorkspace(command, {
          id: text(input.id, 'id'), name: text(input.name, 'name'),
          rootDirectory: text(input.rootDirectory, 'rootDirectory'),
          ...(optionalText(input.pathIdentity, 'pathIdentity') === undefined ? {} : { pathIdentity: optionalText(input.pathIdentity, 'pathIdentity')! }),
          now: integer(input.now, 'now', 0)
        })
      case 'workspace.update':
        return this.#workspaces.updateWorkspace(command, {
          id: text(input.id, 'id'),
          ...(optionalText(input.name, 'name') === undefined ? {} : { name: optionalText(input.name, 'name')! }),
          ...(optionalText(input.rootDirectory, 'rootDirectory') === undefined ? {} : { rootDirectory: optionalText(input.rootDirectory, 'rootDirectory')! }),
          now: integer(input.now, 'now', 0)
        })
      case 'workspace.archive':
        return this.#workspaces.archiveWorkspace(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'execution-context.create-plain':
        return this.#workspaces.createPlainExecutionContext(command, {
          id: text(input.id, 'id'), workspaceId: text(input.workspaceId, 'workspaceId'),
          cwd: text(input.cwd, 'cwd'), now: integer(input.now, 'now', 0)
        })
      case 'task.create':
        return this.#workspaces.createTask(command, {
          id: text(input.id, 'id'), workspaceId: text(input.workspaceId, 'workspaceId'),
          ...(optionalText(input.parentTaskId, 'parentTaskId') === undefined ? {} : { parentTaskId: optionalText(input.parentTaskId, 'parentTaskId')! }),
          executionContextId: text(input.executionContextId, 'executionContextId'),
          title: text(input.title, 'title'),
          status: enumeration(input.status, ['planned', 'active', 'blocked', 'completed'] as const, 'status'),
          sortKey: text(input.sortKey, 'sortKey'), now: integer(input.now, 'now', 0)
        })
      case 'task.update': {
        const parent = optionalNullableText(input, 'parentTaskId')
        return this.#workspaces.updateTask(command, {
          id: text(input.id, 'id'),
          ...(optionalText(input.title, 'title') === undefined ? {} : { title: optionalText(input.title, 'title')! }),
          ...(input.status === undefined ? {} : { status: enumeration(input.status, ['planned', 'active', 'blocked', 'completed'] as const, 'status') }),
          ...(parent.present ? { parentTaskId: parent.value } : {}),
          ...(optionalText(input.executionContextId, 'executionContextId') === undefined ? {} : { executionContextId: optionalText(input.executionContextId, 'executionContextId')! }),
          ...(optionalText(input.sortKey, 'sortKey') === undefined ? {} : { sortKey: optionalText(input.sortKey, 'sortKey')! }),
          now: integer(input.now, 'now', 0)
        })
      }
      case 'task.archive':
        return this.#workspaces.archiveTask(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'session.create':
        return this.#sessions.createSession(command, {
          id: text(input.id, 'id'), taskId: text(input.taskId, 'taskId'),
          executionContextId: text(input.executionContextId, 'executionContextId'),
          kind: enumeration(input.kind, ['shell', 'claude-code', 'codex', 'agent-team-member'] as const, 'kind') as SessionKind,
          title: text(input.title, 'title'), now: integer(input.now, 'now', 0)
        })
      case 'session.update':
        return this.#sessions.updateSession(command, {
          id: text(input.id, 'id'),
          ...(optionalText(input.title, 'title') === undefined ? {} : { title: optionalText(input.title, 'title')! }),
          ...(input.status === undefined ? {} : { status: enumeration(input.status, ['created', 'starting', 'running', 'waiting', 'interrupted', 'exited'] as const, 'status') as Exclude<SessionStatus, 'archived'> }),
          now: integer(input.now, 'now', 0)
        })
      case 'session.archive':
        return this.#sessions.archiveSession(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'relation.create':
        return this.#relations.create(command, {
          id: text(input.id, 'id'), taskId: text(input.taskId, 'taskId'),
          fromSessionId: text(input.fromSessionId, 'fromSessionId'),
          toSessionId: text(input.toSessionId, 'toSessionId'),
          kind: enumeration(input.kind, ['forked-from', 'derived-from', 'depends-on', 'supports', 'blocks', 'references', 'team-member-of'] as const, 'kind') as RelationKind,
          metadata: input.metadata, now: integer(input.now, 'now', 0)
        })
      case 'relation.revoke':
        return this.#relations.revoke(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'relation.restore':
        return this.#relations.restore(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'scene.create':
        return this.#scenes.createScene(command, {
          id: text(input.id, 'id'), rootNodeId: text(input.rootNodeId, 'rootNodeId'),
          taskId: text(input.taskId, 'taskId'), name: text(input.name, 'name'),
          mode: enumeration(input.mode, ['tile', 'card', 'dag'] as const, 'mode') as SceneMode,
          now: integer(input.now, 'now', 0)
        })
      case 'scene.set-mode':
        return this.#scenes.setMode(command, text(input.id, 'id'), enumeration(input.mode, ['tile', 'card', 'dag'] as const, 'mode') as SceneMode, integer(input.now, 'now', 0))
      case 'scene.add-node':
        return this.#scenes.addNode(command, {
          id: text(input.id, 'id'), sceneId: text(input.sceneId, 'sceneId'),
          parentNodeId: text(input.parentNodeId, 'parentNodeId'),
          kind: enumeration(input.kind, ['split', 'mount', 'group'] as const, 'kind'),
          ...(input.direction === undefined ? {} : { direction: enumeration(input.direction, ['horizontal', 'vertical'] as const, 'direction') }),
          ordinal: integer(input.ordinal, 'ordinal', 0), now: integer(input.now, 'now', 0)
        })
      case 'scene.remove-node':
        return this.#scenes.removeNode(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'scene.attach-window':
        return this.#scenes.attachWindow(command, {
          id: text(input.id, 'id'), sceneId: text(input.sceneId, 'sceneId'),
          nativeWindowKey: text(input.nativeWindowKey, 'nativeWindowKey'),
          state: enumeration(input.state, ['attached', 'detached'] as const, 'state'),
          now: integer(input.now, 'now', 0)
        })
      case 'scene.detach-window':
        return this.#scenes.detachWindow(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'scene.mount-session':
        return this.#scenes.mountSession(command, {
          id: text(input.id, 'id'), sceneId: text(input.sceneId, 'sceneId'),
          sceneNodeId: text(input.sceneNodeId, 'sceneNodeId'),
          ...(optionalText(input.sceneWindowId, 'sceneWindowId') === undefined ? {} : { sceneWindowId: optionalText(input.sceneWindowId, 'sceneWindowId')! }),
          sessionId: text(input.sessionId, 'sessionId'), now: integer(input.now, 'now', 0)
        })
      case 'scene.unmount-session':
        return this.#scenes.unmountSession(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      case 'scene.archive':
        return this.#scenes.archiveScene(command, text(input.id, 'id'), integer(input.now, 'now', 0))
      default:
        throw new RpcFault('INVALID_REQUEST', `unsupported RPC method ${method}`)
    }
  }

  #snapshot(): unknown {
    const workspaces = this.#database.all<{ id: string }>('SELECT id FROM workspaces ORDER BY created_at').map(({ id }) => this.#workspaces.getWorkspace(id)!)
    const tasks = this.#database.all<{ id: string }>('SELECT id FROM tasks ORDER BY created_at').map(({ id }) => this.#workspaces.getTask(id)!)
    const sessions = this.#database.all<{ id: string }>('SELECT id FROM sessions ORDER BY created_at').map(({ id }) => this.#sessions.getSession(id)!)
    const sessionRuns = sessions.flatMap(({ id }) => this.#sessions.listRuns(id))
    const providerBindings = sessions.flatMap(({ id }) => this.#sessions.listProviderBindings(id))
    const relations = this.#database.all<{ relation_id: string }>('SELECT relation_id FROM session_relations_current ORDER BY created_at').map(({ relation_id }) => this.#relations.getCurrent(relation_id)!)
    const sceneSnapshots = this.#database.all<{ id: string }>('SELECT id FROM scenes ORDER BY created_at').map(({ id }) => this.#scenes.snapshot(id)!)
    const eventSequence = this.#database.get<{ maximum: number }>('SELECT COALESCE(MAX(seq), 0) AS maximum FROM domain_events')?.maximum ?? 0
    return {
      runtimeGeneration: this.#database.runtimeGeneration,
      eventSequence,
      workspaces,
      tasks,
      sessions,
      sessionRuns,
      providerBindings,
      relations,
      scenes: sceneSnapshots.map(({ scene }) => scene),
      sceneSnapshots
    }
  }

  async #withActivePathState<T extends { workspace: { id: string } | null }>(
    result: T
  ): Promise<T & { pathState?: Awaited<ReturnType<WorkspacePathService['validateWorkspace']>> }> {
    if (result.workspace === null) return result
    const pathState = await this.#workspacePaths.validateWorkspace(result.workspace.id)
    return { ...result, pathState }
  }
}

function commandMetadata(value: unknown): DomainCommandMetadata {
  const command = record(value)
  return {
    commandId: text(command.commandId, 'command.commandId'),
    commandType: text(command.commandType, 'command.commandType'),
    requestHash: text(command.requestHash, 'command.requestHash'),
    ...(optionalText(command.causationId, 'command.causationId') === undefined ? {} : { causationId: optionalText(command.causationId, 'command.causationId')! }),
    ...(optionalText(command.correlationId, 'command.correlationId') === undefined ? {} : { correlationId: optionalText(command.correlationId, 'command.correlationId')! })
  }
}
function record(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new RpcFault('INVALID_REQUEST', 'RPC payload must be an object')
  }
  return value as Record<string, unknown>
}
function text(value: unknown, label: string): string {
  if (typeof value !== 'string' || !value.trim()) throw new RpcFault('INVALID_REQUEST', `${label} must be a non-empty string`)
  return value.trim()
}
function optionalText(value: unknown, label: string): string | undefined {
  return value === undefined ? undefined : text(value, label)
}
function optionalNullableText(input: Record<string, unknown>, key: string): { present: boolean; value: string | null } {
  if (!Object.prototype.hasOwnProperty.call(input, key)) return { present: false, value: null }
  return { present: true, value: input[key] === null ? null : text(input[key], key) }
}
function integer(value: unknown, label: string, minimum: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < minimum) {
    throw new RpcFault('INVALID_REQUEST', `${label} must be an integer >= ${minimum}`)
  }
  return value as number
}
function enumeration<const T extends readonly string[]>(value: unknown, values: T, label: string): T[number] {
  if (typeof value !== 'string' || !values.includes(value)) {
    throw new RpcFault('INVALID_REQUEST', `${label} must be one of ${values.join(', ')}`)
  }
  return value as T[number]
}
function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
