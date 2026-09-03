import { createHash } from 'node:crypto'
import { access, constants, stat } from 'node:fs/promises'
import { basename, resolve } from 'node:path'

import type { DomainCommandMetadata } from '@matou/domain'
import { ZodError } from 'zod'

import type {
  CoordinateAcceptedForkInput,
  CreateForkBatchInput,
  ForkBatchCoordinator,
  ForkFocusLease,
  RetryForkBatchInput
} from './fork-batch-coordinator'
import {
  HostActionConfirmationError,
  HostActionConfirmationService,
  type ConfirmationRecord
} from './host-action-confirmation-service'
import {
  HostActionTargetResolver,
  HostActionTargetResolverError,
  type RemovalImpact,
  type ResolvedForkEnvironment,
  type ResolvedHostEntity
} from './host-action-target-resolver'
import {
  parseHostActionRequest,
  type ForkItemInput,
  type ForkEnvironmentChoice,
  type HostActionErrorCode,
  type HostActionMethod,
  type HostActionRequest,
  type HostActionResult,
  type HostEntitySelector,
  type HostImpactSummary,
  type HostResultPath
} from './host-action-types'
import {
  markHostControlCommittedResult,
  withHostControlPostResponseEffect
} from './host-control-post-response'
import type { HostCallerIdentity } from './host-control-types'
import {
  HierarchyApplicationService,
  readHierarchyResult,
  type CreateHierarchyResult,
  type CreatedHierarchyPath,
  type WorkspaceHierarchyResult
} from '../hierarchy/hierarchy-application-service'
import {
  SessionCanvasService,
  type RemoveSessionBranchResult
} from '../session-canvas/session-canvas-service'
import {
  ForkWorkflowError,
  type ForkWorkflowResult,
  type ForkWorkflowService
} from '../session-canvas/fork-workflow-service'
import {
  RuntimeDatabase,
  StorageReadOnlyError
} from '../storage/database'
import { WorkspacePathInvalidError } from '../hierarchy/workspace-path-service'

type HierarchyActions = Pick<HierarchyApplicationService,
  'createWorkspace' | 'createTask' | 'removeWorkspace' | 'deleteTask' | 'closeScene'>

type SessionCanvasActions = Pick<SessionCanvasService,
  'createCanvas' | 'createSessionSibling' | 'removeSessionBranch' |
  'restoreFocusedSessionIfCurrent'>

type ForkWorkflowActions = Pick<ForkWorkflowService, 'createForkChild' | 'createForkSibling'>
type ForkBatchActions = Pick<
  ForkBatchCoordinator,
  'createChildren' | 'retryFailures' | 'preflightAccepted' | 'coordinateAcceptedFork'
>

interface ActiveSingleFork {
  requestHash: string
  promise: Promise<HostActionResult>
}

interface FocusSnapshot {
  windowId: string
  sceneId: string
  sessionId: string
}

const DATABASE_FOCUS_WINDOW_LOCKS = new WeakMap<
  RuntimeDatabase,
  Map<string, Promise<void>>
>()

export interface RuntimeHostActionFacadeDependencies {
  database: RuntimeDatabase
  resolver: HostActionTargetResolver
  confirmations: HostActionConfirmationService
  hierarchy: HierarchyActions
  sessionCanvas: SessionCanvasActions
  forkWorkflow: ForkWorkflowActions
  forkBatches: ForkBatchActions
  disposeSessions(sessionIds: string[]): void | Promise<void>
  now?: () => number
}

export class RuntimeHostActionError extends Error {
  readonly code: HostActionErrorCode
  readonly candidates: readonly unknown[]

  constructor(
    code: HostActionErrorCode,
    message: string,
    options: { candidates?: readonly unknown[]; cause?: unknown } = {}
  ) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause })
    this.name = 'RuntimeHostActionError'
    this.code = code
    this.candidates = options.candidates ?? []
  }
}

/** Runtime's single product-level mutation boundary for Host Control actions. */
export class RuntimeHostActionFacade {
  readonly #database: RuntimeDatabase
  readonly #resolver: HostActionTargetResolver
  readonly #confirmations: HostActionConfirmationService
  readonly #hierarchy: HierarchyActions
  readonly #sessionCanvas: SessionCanvasActions
  readonly #forkWorkflow: ForkWorkflowActions
  readonly #forkBatches: ForkBatchActions
  readonly #disposeManagedSessions: RuntimeHostActionFacadeDependencies['disposeSessions']
  readonly #now: () => number
  readonly #singleForks = new Map<string, ActiveSingleFork>()

  constructor(dependencies: RuntimeHostActionFacadeDependencies) {
    this.#database = dependencies.database
    this.#resolver = dependencies.resolver
    this.#confirmations = dependencies.confirmations
    this.#hierarchy = dependencies.hierarchy
    this.#sessionCanvas = dependencies.sessionCanvas
    this.#forkWorkflow = dependencies.forkWorkflow
    this.#forkBatches = dependencies.forkBatches
    this.#disposeManagedSessions = dependencies.disposeSessions
    this.#now = dependencies.now ?? Date.now
  }

  async execute(
    method: HostActionMethod,
    caller: HostCallerIdentity,
    rawParams: unknown
  ): Promise<HostActionResult> {
    try {
      const request = parseHostActionRequest(method, rawParams)
      if (isStructuralMutation(request.method)) this.#assertWritable()
      switch (request.method) {
        case 'structure.create.workspace':
          return markHostControlCommittedResult(await this.#createWorkspace(caller, request))
        case 'structure.create.task':
          return markHostControlCommittedResult(this.#createTask(caller, request))
        case 'structure.create.canvas':
          return markHostControlCommittedResult(this.#createCanvas(caller, request))
        case 'structure.create.session':
          return markHostControlCommittedResult(this.#createSession(caller, request))
        case 'structure.fork.child':
        case 'structure.fork.sibling':
          return markHostControlCommittedResult(await this.#forkOne(caller, request))
        case 'structure.fork.children':
          return markHostControlCommittedResult(await this.#forkChildren(caller, request))
        case 'structure.remove.preview':
          return this.#previewRemoval(caller, request)
        case 'structure.remove.commit':
          return markHostControlCommittedResult(
            await this.#commitRemoval(caller, request.confirmationRef)
          )
        case 'structure.canvas-close.preview':
          return this.#previewCanvasClose(caller, request)
        case 'structure.canvas-close.commit':
          return markHostControlCommittedResult(
            await this.#commitCanvasClose(caller, request.confirmationRef)
          )
        case 'navigation.focus.session':
        case 'navigation.switch.workspace':
        case 'navigation.switch.task':
        case 'navigation.switch.canvas':
          throw new RuntimeHostActionError(
            'TARGET_NOT_READY',
            '目标已解析，等待窗口导航通道接管'
          )
      }
    } catch (error) {
      throw normalizeFacadeError(error)
    }
  }

  async #createWorkspace(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, { method: 'structure.create.workspace' }>
  ): Promise<HostActionResult> {
    const command = actionCommand(caller, request.submissionKey, request)
    const replay = this.#commandReplay<CreateHierarchyResult>(command)
    if (replay) return this.#createdResult('workspace', replay)

    const rootDirectory = resolve(request.path)
    await validateWorkspaceDirectory(rootDirectory)
    const result = this.#hierarchy.createWorkspace(command, {
      windowId: this.#callerSession(caller).windowId,
      name: request.title ?? (basename(rootDirectory) || rootDirectory),
      rootDirectory,
      navigation: request.enter === true ? 'activate' : 'preserve',
      now: this.#now()
    })
    return this.#createdResult('workspace', result)
  }

  #createTask(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, { method: 'structure.create.task' }>
  ): HostActionResult {
    const command = actionCommand(caller, request.submissionKey, request)
    const replay = this.#commandReplay<CreateHierarchyResult>(command)
    if (replay) return this.#createdResult('task', replay)
    const target = requireEntity(this.#resolve(caller, request.workspace), 'workspace')
    const result = this.#hierarchy.createTask(command, {
      windowId: target.windowId,
      workspaceId: target.workspaceId,
      ...(request.title === undefined ? {} : { title: request.title }),
      navigation: request.enter === true ? 'activate' : 'preserve',
      now: this.#now()
    })
    return this.#createdResult('task', result)
  }

  #createCanvas(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, { method: 'structure.create.canvas' }>
  ): HostActionResult {
    const command = actionCommand(caller, request.submissionKey, request)
    const replay = this.#commandReplay<CreateHierarchyResult>(command)
    if (replay) return this.#createdResult('canvas', replay)
    const target = requireEntity(this.#resolve(caller, request.task), 'task')
    const result = this.#sessionCanvas.createCanvas(command, {
      windowId: target.windowId,
      taskId: target.taskId,
      ...(request.title === undefined ? {} : { title: request.title }),
      navigation: request.enter === true ? 'activate' : 'preserve',
      now: this.#now()
    })
    return this.#createdResult('canvas', result)
  }

  #createSession(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, { method: 'structure.create.session' }>
  ): HostActionResult {
    const command = actionCommand(caller, request.submissionKey, request)
    const replay = this.#commandReplay<CreateHierarchyResult>(command)
    if (replay) return this.#createdResult('session', replay)
    const target = requireEntity(this.#resolve(caller, request.canvas), 'canvas')
    const result = this.#sessionCanvas.createSessionSibling(command, {
      windowId: target.windowId,
      sceneId: target.sceneId,
      sourceSessionId: this.#canvasAnchor(target.windowId, target.sceneId),
      profile: request.profile,
      ...(request.title === undefined ? {} : { title: request.title }),
      navigation: request.enter === true ? 'activate' : 'preserve',
      now: this.#now()
    })
    return this.#createdResult('session', result)
  }

  async #forkOne(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, {
      method: 'structure.fork.child' | 'structure.fork.sibling'
    }>
  ): Promise<HostActionResult> {
    const command = actionCommand(caller, request.submissionKey, request)
    const stored = this.#commandReplay<ForkWorkflowResult>(command)
    if (stored) return this.#replaySingleFork(command, caller, request, stored)

    const active = this.#singleForks.get(command.commandId)
    if (active) {
      if (active.requestHash !== command.requestHash) {
        throw new RuntimeHostActionError(
          'PATH_CONFLICT',
          'submission key 已被不同输入使用'
        )
      }
      return active.promise
    }

    const operation = this.#performSingleFork(command, caller, request)
    this.#singleForks.set(command.commandId, {
      requestHash: command.requestHash,
      promise: operation
    })
    try {
      return await operation
    } finally {
      if (this.#singleForks.get(command.commandId)?.promise === operation) {
        this.#singleForks.delete(command.commandId)
      }
    }
  }

  async #performSingleFork(
    command: DomainCommandMetadata,
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, {
      method: 'structure.fork.child' | 'structure.fork.sibling'
    }>
  ): Promise<HostActionResult> {
    const source = requireEntity(this.#resolve(caller, request.source), 'session')
    this.#assertWorkspacePathAvailable(source.workspaceId)
    const environment = this.#resolver.resolveForkEnvironment(source, request.environment)
    const focusLease = await this.#acquireForkFocusLease(
      caller, source, request.method === 'structure.fork.sibling'
    )
    let result!: ForkWorkflowResult
    try {
      result = await (request.method === 'structure.fork.child'
        ? this.#forkWorkflow.createForkChild(command, forkInput(source, request.title, environment, request.submissionKey, this.#now()))
        : this.#forkWorkflow.createForkSibling(command, forkInput(source, request.title, environment, request.submissionKey, this.#now())))
    } finally {
      focusLease.finish(result?.session?.id, result?.session?.updatedAt)
    }
    const forked = this.#forkedResult(result, environment)
    if (request.start !== true) return forked
    return this.#coordinateSingleForkStart(
      caller,
      request,
      source,
      environment,
      result,
      forked
    )
  }

  async #replaySingleFork(
    command: DomainCommandMetadata,
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, {
      method: 'structure.fork.child' | 'structure.fork.sibling'
    }>,
    stored: ForkWorkflowResult
  ): Promise<HostActionResult> {
    if (!stored.session || !stored.scene) {
      throw new RuntimeHostActionError(
        'TARGET_NOT_FOUND',
        '已接受的 Fork 结果缺少稳定会话路径'
      )
    }
    const publicItem = singleForkPublicItem(request)
    const accepted = this.#forkBatches.preflightAccepted({
      caller,
      source: request.source,
      batchKey: singleForkBatchKey(request.submissionKey),
      items: [publicItem]
    })
    const source = accepted?.source ?? this.#acceptedForkSource(stored)
    const environment = accepted?.items[0]?.environment ??
      this.#acceptedBatchEnvironment(source, request.environment)
    const focusLease = await this.#acquireForkFocusLease(
      caller, source, request.method === 'structure.fork.sibling'
    )
    // The workflow checks its durable submission intent before inspecting this
    // compatibility input. Re-entering it refreshes current Fork progress while
    // avoiding a fresh branch/Worktree reservation check for the accepted key.
    const replayInput = {
      windowId: stored.navigation.windowId,
      sceneId: stored.scene.id,
      sourceSessionId: stored.session.id,
      name: request.title,
      environment: {
        mode: 'current' as const,
        executionContextId: stored.session.executionContextId
      },
      submissionKey: request.submissionKey,
      now: this.#now()
    }
    let result!: ForkWorkflowResult
    try {
      result = await (request.method === 'structure.fork.child'
        ? this.#forkWorkflow.createForkChild(command, replayInput)
        : this.#forkWorkflow.createForkSibling(command, replayInput))
    } finally {
      focusLease.finish(result?.session?.id, result?.session?.updatedAt)
    }
    const forked = this.#forkedResult(result, request.environment)
    if (request.start !== true) return forked
    return this.#coordinateSingleForkStart(
      caller,
      request,
      source,
      environment,
      result,
      forked
    )
  }

  async #coordinateSingleForkStart(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, {
      method: 'structure.fork.child' | 'structure.fork.sibling'
    }>,
    source: ResolvedHostEntity & { kind: 'session' },
    environment: ResolvedForkEnvironment,
    result: ForkWorkflowResult,
    forked: Extract<HostActionResult, { kind: 'forked' }>
  ): Promise<HostActionResult> {
    const input: CoordinateAcceptedForkInput = {
      caller,
      source,
      batchKey: singleForkBatchKey(request.submissionKey),
      items: [{
        itemKey: request.method,
        title: request.title,
        environment,
        ...(request.prompt === undefined ? {} : { prompt: request.prompt }),
        start: true
      }],
      publicRequest: {
        source: request.source,
        items: [singleForkPublicItem(request)]
      },
      sessionId: result.session!.id,
      state: result.forkState === 'succeeded' ? 'ready' : 'created'
    }
    const coordinated = await this.#forkBatches.coordinateAcceptedFork(input)
    return {
      ...forked,
      state: coordinated.items[0]?.state === 'started'
        ? 'started'
        : coordinated.items[0]?.state === 'ready'
          ? 'ready'
          : 'created'
    }
  }

  #acceptedForkSource(
    stored: ForkWorkflowResult
  ): ResolvedHostEntity & { kind: 'session' } {
    const sourceSessionId = this.#database.get<{ source_session_id: string }>(
      `SELECT to_session_id AS source_session_id
       FROM session_relations_current
       WHERE from_session_id = ? AND relation_kind IN ('derived-from', 'forked-from')`,
      stored.session!.id
    )?.source_session_id
    if (sourceSessionId !== undefined) {
      return requireEntity(this.#resolve({
        runId: sourceSessionId,
        sessionId: sourceSessionId
      }, { kind: 'session', sessionId: sourceSessionId }), 'session')
    }
    return {
      kind: 'session',
      windowId: stored.navigation.windowId,
      workspaceId: stored.workspace!.id,
      taskId: stored.task!.id,
      sceneId: stored.scene!.id,
      sessionId: stored.session!.id
    }
  }

  #forkedResult(
    result: ForkWorkflowResult,
    environment: ResolvedForkEnvironment | ForkEnvironmentChoice
  ): Extract<HostActionResult, { kind: 'forked' }> {
    if (result.forkState === 'failed' || !result.session) {
      throw new RuntimeHostActionError(
        'TARGET_NOT_READY', result.error ?? 'Fork 节点尚未准备完成'
      )
    }
    return {
      kind: 'forked',
      state: result.forkState === 'succeeded' ? 'ready' : 'created',
      sessionRef: `session:${result.session.id}`,
      path: this.#hostPath(result),
      environment: publicEnvironment(environment)
    }
  }

  async #forkChildren(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, { method: 'structure.fork.children' }>
  ): Promise<HostActionResult> {
    const accepted = this.#forkBatches.preflightAccepted({
      caller,
      source: request.source,
      batchKey: request.batchKey,
      items: request.items
    })
    // Complete every environment preflight before the durable coordinator can
    // accept its first item, so an invalid later choice never causes a partial batch.
    const source = accepted?.source ??
      requireEntity(this.#resolve(caller, request.source), 'session')
    if (accepted === undefined) this.#assertWorkspacePathAvailable(source.workspaceId)
    const items = accepted?.items ?? request.items.map((item) => ({
      ...item,
      environment: this.#resolver.resolveForkEnvironment(source, item.environment)
    }))
    const input: CreateForkBatchInput = {
      caller,
      source,
      batchKey: request.batchKey,
      items,
      publicRequest: { source: request.source, items: request.items },
      acquireFocusLease: () => this.#acquireForkFocusLease(caller, source)
    }
    if (request.retryItemKeys !== undefined) {
      const retry: RetryForkBatchInput = {
        ...input,
        retryItemKeys: request.retryItemKeys
      }
      return this.#forkBatches.retryFailures(retry)
    }
    return this.#forkBatches.createChildren(input)
  }

  #previewRemoval(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, { method: 'structure.remove.preview' }>
  ): HostActionResult {
    const target = this.#resolve(caller, request.target)
    this.#assertRemovalTarget(target)
    const impact = this.#removalImpact(target, request.scope)
    const targetRef = stableRef(target)
    return {
      kind: 'removal-preview',
      impact,
      confirmationRef: this.#confirmations.issue({
        caller,
        action: 'remove',
        targetRef,
        scope: impact.scope,
        projectionRevision: this.#resolver.projectionRevision(caller),
        impact,
        now: this.#now()
      })
    }
  }

  #previewCanvasClose(
    caller: HostCallerIdentity,
    request: Extract<HostActionRequest, { method: 'structure.canvas-close.preview' }>
  ): HostActionResult {
    const target = requireEntity(this.#resolve(caller, request.target), 'canvas')
    const impact = this.#canvasCloseImpact(target)
    const targetRef = stableRef(target)
    return {
      kind: 'canvas-close-preview',
      impact,
      confirmationRef: this.#confirmations.issue({
        caller,
        action: 'canvas-close',
        targetRef,
        scope: 'subtree',
        projectionRevision: this.#resolver.projectionRevision(caller),
        impact,
        now: this.#now()
      })
    }
  }

  async #commitRemoval(caller: HostCallerIdentity, confirmationRef: string): Promise<HostActionResult> {
    const confirmed = this.#revalidateConfirmation(caller, confirmationRef, 'remove')
    const target = confirmed.target
    this.#assertRemovalTarget(target)
    const now = this.#now()
    const command = destructiveCommand(caller, confirmationRef, confirmed.record, now)
    let active: WorkspaceHierarchyResult
    let disposedSessionIds: string[]
    let removedSessions = confirmed.record.impact.sessions

    if (target.kind === 'workspace') {
      const result = this.#hierarchy.removeWorkspace(command, {
        windowId: target.windowId,
        workspaceId: target.workspaceId,
        confirmedIntent: `remove-workspace:${target.workspaceId}`,
        now
      })
      active = result
      disposedSessionIds = result.disposedSessionIds
    } else if (target.kind === 'task') {
      const result = this.#hierarchy.deleteTask(command, {
        windowId: target.windowId,
        taskId: target.taskId,
        confirmedIntent: `delete-task:${target.taskId}`,
        now
      })
      active = result
      disposedSessionIds = result.disposedSessionIds
    } else {
      const result: RemoveSessionBranchResult = this.#sessionCanvas.removeSessionBranch(command, {
        windowId: target.windowId,
        sceneId: target.sceneId,
        sessionId: target.sessionId,
        scope: confirmed.record.scope === 'subtree' ? 'node-and-descendants' : 'node-only',
        now
      })
      active = readHierarchyResult(this.#database, target.windowId)
      disposedSessionIds = result.disposedSessionIds
      removedSessions = result.removedSessionIds.length
    }

    const result: HostActionResult = {
      kind: 'removed',
      targetRef: confirmed.record.targetRef,
      removedTasks: confirmed.record.impact.tasks,
      removedCanvases: confirmed.record.impact.canvases,
      removedSessions,
      activePath: this.#hostPath(active)
    }
    return this.#applySessionDisposals(result, disposedSessionIds)
  }

  async #commitCanvasClose(
    caller: HostCallerIdentity,
    confirmationRef: string
  ): Promise<HostActionResult> {
    const confirmed = this.#revalidateConfirmation(caller, confirmationRef, 'canvas-close')
    const target = requireEntity(confirmed.target, 'canvas')
    const now = this.#now()
    const result = this.#hierarchy.closeScene(
      destructiveCommand(caller, confirmationRef, confirmed.record, now),
      {
        windowId: target.windowId,
        sceneId: target.sceneId,
        confirmedIntent: `close-scene:${target.sceneId}`,
        now
      }
    )
    const response: HostActionResult = {
      kind: 'canvas-closed',
      targetRef: confirmed.record.targetRef,
      removedSessions: result.disposedSessionIds.length,
      activePath: this.#hostPath(result)
    }
    return this.#applySessionDisposals(response, result.disposedSessionIds)
  }

  #revalidateConfirmation(
    caller: HostCallerIdentity,
    ref: string,
    action: ConfirmationRecord['action']
  ): { record: ConfirmationRecord; target: ResolvedHostEntity } {
    const now = this.#now()
    const record = this.#confirmations.inspect({ ref, caller, now })
    if (record.action !== action) {
      throw new RuntimeHostActionError(
        'CONFIRMATION_STALE',
        '确认对应的操作已变化，请重新预览'
      )
    }
    let target: ResolvedHostEntity
    try {
      target = this.#resolveStable(caller, record.targetRef)
    } catch (error) {
      if (error instanceof HostActionTargetResolverError && error.code === 'TARGET_NOT_FOUND') {
        throw new RuntimeHostActionError(
          'CONFIRMATION_STALE',
          '确认对应的目标或影响已变化，请重新预览',
          { cause: error }
        )
      }
      throw error
    }
    const impact = action === 'canvas-close'
      ? this.#canvasCloseImpact(requireEntity(target, 'canvas'))
      : this.#hostImpact(target, record.scope)
    const projectionRevision = this.#resolver.projectionRevision(caller)
    const consumed = this.#confirmations.consume({
      ref,
      caller,
      action,
      targetRef: stableRef(target),
      scope: record.scope,
      projectionRevision,
      impact,
      now
    })
    return { record: consumed, target }
  }

  #resolve(caller: HostCallerIdentity, selector: HostEntitySelector): ResolvedHostEntity {
    const revision = 'projectionRevision' in selector ? selector.projectionRevision : ''
    return this.#resolver.resolveEntity(caller, selector, revision)
  }

  #resolveStable(caller: HostCallerIdentity, ref: string): ResolvedHostEntity {
    const projectionRevision = this.#resolver.projectionRevision(caller)
    return this.#resolver.resolveEntity(caller, {
      kind: 'ref', ref, projectionRevision
    }, projectionRevision)
  }

  #callerSession(caller: HostCallerIdentity): ResolvedHostEntity & { kind: 'session' } {
    return requireEntity(this.#resolver.resolveEntity(caller, { kind: 'self' }, ''), 'session')
  }

  #hostImpact(target: ResolvedHostEntity, scope: 'node' | 'subtree'): HostImpactSummary {
    return this.#resolver.toHostImpactSummary(this.#resolver.previewRemoval(target, scope))
  }

  #canvasCloseImpact(
    target: ResolvedHostEntity & { kind: 'canvas' }
  ): HostImpactSummary {
    const impact = this.#hostImpact(target, 'subtree')
    const counts = this.#database.get<{ scenes: number; tasks: number }>(
      `SELECT
         (SELECT COUNT(*) FROM scenes
          WHERE task_id = ? AND archived_at IS NULL) AS scenes,
         (SELECT COUNT(*) FROM tasks
          WHERE workspace_id = ? AND archived_at IS NULL) AS tasks`,
      target.taskId,
      target.workspaceId
    )
    if (counts?.scenes !== 1 || counts.tasks !== 1) return impact
    // Existing closeScene behavior hides the window for the last Canvas of the
    // last Task. Its preview must describe that no hierarchy or process ends.
    return {
      ...impact,
      canvases: 0,
      sessions: 0,
      descendants: 0,
      liveRuns: 0,
      terminalProcesses: 0
    }
  }

  #removalImpact(
    target: ResolvedHostEntity,
    requestedScope: 'node' | 'subtree'
  ): HostImpactSummary {
    const effectiveScope = effectiveRemovalScope(target, requestedScope)
    let impact: RemovalImpact = this.#resolver.previewRemoval(target, effectiveScope)
    if (
      target.kind === 'session' &&
      impact.scope === 'subtree' &&
      impact.descendants === 0
    ) {
      impact = this.#resolver.previewRemoval(target, 'node')
    }
    return this.#resolver.toHostImpactSummary(impact)
  }

  #createdResult(
    entity: 'workspace' | 'task' | 'canvas' | 'session',
    result: CreateHierarchyResult
  ): HostActionResult {
    return {
      kind: 'created',
      entity,
      createdRef: createdRef(entity, result.created),
      path: this.#createdPath(result.navigation.windowId, result.created),
      focusedPath: this.#hostPath(result)
    }
  }

  #createdPath(windowId: string, created: CreatedHierarchyPath): HostResultPath {
    return {
      window: this.#window(windowId),
      workspace: {
        ref: `workspace:${created.workspace.id}`,
        title: created.workspace.name,
        path: created.workspace.rootDirectory
      },
      task: { ref: `task:${created.task.id}`, title: created.task.title },
      canvas: { ref: `scene:${created.scene.id}`, title: created.scene.name },
      session: { ref: `session:${created.session.id}`, title: created.session.title }
    }
  }

  #hostPath(result: WorkspaceHierarchyResult): HostResultPath {
    if (!result.workspace) {
      throw new RuntimeHostActionError('TARGET_NOT_FOUND', '操作完成后没有可用工作空间')
    }
    return {
      window: this.#window(result.navigation.windowId),
      workspace: {
        ref: `workspace:${result.workspace.id}`,
        title: result.workspace.name,
        path: result.workspace.rootDirectory
      },
      ...(result.task
        ? { task: { ref: `task:${result.task.id}`, title: result.task.title } }
        : {}),
      ...(result.scene
        ? { canvas: { ref: `scene:${result.scene.id}`, title: result.scene.name } }
        : {}),
      ...(result.session
        ? { session: { ref: `session:${result.session.id}`, title: result.session.title } }
        : {})
    }
  }

  #window(windowId: string): HostResultPath['window'] {
    const kind = this.#database.get<{ kind: 'main' | 'detached-terminal' }>(
      'SELECT kind FROM app_windows WHERE id = ?', windowId
    )?.kind
    return {
      ref: `window:${windowId}`,
      title: kind === 'detached-terminal' ? '独立终端窗口' : '主窗口'
    }
  }

  #canvasAnchor(windowId: string, sceneId: string): string {
    const focused = this.#database.get<{ session_id: string }>(
      `SELECT focus.active_session_id AS session_id
       FROM window_scene_focus AS focus
       JOIN sessions ON sessions.id = focus.active_session_id AND sessions.archived_at IS NULL
       JOIN session_canvas_memberships AS membership
         ON membership.session_id = sessions.id AND membership.scene_id = focus.scene_id
       WHERE focus.window_id = ? AND focus.scene_id = ?`,
      windowId,
      sceneId
    )
    if (focused) return focused.session_id
    const fallback = this.#database.get<{ session_id: string }>(
      `SELECT membership.session_id
       FROM session_canvas_memberships AS membership
       JOIN sessions ON sessions.id = membership.session_id AND sessions.archived_at IS NULL
       WHERE membership.scene_id = ?
       ORDER BY membership.last_user_interaction_seq DESC,
                membership.sibling_created_seq, membership.session_id LIMIT 1`,
      sceneId
    )
    if (!fallback) {
      throw new RuntimeHostActionError('TARGET_NOT_READY', '目标画布没有可用会话锚点')
    }
    return fallback.session_id
  }

  async #acquireForkFocusLease(
    caller: HostCallerIdentity,
    source: ResolvedHostEntity & { kind: 'session' },
    sourceMayBeTemporary = false
  ): Promise<ForkFocusLease> {
    const callerWindowId = this.#callerSession(caller).windowId
    const releaseWindows = await acquireFocusWindowLocks(
      this.#database,
      [callerWindowId, source.windowId]
    )
    let finished = false
    let snapshots: FocusSnapshot[]
    try {
      snapshots = [...new Set([callerWindowId, source.windowId])].flatMap((windowId) => {
        const current = readHierarchyResult(this.#database, windowId)
        return current.session === null || current.scene === null ? [] : [{
          windowId,
          sceneId: current.scene.id,
          sessionId: current.session.id
        }]
      })
    } catch (error) {
      releaseWindows()
      throw error
    }
    return {
      finish: (temporarySessionId, focusUpdatedAt) => {
        if (finished) return
        finished = true
        try {
          if (temporarySessionId === undefined) return
          for (const snapshot of snapshots) {
            const expectedSessionIds = snapshot.windowId === source.windowId && sourceMayBeTemporary
              ? [temporarySessionId, source.sessionId]
              : [temporarySessionId]
            for (const expectedSessionId of new Set(expectedSessionIds)) {
              if (snapshot.sessionId === expectedSessionId) break
              let restored = false
              try {
                restored = this.#sessionCanvas.restoreFocusedSessionIfCurrent({
                  windowId: snapshot.windowId,
                  sceneId: snapshot.sceneId,
                  sessionId: snapshot.sessionId,
                  expectedSessionId,
                  ...(focusUpdatedAt === undefined
                    ? {}
                    : { expectedFocusUpdatedAt: focusUpdatedAt }),
                  now: this.#now()
                })
              } catch {
                // Focus restoration is a best-effort post-mutation CAS. The durable
                // Fork result stays authoritative when its snapshot disappears.
              }
              if (restored) break
            }
          }
        } finally {
          releaseWindows()
        }
      }
    }
  }

  #assertWorkspacePathAvailable(workspaceId: string): void {
    const invalid = this.#database.get<{ status: 'valid' | 'invalid' }>(
      'SELECT status FROM workspace_path_state WHERE workspace_id = ?',
      workspaceId
    )?.status === 'invalid'
    if (invalid) throw new WorkspacePathInvalidError(workspaceId)
  }

  #assertRemovalTarget(target: ResolvedHostEntity): asserts target is Exclude<
    ResolvedHostEntity,
    { kind: 'canvas' }
  > {
    if (target.kind === 'canvas') {
      throw new RuntimeHostActionError(
        'TARGET_NOT_READY',
        '画布使用关闭画布操作'
      )
    }
    if (target.kind === 'workspace') {
      const isDefault = this.#database.get<{ is_default: number }>(
        'SELECT is_default FROM workspaces WHERE id = ?', target.workspaceId
      )?.is_default
      if (isDefault === 1) {
        throw new RuntimeHostActionError('TARGET_NOT_READY', '默认工作空间会保留在侧栏中')
      }
    }
  }

  #acceptedBatchEnvironment(
    source: ResolvedHostEntity & { kind: 'session' },
    environment: ForkEnvironmentChoice
  ): ResolvedForkEnvironment {
    if (environment.mode === 'new-worktree') return environment
    if (environment.mode === 'current') {
      return this.#resolver.resolveForkEnvironment(source, environment)
    }
    const worktreeId = stableId(environment.worktreeRef, 'worktree')
    const worktree = worktreeId === undefined ? undefined : this.#database.get<{
      id: string
      execution_context_id: string
    }>(
      'SELECT id, execution_context_id FROM worktrees WHERE id = ?',
      worktreeId
    )
    if (!worktree) return this.#resolver.resolveForkEnvironment(source, environment)
    return {
      mode: 'existing-worktree',
      executionContextId: worktree.execution_context_id,
      worktreeId: worktree.id,
      worktreeRef: `worktree:${worktree.id}`,
      branch: environment.branch
    }
  }

  #commandReplay<T>(command: DomainCommandMetadata): T | undefined {
    const stored = this.#database.get<{ request_hash: string; response_json: string }>(
      `SELECT request_hash, response_json FROM command_deduplication
       WHERE command_id = ?`,
      command.commandId
    )
    if (!stored) return undefined
    if (stored.request_hash !== command.requestHash) {
      throw new RuntimeHostActionError(
        'PATH_CONFLICT',
        'submission key 已被不同输入使用'
      )
    }
    return JSON.parse(stored.response_json) as T
  }

  async #applySessionDisposals<T extends HostActionResult>(
    result: T,
    sessionIds: string[]
  ): Promise<T> {
    const unique = [...new Set(sessionIds)]
    if (unique.length > 0) {
      withHostControlPostResponseEffect(result, () => this.#disposeManagedSessions(unique))
    }
    return result
  }

  #assertWritable(): void {
    if (this.#database.readOnly) throw new StorageReadOnlyError()
  }
}

function actionCommand(
  caller: HostCallerIdentity,
  submissionKey: string,
  request: HostActionRequest
): DomainCommandMetadata {
  const requestHash = hash(canonicalJson({ callerSessionId: caller.sessionId, request }))
  return {
    commandId: `host-action:${submissionKey}`,
    commandType: request.method,
    requestHash,
    causationId: caller.runId,
    correlationId: `host-action:${submissionKey}`
  }
}

function singleForkBatchKey(submissionKey: string): string {
  return `host-single-fork:${hash(submissionKey)}`
}

function singleForkPublicItem(
  request: Extract<HostActionRequest, {
    method: 'structure.fork.child' | 'structure.fork.sibling'
  }>
): ForkItemInput {
  return {
    itemKey: request.method,
    title: request.title,
    environment: request.environment,
    ...(request.prompt === undefined ? {} : { prompt: request.prompt }),
    start: true
  }
}

function destructiveCommand(
  caller: HostCallerIdentity,
  confirmationRef: string,
  record: ConfirmationRecord,
  now: number
): DomainCommandMetadata {
  const referenceHash = hash(confirmationRef)
  return {
    commandId: `host-action:${record.action}:${referenceHash}`,
    commandType: `structure.${record.action}.commit`,
    requestHash: hash(canonicalJson({
      callerSessionId: caller.sessionId,
      targetRef: record.targetRef,
      scope: record.scope,
      impact: record.impact,
      now
    })),
    causationId: caller.runId,
    correlationId: `host-action-confirmation:${referenceHash}`
  }
}

function forkInput(
  source: ResolvedHostEntity & { kind: 'session' },
  name: string,
  environment: ResolvedForkEnvironment,
  submissionKey: string,
  now: number
) {
  return {
    windowId: source.windowId,
    sceneId: source.sceneId,
    sourceSessionId: source.sessionId,
    name,
    environment,
    submissionKey,
    now
  }
}

function publicEnvironment(
  environment: ResolvedForkEnvironment | ForkEnvironmentChoice
): ForkEnvironmentChoice {
  if (environment.mode === 'current') return { mode: 'current' }
  if (environment.mode === 'new-worktree') {
    return { mode: 'new-worktree', branch: environment.branch }
  }
  return {
    mode: 'existing-worktree',
    branch: environment.branch,
    worktreeRef: environment.worktreeRef
  }
}

function stableRef(target: ResolvedHostEntity): string {
  if (target.kind === 'workspace') return `workspace:${target.workspaceId}`
  if (target.kind === 'task') return `task:${target.taskId}`
  if (target.kind === 'canvas') return `scene:${target.sceneId}`
  return `session:${target.sessionId}`
}

function stableId(ref: string, prefix: string): string | undefined {
  const marker = `${prefix}:`
  if (!ref.startsWith(marker)) return undefined
  const id = ref.slice(marker.length)
  return id.length > 0 ? id : undefined
}

function createdRef(
  entity: 'workspace' | 'task' | 'canvas' | 'session',
  created: CreatedHierarchyPath
): string {
  if (entity === 'workspace') return `workspace:${created.workspace.id}`
  if (entity === 'task') return `task:${created.task.id}`
  if (entity === 'canvas') return `scene:${created.scene.id}`
  return `session:${created.session.id}`
}

function effectiveRemovalScope(
  target: ResolvedHostEntity,
  requested: 'node' | 'subtree'
): 'node' | 'subtree' {
  if (target.kind !== 'session') return 'subtree'
  return requested
}

function requireEntity<K extends ResolvedHostEntity['kind']>(
  target: ResolvedHostEntity,
  kind: K
): Extract<ResolvedHostEntity, { kind: K }> {
  if (target.kind !== kind) {
    throw new RuntimeHostActionError(
      'TARGET_NOT_FOUND',
      `目标类型不匹配，需要 ${kind}`
    )
  }
  return target as Extract<ResolvedHostEntity, { kind: K }>
}

async function acquireFocusWindowLocks(
  database: RuntimeDatabase,
  windowIds: string[]
): Promise<() => void> {
  // Facades sharing one Runtime database also share these short mutation leases.
  // Stable window ordering keeps overlapping caller/target pairs deadlock-free.
  let locks = DATABASE_FOCUS_WINDOW_LOCKS.get(database)
  if (locks === undefined) {
    locks = new Map()
    DATABASE_FOCUS_WINDOW_LOCKS.set(database, locks)
  }
  const releases: Array<() => void> = []
  try {
    for (const windowId of [...new Set(windowIds)].sort()) {
      const previous = locks.get(windowId) ?? Promise.resolve()
      let releaseTicket!: () => void
      const ticket = new Promise<void>((resolveTicket) => {
        releaseTicket = resolveTicket
      })
      const tail = previous.then(() => ticket)
      locks.set(windowId, tail)
      await previous
      let released = false
      releases.push(() => {
        if (released) return
        released = true
        releaseTicket()
        void tail.then(() => {
          if (locks?.get(windowId) === tail) locks.delete(windowId)
        })
      })
    }
  } catch (error) {
    for (const release of releases.reverse()) release()
    throw error
  }
  let released = false
  return () => {
    if (released) return
    released = true
    for (const release of releases.reverse()) release()
  }
}

async function validateWorkspaceDirectory(path: string): Promise<void> {
  try {
    const metadata = await stat(path)
    if (!metadata.isDirectory()) throw new Error('path is not a directory')
    await access(path, constants.R_OK | constants.X_OK)
  } catch (error) {
    throw new RuntimeHostActionError(
      'PATH_CONFLICT',
      `工作空间目录不可用: ${path}`,
      { cause: error }
    )
  }
}

function isStructuralMutation(method: HostActionMethod): boolean {
  return method.startsWith('structure.') && !method.endsWith('.preview')
}

function normalizeFacadeError(error: unknown): unknown {
  if (error instanceof RuntimeHostActionError) return error
  if (error instanceof ZodError) {
    return new RuntimeHostActionError(
      'INVALID_REQUEST',
      invalidRequestMessage(error),
      { cause: error }
    )
  }
  if (error instanceof HostActionTargetResolverError) {
    return new RuntimeHostActionError(error.code, error.message, {
      candidates: error.candidates,
      cause: error
    })
  }
  if (error instanceof HostActionConfirmationError) {
    return new RuntimeHostActionError(error.code, error.message, { cause: error })
  }
  if (error instanceof StorageReadOnlyError) {
    return new RuntimeHostActionError('STORAGE_READ_ONLY', error.message, { cause: error })
  }
  if (error instanceof WorkspacePathInvalidError) {
    return new RuntimeHostActionError('PATH_CONFLICT', error.message, { cause: error })
  }
  if (error instanceof ForkWorkflowError) {
    if (error.code === 'BRANCH_CONFLICT' || error.code === 'INVALID_BRANCH') {
      return new RuntimeHostActionError('BRANCH_CONFLICT', error.message, { cause: error })
    }
    if (error.code === 'WORKTREE_CONFLICT' || error.code === 'GIT_REPOSITORY_REQUIRED') {
      return new RuntimeHostActionError('WORKTREE_CONFLICT', error.message, { cause: error })
    }
    if (error.code === 'DUPLICATE_NAME') {
      return new RuntimeHostActionError('PATH_CONFLICT', error.message, { cause: error })
    }
    return new RuntimeHostActionError('TARGET_NOT_READY', error.message, { cause: error })
  }
  if (error instanceof Error) {
    if (
      error.message.includes('与已提交输入不一致') ||
      error.message.includes('was already used for a different request') ||
      error.message.includes('当前事项下已存在同名页签') ||
      error.message.includes('already exists in this Workspace') ||
      error.message.includes('该目录已经属于另一个工作空间')
    ) {
      return new RuntimeHostActionError('PATH_CONFLICT', error.message, { cause: error })
    }
    if (
      error.message.includes('does not exist') ||
      error.message.includes('不存在')
    ) {
      return new RuntimeHostActionError('TARGET_NOT_FOUND', error.message, { cause: error })
    }
    if (
      error.message.includes('Scene must keep one Session') ||
      error.message.includes('intent is stale')
    ) {
      return new RuntimeHostActionError('TARGET_NOT_READY', error.message, { cause: error })
    }
  }
  return error
}

function invalidRequestMessage(error: ZodError): string {
  const issue = error.issues[0]
  if (!issue) return '请求参数不符合动作约束'
  if (issue.code === 'unrecognized_keys') {
    const fields = issue.keys.slice(0, 3).map(safeFieldName).join(', ')
    return `请求参数包含不支持的字段: ${fields}`
  }
  const field = issue.path.length > 0
    ? issue.path.map((part) => safeFieldName(String(part))).join('.')
    : 'params'
  if (issue.code === 'invalid_type') {
    return `请求参数 ${field} 缺失或类型不正确`
  }
  if (issue.code === 'invalid_value') {
    return `请求参数 ${field} 的值不受支持`
  }
  if (issue.code === 'invalid_union') {
    return `请求参数 ${field} 不符合可用选择器格式`
  }
  return `请求参数 ${field} 不符合约束`
}

function safeFieldName(value: string): string {
  const normalized = value.replace(/[^a-zA-Z0-9_.[\]-]/g, '?').slice(0, 80)
  return normalized || 'params'
}

function hash(value: string): string {
  return createHash('sha256').update(value).digest('hex')
}

function canonicalJson(value: unknown): string {
  return JSON.stringify(canonicalValue(value))
}

function canonicalValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalValue)
  if (value === null || typeof value !== 'object') return value
  return Object.fromEntries(
    Object.entries(value)
      .filter(([, entry]) => entry !== undefined)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, entry]) => [key, canonicalValue(entry)])
  )
}
