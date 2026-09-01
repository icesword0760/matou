import { randomUUID } from 'node:crypto'
import { homedir } from 'node:os'
import { dirname, resolve } from 'node:path'

import type { RpcMethod } from '@matou/contracts'
import type {
  DomainCommandMetadata,
  LayoutNode,
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
import { DetachedSessionService } from '../hierarchy/detached-session-service'
import { TaskWindowMigrationService } from '../hierarchy/task-window-migration-service'
import { WorkspacePathService } from '../hierarchy/workspace-path-service'
import { SceneLayoutService } from '../hierarchy/scene-layout-service'
import { NavigationRepository } from '../hierarchy/navigation-repository'
import { SessionRelationRepository } from '../relations/session-relation-repository'
import { GeometryRepository } from '../scenes/geometry-repository'
import { SceneRepository } from '../scenes/scene-repository'
import { SessionGraphRepository } from '../session-canvas/session-graph-repository'
import { SessionCanvasService } from '../session-canvas/session-canvas-service'
import { SessionInteractionService } from '../session-canvas/session-interaction-service'
import { ProviderModeService } from '../session-canvas/provider-mode-service'
import { ForkWorkflowError, ForkWorkflowService } from '../session-canvas/fork-workflow-service'
import type { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { NotificationProjection } from '../product/experience-foundation'
import { GitWorkspaceService } from '../git/git-workspace-service'
import { ClaudeSessionCatalog } from '../session/claude-session-catalog'

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
  readonly #sceneLayouts: SceneLayoutService
  readonly #navigation: NavigationRepository
  readonly #detachedSessions: DetachedSessionService
  readonly #taskWindowMigrations: TaskWindowMigrationService
  readonly #notifications: NotificationProjection
  readonly #sessionGraphs: SessionGraphRepository
  readonly #sessionCanvas: SessionCanvasService
  readonly #sessionInteractions: SessionInteractionService
  readonly #providerModes: ProviderModeService
  readonly #forkWorkflows: ForkWorkflowService
  readonly #git: GitWorkspaceService
  readonly #claudeSessions: ClaudeSessionCatalog

  constructor(
    database: RuntimeDatabase,
    notifications = new NotificationProjection(),
    options: { projectsRoot?: string } = {}
  ) {
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
    this.#sceneLayouts = new SceneLayoutService(database, transactions)
    this.#navigation = new NavigationRepository(database)
    this.#detachedSessions = new DetachedSessionService(database, transactions)
    this.#taskWindowMigrations = new TaskWindowMigrationService(database, transactions)
    this.#notifications = notifications
    this.#sessionGraphs = new SessionGraphRepository(database, transactions)
    this.#sessionCanvas = new SessionCanvasService(database, transactions)
    this.#sessionInteractions = new SessionInteractionService(database, transactions)
    this.#providerModes = new ProviderModeService(database, transactions)
    this.#forkWorkflows = new ForkWorkflowService(
      dirname(database.path), database, transactions, { stopRuns: async () => undefined }
    )
    this.#git = new GitWorkspaceService({ database, dataRoot: dirname(database.path) })
    this.#claudeSessions = new ClaudeSessionCatalog(
      options.projectsRoot ?? process.env.MATOU_CLAUDE_PROJECTS_ROOT ??
        resolve(homedir(), '.claude', 'projects')
    )
  }

  async handle(method: RpcMethod, payload: unknown): Promise<unknown> {
    try {
      return await this.#dispatch(method, payload)
    } catch (error) {
      if (error instanceof RpcFault) throw error
      if (error instanceof ForkWorkflowError) {
        const invalidName = ['EMPTY_NAME', 'TOO_LONG_NAME'].includes(error.code)
        throw new RpcFault(invalidName ? 'INVALID_REQUEST' : 'CONFLICT', error.message)
      }
      const message = errorMessage(error)
      if (/does not exist|missing/i.test(message)) throw new RpcFault('NOT_FOUND', message)
      if (/already|conflict|stale|must|cannot|archived|cycle|belong/i.test(message)) {
        throw new RpcFault('CONFLICT', message)
      }
      throw error
    }
  }

  async #dispatch(method: RpcMethod, payload: unknown): Promise<unknown> {
    if (method === 'projection.snapshot') return this.#snapshot(payload)
    if (method === 'hierarchy.get-scene-session-graph') {
      const input = record(payload)
      return this.#sessionGraphs.projectSceneGraph(
        text(input.sceneId, 'sceneId'),
        optionalText(input.windowId, 'windowId')
      )
    }
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
    if (method === 'geometry.list') {
      const input = record(payload)
      return this.#geometry.list(text(input.sceneId, 'sceneId'))
    }
    if (method === 'claude-sessions.list') {
      const input = record(payload)
      const sessionId = text(input.sessionId, 'sessionId')
      const result = await this.#claudeSessions.list({
        cwd: this.#sessionCwd(sessionId),
        query: optionalString(input.query),
        limit: optionalInteger(input.limit, 100)
      })
      const scopeProviderSessionId = optionalText(input.providerSessionId, 'providerSessionId')
      const sessions = result.sessions
        .filter(({ providerSessionId }) =>
          scopeProviderSessionId === undefined || providerSessionId === scopeProviderSessionId)
        .map((session) => ({
          ...session,
          ...this.#providerConversationUsage(session.providerSessionId, sessionId)
        }))
      return { sessions, total: scopeProviderSessionId === undefined ? result.total : sessions.length }
    }
    if (method === 'claude-sessions.detail') {
      const input = record(payload)
      const sessionId = text(input.sessionId, 'sessionId')
      const providerSessionId = text(input.providerSessionId, 'providerSessionId')
      const detail = await this.#claudeSessions.detail({
        cwd: this.#sessionCwd(sessionId), providerSessionId,
        query: optionalString(input.query)
      })
      return {
        ...detail,
        ...this.#providerConversationUsage(providerSessionId, sessionId)
      }
    }

    const envelope = record(payload)
    const command = commandMetadata(envelope.command)
    const input = record(envelope.input)
    switch (method) {
      case 'claude-sessions.load': {
        const sessionId = text(input.sessionId, 'sessionId')
        const providerSessionId = text(input.providerSessionId, 'providerSessionId')
        const detail = await this.#claudeSessions.detail({
          cwd: this.#sessionCwd(sessionId), providerSessionId, query: ''
        })
        const result = this.#providerModes.loadClaudeSession(command, {
          sessionId,
          bindingId: randomUUID(),
          providerSessionId,
          title: detail.title,
          permissionMode: detail.permissionMode,
          ...(detail.model ? { model: detail.model } : {}),
          now: integer(input.now, 'now', 0)
        })
        return {
          ...result,
          load: {
            sessionId, providerSessionId, permissionMode: detail.permissionMode
          }
        }
      }
      case 'git.status':
        return this.#git.status(text(input.cwd, 'cwd'))
      case 'git.checkout':
        return this.#git.checkout(
          text(input.cwd, 'cwd'), text(input.branch, 'branch')
        )
      case 'git.create-branch':
        return this.#git.createBranch(
          text(input.cwd, 'cwd'), text(input.branch, 'branch')
        )
      case 'git.commit':
        return this.#git.commit(text(input.cwd, 'cwd'), {
          message: text(input.message, 'message'),
          includeUnstaged: flag(input.includeUnstaged, 'includeUnstaged')
        })
      case 'git.push':
        return this.#git.push(text(input.cwd, 'cwd'))
      case 'git.worktree-create': {
        const owner = this.#sessionOwner(text(input.sessionId, 'sessionId'))
        const status = await this.#git.status(text(input.cwd, 'cwd'))
        return this.#git.createWorktree(command, {
          workspaceId: owner.workspaceId,
          repositoryRoot: status.repositoryRoot,
          branch: text(input.branch, 'branch'),
          baseRef: optionalText(input.baseRef, 'baseRef') ?? status.currentBranch ?? 'HEAD',
          now: integer(input.now, 'now', 0)
        })
      }
      case 'git.worktree-open': {
        const sourceSessionId = text(input.sessionId, 'sessionId')
        const owner = this.#sessionOwner(sourceSessionId)
        const context = await this.#git.ensureWorktreeContext(
          derivedCommand(command, 'worktree-context'), {
            workspaceId: owner.workspaceId,
            repositoryRoot: text(input.repositoryRoot, 'repositoryRoot'),
            path: text(input.path, 'path'),
            branch: text(input.branch, 'branch'),
            now: integer(input.now, 'now', 0)
          }
        )
        const sceneId = text(input.sceneId, 'sceneId')
        const existing = this.#database.get<{ id: string }>(
          `SELECT sessions.id FROM sessions
           JOIN session_canvas_memberships AS membership
             ON membership.session_id = sessions.id
           WHERE membership.scene_id = ? AND sessions.execution_context_id = ?
             AND sessions.archived_at IS NULL
           ORDER BY sessions.last_activity_at DESC, sessions.created_at DESC LIMIT 1`,
          sceneId, context.executionContextId
        )
        const now = integer(input.now, 'now', 0)
        if (existing) {
          return {
            created: false,
            focusedSessionId: existing.id,
            graph: this.#sessionCanvas.setFocusedSession({
              windowId: text(input.windowId, 'windowId'), sceneId,
              sessionId: existing.id, now
            })
          }
        }
        const result = this.#sessionCanvas.createShellSibling(
          derivedCommand(command, 'worktree-shell'), {
            windowId: text(input.windowId, 'windowId'), sceneId,
            sourceSessionId, executionContextId: context.executionContextId, now
          }
        )
        return { ...result, created: true, focusedSessionId: result.session?.id }
      }
      case 'git.worktree-remove':
        return this.#git.removeWorktree(command, {
          worktreeId: text(input.worktreeId, 'worktreeId'),
          now: integer(input.now, 'now', 0)
        })
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
      case 'hierarchy.relink-workspace':
        return this.#hierarchy.relinkWorkspace(command, {
          workspaceId: text(input.workspaceId, 'workspaceId'),
          rootDirectory: text(input.rootDirectory, 'rootDirectory'),
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
      case 'hierarchy.set-workspace-pinned':
        return this.#hierarchy.setWorkspacePinned(command, {
          workspaceId: text(input.workspaceId, 'workspaceId'),
          pinned: flag(input.pinned, 'pinned'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.reorder-pinned-workspace':
        return this.#hierarchy.reorderPinnedWorkspace(command, {
          workspaceId: text(input.workspaceId, 'workspaceId'),
          ...(optionalText(input.beforeWorkspaceId, 'beforeWorkspaceId') === undefined ? {} : {
            beforeWorkspaceId: optionalText(input.beforeWorkspaceId, 'beforeWorkspaceId')!
          }),
          now: integer(input.now, 'now', 0)
        })
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
      case 'hierarchy.delete-task': {
        const taskId = text(input.taskId, 'taskId')
        const result = this.#hierarchy.deleteTask(command, {
          windowId: text(input.windowId, 'windowId'),
          taskId,
          confirmedIntent: text(input.confirmedIntent, 'confirmedIntent'),
          now: integer(input.now, 'now', 0)
        })
        this.#notifications.clearTask(taskId)
        return result
      }
      case 'hierarchy.activate-task': {
        const result = this.#hierarchy.activateTask({
          windowId: text(input.windowId, 'windowId'),
          taskId: text(input.taskId, 'taskId'),
          now: integer(input.now, 'now', 0)
        })
        if (result.mount) this.#notifications.markPanelRead(result.mount.id)
        return result
      }
      case 'hierarchy.set-task-pinned':
        return this.#hierarchy.setTaskPinned(command, {
          taskId: text(input.taskId, 'taskId'),
          pinned: flag(input.pinned, 'pinned'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.reorder-pinned-task':
        return this.#hierarchy.reorderPinnedTask(command, {
          workspaceId: text(input.workspaceId, 'workspaceId'),
          taskId: text(input.taskId, 'taskId'),
          ...(optionalText(input.beforeTaskId, 'beforeTaskId') === undefined ? {} : {
            beforeTaskId: optionalText(input.beforeTaskId, 'beforeTaskId')!
          }),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.create-scene':
        return this.#hierarchy.createScene(command, {
          windowId: text(input.windowId, 'windowId'),
          taskId: text(input.taskId, 'taskId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.create-canvas':
        return this.#withActivePathState(this.#sessionCanvas.createCanvas(command, {
          windowId: text(input.windowId, 'windowId'),
          taskId: text(input.taskId, 'taskId'),
          now: integer(input.now, 'now', 0)
        }))
      case 'hierarchy.rename-scene':
        return this.#hierarchy.renameScene(command, {
          sceneId: text(input.sceneId, 'sceneId'),
          name: text(input.name, 'name'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.reorder-scene':
        return this.#hierarchy.reorderScene(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          ...(optionalText(input.beforeSceneId, 'beforeSceneId') === undefined
            ? {}
            : { beforeSceneId: optionalText(input.beforeSceneId, 'beforeSceneId')! }),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.close-scene':
        return this.#hierarchy.closeScene(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          ...(optionalText(input.confirmedIntent, 'confirmedIntent') === undefined
            ? {}
            : { confirmedIntent: optionalText(input.confirmedIntent, 'confirmedIntent')! }),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.activate-scene':
        return this.#hierarchy.activateScene({
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.replace-layout':
        return this.#sceneLayouts.replaceLayout(command, {
          sceneId: text(input.sceneId, 'sceneId'),
          expectedRevision: integer(input.expectedRevision, 'expectedRevision', 1),
          root: input.root as LayoutNode,
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.split-session':
        return enumeration(input.direction, ['horizontal', 'vertical'] as const, 'direction') === 'horizontal'
          ? this.#withActivePathState(this.#sessionCanvas.createShellSibling(command, {
              windowId: text(input.windowId, 'windowId'),
              sceneId: text(input.sceneId, 'sceneId'),
              sourceSessionId: text(input.sourceSessionId, 'sourceSessionId'),
              now: integer(input.now, 'now', 0)
            }))
          : this.#hierarchy.splitSession(command, {
              windowId: text(input.windowId, 'windowId'),
              sceneId: text(input.sceneId, 'sceneId'),
              sourceSessionId: text(input.sourceSessionId, 'sourceSessionId'),
              direction: 'vertical',
              now: integer(input.now, 'now', 0)
            })
      case 'hierarchy.create-shell-sibling':
        return this.#withActivePathState(this.#sessionCanvas.createShellSibling(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sourceSessionId: text(input.sourceSessionId, 'sourceSessionId'),
          ...(optionalText(input.parentSessionId, 'parentSessionId') === undefined
            ? {}
            : { parentSessionId: optionalText(input.parentSessionId, 'parentSessionId')! }),
          now: integer(input.now, 'now', 0)
        }))
      case 'hierarchy.record-session-interaction':
        return this.#sessionInteractions.record(command, {
          sessionId: text(input.sessionId, 'sessionId'),
          interactionKind: enumeration(
            input.interactionKind,
            ['submit', 'control', 'provider-action'] as const,
            'interactionKind'
          ),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.retry-provider-restore':
        return this.#providerModes.retryRestore(command, {
          sessionId: text(input.sessionId, 'sessionId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.restart-stopped-session':
        return this.#sessionCanvas.restartStoppedSession(command, {
          windowId: text(input.windowId, 'windowId'),
          sessionId: text(input.sessionId, 'sessionId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.remove-session-branch':
        return this.#sessionCanvas.removeSessionBranch(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sessionId: text(input.sessionId, 'sessionId'),
          includeDescendants: flag(input.includeDescendants, 'includeDescendants'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.reopen-scene':
        return this.#hierarchy.reopenScene(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.create-fork-child':
        return this.#forkWorkflows.createForkChild(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sourceSessionId: text(input.sourceSessionId, 'sourceSessionId'),
          name: text(input.name, 'name'),
          worktreeMode: enumeration(input.worktreeMode, ['current', 'new'] as const, 'worktreeMode'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.create-fork-sibling':
        return this.#forkWorkflows.createForkSibling(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sourceSessionId: text(input.sourceSessionId, 'sourceSessionId'),
          name: text(input.name, 'name'),
          worktreeMode: enumeration(input.worktreeMode, ['current', 'new'] as const, 'worktreeMode'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.retry-fork':
        return this.#forkWorkflows.retryFork(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sessionId: text(input.sessionId, 'sessionId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.remove-failed-fork':
        return this.#forkWorkflows.removeFailedFork(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sessionId: text(input.sessionId, 'sessionId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.fork-session':
        return this.#hierarchy.forkSession(command, {
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sourceSessionId: text(input.sourceSessionId, 'sourceSessionId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.activate-session':
        return this.#hierarchy.activateSession({
          windowId: text(input.windowId, 'windowId'),
          sessionId: text(input.sessionId, 'sessionId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.set-focused-session':
        return this.#sessionCanvas.setFocusedSession({
          windowId: text(input.windowId, 'windowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          sessionId: text(input.sessionId, 'sessionId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.delete-session':
        return this.#hierarchy.deleteSession(command, {
          windowId: text(input.windowId, 'windowId'),
          sessionId: text(input.sessionId, 'sessionId'),
          ...(optionalText(input.confirmedIntent, 'confirmedIntent') === undefined
            ? {}
            : { confirmedIntent: optionalText(input.confirmedIntent, 'confirmedIntent')! }),
          ...(input.preserveSceneOnLastSession === true
            ? { preserveSceneOnLastSession: true }
            : {}),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.detach-session':
        return this.#detachedSessions.detach(command, {
          mainWindowId: text(input.windowId, 'windowId'),
          sceneWindowId: text(input.sceneWindowId, 'sceneWindowId'),
          sceneId: text(input.sceneId, 'sceneId'),
          mountId: text(input.mountId, 'mountId'),
          sessionId: text(input.sessionId, 'sessionId'),
          nativeWindowKey: text(input.nativeWindowKey, 'nativeWindowKey'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.return-session':
        return this.#detachedSessions.returnSession(command, {
          sceneWindowId: text(input.sceneWindowId, 'sceneWindowId'),
          mainWindowId: text(input.windowId, 'windowId'),
          now: integer(input.now, 'now', 0)
        })
      case 'hierarchy.move-task-to-window': {
        const phase = enumeration(
          input.phase, ['prepare', 'acknowledge', 'fail'] as const, 'phase'
        )
        if (phase === 'prepare') {
          return this.#taskWindowMigrations.prepare(command, {
            migrationId: text(input.migrationId, 'migrationId'),
            taskId: text(input.taskId, 'taskId'),
            sourceWindowId: text(input.sourceWindowId, 'sourceWindowId'),
            targetWindowId: text(input.targetWindowId, 'targetWindowId'),
            now: integer(input.now, 'now', 0)
          })
        }
        if (phase === 'acknowledge') {
          return this.#taskWindowMigrations.acknowledgeTarget(command, {
            migrationId: text(input.migrationId, 'migrationId'),
            now: integer(input.now, 'now', 0)
          })
        }
        return this.#taskWindowMigrations.fail(command, {
          migrationId: text(input.migrationId, 'migrationId'),
          reason: text(input.reason, 'reason'),
          now: integer(input.now, 'now', 0)
        })
      }
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
      case 'session.set-permission-mode':
        return this.#sessions.updateProviderPermissionMode(command, {
          sessionId: text(input.sessionId, 'sessionId'),
          provider: enumeration(
            input.provider,
            ['claude-code', 'codex'] as const,
            'provider'
          ),
          permissionMode: enumeration(
            input.permissionMode,
            ['default', 'acceptEdits', 'plan', 'bypassPermissions'] as const,
            'permissionMode'
          ),
          now: integer(input.now, 'now', 0)
        })
      case 'session.set-model':
        return {
          sessionId: text(input.sessionId, 'sessionId'),
          modelStrategy: enumeration(
            input.modelStrategy,
            ['opusplan', 'claude-opus-4-6', 'claude-sonnet-4-6'] as const,
            'modelStrategy'
          )
        }
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

  #sessionCwd(sessionId: string): string {
    const row = this.#database.get<{ cwd: string }>(
      `SELECT COALESCE(context.cwd, sessions.cwd) AS cwd
       FROM sessions
       LEFT JOIN execution_contexts AS context ON context.id = sessions.execution_context_id
       WHERE sessions.id = ? AND sessions.archived_at IS NULL`, sessionId
    )
    if (!row) throw new RpcFault('NOT_FOUND', 'Session does not exist')
    return row.cwd
  }

  #providerConversationUsage(providerSessionId: string, targetSessionId: string): {
    availability: 'available' | 'loaded-here' | 'loaded-elsewhere'
    loadedSessionId?: string
    loadedSessionTitle?: string
  } {
    const { workspaceId } = this.#sessionOwner(targetSessionId)
    const bindings = this.#database.all<{
      session_id: string
      title: string
      kind: SessionKind
      archived_at: number | null
      task_archived_at: number | null
      resume_state: string
      invalidated_at: number | null
    }>(
      `SELECT binding.session_id, owner.title, owner.kind, owner.archived_at,
              tasks.archived_at AS task_archived_at,
              binding.resume_state, binding.invalidated_at
       FROM provider_bindings AS binding
       JOIN sessions AS owner ON owner.id = binding.session_id
       JOIN tasks ON tasks.id = owner.task_id
       WHERE binding.provider = 'claude-code' AND binding.provider_session_id = ?
         AND tasks.workspace_id = ?`,
      providerSessionId, workspaceId
    ).filter((binding) => binding.archived_at === null && binding.task_archived_at === null &&
      binding.kind === 'claude-code' && binding.invalidated_at === null &&
      ['unknown', 'available', 'resuming', 'resumed'].includes(binding.resume_state))
    const binding = bindings.find(({ session_id }) => session_id !== targetSessionId) ?? bindings[0]
    if (!binding) return { availability: 'available' }
    return {
      availability: binding.session_id === targetSessionId ? 'loaded-here' : 'loaded-elsewhere',
      loadedSessionId: binding.session_id,
      loadedSessionTitle: binding.title
    }
  }

  #snapshot(payload: unknown): unknown {
    const workspaces = this.#database.all<{ id: string }>('SELECT id FROM workspaces ORDER BY created_at').map(({ id }) => this.#workspaces.getWorkspace(id)!)
    const tasks = this.#database.all<{ id: string }>('SELECT id FROM tasks ORDER BY created_at').map(({ id }) => this.#workspaces.getTask(id)!)
    const sessions = this.#database.all<{ id: string }>('SELECT id FROM sessions ORDER BY created_at').map(({ id }) => this.#sessions.getSession(id)!)
    const sessionRuns = sessions.flatMap(({ id }) => this.#sessions.listRuns(id))
    const providerBindings = sessions.flatMap(({ id }) => this.#sessions.listProviderBindings(id))
    const relations = this.#database.all<{ relation_id: string }>('SELECT relation_id FROM session_relations_current ORDER BY created_at').map(({ relation_id }) => this.#relations.getCurrent(relation_id)!)
    const sceneSnapshots = this.#database.all<{ id: string }>(
      'SELECT id FROM scenes ORDER BY task_id, sort_key, created_at, id'
    ).map(({ id }) => ({
      ...this.#scenes.snapshot(id)!,
      geometry: this.#geometry.list(id)
    }))
    const requestedWindowId = typeof payload === 'object' && payload !== null &&
      'windowId' in payload && typeof payload.windowId === 'string'
      ? payload.windowId
      : undefined
    const sessionGraphs = Object.fromEntries(sceneSnapshots.map(({ scene }) => [
      scene.id,
      this.#sessionGraphs.projectSceneGraph(scene.id, requestedWindowId)
    ]))
    const eventSequence = this.#database.get<{ maximum: number }>('SELECT COALESCE(MAX(seq), 0) AS maximum FROM domain_events')?.maximum ?? 0
    const windowId = requestedWindowId ?? this.#database.get<{ id: string }>(
      `SELECT id FROM app_windows WHERE kind = 'main' AND state <> 'closed'
       ORDER BY created_at LIMIT 1`
    )?.id ?? 'window-1'
    const activeWorkspaces = workspaces.filter(({ archivedAt }) => archivedAt === undefined)
    const activeTasks = tasks.filter(({ archivedAt }) => archivedAt === undefined)
    const activeSessions = sessions.filter(({ archivedAt }) => archivedAt === undefined)
    const activeSceneSnapshots = sceneSnapshots.filter(({ scene }) => scene.archivedAt === undefined)
    const pathStates = this.#database.all<{
      workspace_id: string; status: 'valid' | 'invalid'; reason: '' | 'missing' | 'not-directory' | 'no-access' | 'unknown'
      checked_at: number; validation_generation: number
    }>('SELECT * FROM workspace_path_state ORDER BY workspace_id').map((row) => ({
      workspaceId: row.workspace_id, status: row.status, reason: row.reason,
      checkedAt: row.checked_at, validationGeneration: row.validation_generation
    }))
    const unreadByTask = Object.fromEntries(activeTasks.flatMap(({ id }) => {
      const count = this.#notifications.unreadCount({ taskId: id })
      return count > 0 ? [[id, count] as const] : []
    }))
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
      sceneSnapshots,
      sessionGraphs,
      hierarchy: {
        windowId,
        workspaces: activeWorkspaces,
        tasks: activeTasks,
        sessions: activeSessions,
        scenes: activeSceneSnapshots.map(({ scene }) => scene),
        sceneSnapshots: activeSceneSnapshots,
        sessionGraphs: Object.fromEntries(
          activeSceneSnapshots.map(({ scene }) => [scene.id, sessionGraphs[scene.id]])
        ),
        pathStates,
        unreadByTask,
        navigation: this.#navigation.get(windowId),
        taskPlacements: this.#navigation.listTaskPlacements()
      }
    }
  }

  async #withActivePathState<T extends { workspace: { id: string } | null }>(
    result: T
  ): Promise<T & { pathState?: Awaited<ReturnType<WorkspacePathService['validateWorkspace']>> }> {
    if (result.workspace === null) return result
    const pathState = await this.#workspacePaths.validateWorkspace(result.workspace.id)
    return { ...result, pathState }
  }

  #sessionOwner(sessionId: string): { workspaceId: string } {
    const owner = this.#database.get<{ workspace_id: string }>(
      `SELECT tasks.workspace_id FROM sessions
       JOIN tasks ON tasks.id = sessions.task_id
       WHERE sessions.id = ? AND sessions.archived_at IS NULL
         AND tasks.archived_at IS NULL`,
      sessionId
    )
    if (!owner) throw new RpcFault('NOT_FOUND', `Session ${sessionId} does not exist`)
    return { workspaceId: owner.workspace_id }
  }
}

function derivedCommand(
  command: DomainCommandMetadata,
  suffix: string
): DomainCommandMetadata {
  return {
    ...command,
    commandId: `${command.commandId}:${suffix}`,
    commandType: `${command.commandType}:${suffix}`,
    requestHash: `${command.requestHash}:${suffix}`,
    causationId: command.commandId
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
function optionalString(value: unknown): string {
  if (value === undefined) return ''
  if (typeof value !== 'string') throw new RpcFault('INVALID_REQUEST', 'query must be a string')
  return value
}
function optionalInteger(value: unknown, fallback: number): number {
  return value === undefined ? fallback : integer(value, 'limit', 1)
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
function flag(value: unknown, label: string): boolean {
  if (typeof value !== 'boolean') throw new RpcFault('INVALID_REQUEST', `${label} must be a boolean`)
  return value
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
