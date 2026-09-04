import { execFile, spawnSync } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { realpathSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'

import type {
  DomainCommandMetadata,
  ForkProgress,
  SceneSessionGraph,
  Session,
  Worktree
} from '@matou/domain'

import {
  activateSessionInTransaction,
  assertWorkspacePathAvailable,
  readHierarchyResult,
  readHierarchyResultForSession,
  registerWindow,
  type WorkspaceHierarchyResult
} from '../hierarchy/hierarchy-application-service'
import type { ResolvedForkEnvironment } from '../control/host-action-target-resolver'
import { createHierarchyIds } from '../hierarchy/hierarchy-ids'
import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainMutationContext, DomainTransactionManager } from '../storage/domain-transaction'
import { SessionEnvironmentRepository } from '../session/session-environment-repository'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import {
  SessionForkIntentRepository,
  type ForkLease
} from '../session/session-fork-intent-repository'
import { SessionGitStateRepository } from '../session/session-git-state-repository'
import { WorktreeService, type WorktreeSetupStep } from '../worktrees/worktree-service'
import { createGitBranchName, validateDisplayName } from './branch-name'
import type { ForkKillPointObserver } from './fork-operation-coordinator'
import { projectSceneGraphFrom } from './session-graph-repository'

const exec = promisify(execFile)

export type ForkWorktreeMode = 'current' | 'new'
export type ForkWorkflowErrorCode =
  | 'EMPTY_NAME'
  | 'TOO_LONG_NAME'
  | 'DUPLICATE_NAME'
  | 'ROOT_HAS_NO_FORK_PARENT'
  | 'FORK_SOURCE_NOT_READY'
  | 'DETACHED_SOURCE'
  | 'GIT_REPOSITORY_REQUIRED'
  | 'INVALID_BRANCH'
  | 'BRANCH_CONFLICT'
  | 'WORKTREE_CONFLICT'
  | 'FORK_NOT_FAILED'

export class ForkWorkflowError extends Error {
  readonly code: ForkWorkflowErrorCode
  readonly input: string | undefined

  constructor(code: ForkWorkflowErrorCode, message: string, input?: string) {
    super(message)
    this.name = 'ForkWorkflowError'
    this.code = code
    this.input = input
  }
}

class StaleForkLeaseError extends Error {
  constructor() {
    super('stale Fork lease')
    this.name = 'StaleForkLeaseError'
  }
}

export interface CreateForkInput {
  windowId: string
  sceneId: string
  sourceSessionId: string
  name: string
  environment: ResolvedForkEnvironment
  submissionKey?: string
  now: number
}

/** Renderer RPC compatibility input; Host Control submits a resolved environment. */
export interface CreateForkModeInput {
  windowId: string
  sceneId: string
  sourceSessionId: string
  name: string
  worktreeMode: ForkWorktreeMode
  submissionKey?: string
  now: number
}

type ForkCreateRequest = CreateForkInput | CreateForkModeInput

export interface ForkWorkflowResult extends WorkspaceHierarchyResult {
  graph: SceneSessionGraph
  forkState: 'pending' | 'starting' | 'succeeded' | 'failed'
  forkProgress?: ForkProgress
  worktree?: Worktree
  error?: string
}

export interface RetryForkInput {
  windowId: string
  sceneId: string
  sessionId: string
  now: number
}

export interface RemoveFailedForkInput extends RetryForkInput {}

export interface ExecuteForkInput extends MutationLocation {
  operationId: string
  lease: ForkLease
  observer?: ForkKillPointObserver
}

type MutationLocation = Pick<RetryForkInput, 'windowId' | 'sceneId' | 'now'>

interface SceneRow {
  id: string
  task_id: string
}

interface SessionRow {
  id: string
  task_id: string
  execution_context_id: string
  kind: Session['kind']
  title: string
  cwd: string
}

interface MountRow {
  id: string
  scene_node_id: string | null
  scene_window_id: string | null
}

interface SceneNodeRow {
  id: string
  parent_node_id: string | null
  ordinal: number
}

interface TaskOwnerRow {
  id: string
  workspace_id: string
}

interface BindingRow {
  provider_session_id: string
  metadata_json: string
  restore_state: string
}

interface FrozenProviderContext {
  profile: 'claude-code' | 'codex'
  providerSessionId: string
  providerConfigId: string
  model: string
  permissionMode: 'default' | 'auto' | 'acceptEdits' | 'plan' | 'bypassPermissions'
}

interface SourceContext {
  scene: SceneRow
  task: TaskOwnerRow
  selected: SessionRow
  selectedMount: MountRow
  forkSource: SessionRow
  binding: BindingRow
}

interface GitPlan {
  repositoryRoot: string
  baseRef: string
  baseRevision: string
  path: string
  branch: string
  worktreeId: string
  executionContextId: string
}

interface ForkEnvironmentPlan {
  worktreeMode: ForkWorktreeMode
  executionContextId: string
  cwd: string
  targetExecutionContextId?: string
  gitPlan?: GitPlan
  existingWorktree?: Extract<ResolvedForkEnvironment, { mode: 'existing-worktree' }>
}

interface ForkIntentRow {
  session_id: string
  source_session_id: string
  state: 'pending' | 'starting' | 'succeeded' | 'failed'
  worktree_mode: ForkWorktreeMode
  worktree_id: string | null
  operation_id: string
}

export class ForkWorkflowService {
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager
  readonly #worktrees: WorktreeService
  readonly #environments: SessionEnvironmentRepository
  readonly #gitStates: SessionGitStateRepository
  readonly #forkIntents: SessionForkIntentRepository
  readonly #providerConfigs: ProviderConfigStore
  readonly #setupPolicyForWorkspace: (workspaceId: string) => WorktreeSetupStep[]
  readonly #onProgressCommitted: () => void
  readonly #accepting = new Map<string, Promise<ForkWorkflowResult>>()

  constructor(
    dataRoot: string,
    database: RuntimeDatabase,
    transactions: DomainTransactionManager,
    dependencies: {
      stopRuns: (runIds: string[]) => Promise<void>
      providerConfigs?: ProviderConfigStore
      setupPolicyForWorkspace?: (workspaceId: string) => WorktreeSetupStep[]
      onProgressCommitted?: () => void
    }
  ) {
    try {
      this.#dataRoot = realpathSync(dataRoot)
    } catch {
      this.#dataRoot = resolve(dataRoot)
    }
    this.#database = database
    this.#transactions = transactions
    this.#worktrees = new WorktreeService(database, transactions, dependencies)
    this.#environments = new SessionEnvironmentRepository(database)
    this.#gitStates = new SessionGitStateRepository(database)
    this.#forkIntents = new SessionForkIntentRepository(database)
    this.#providerConfigs = dependencies.providerConfigs ?? new ProviderConfigStore(this.#dataRoot)
    this.#setupPolicyForWorkspace = dependencies.setupPolicyForWorkspace ?? (() => [])
    this.#onProgressCommitted = dependencies.onProgressCommitted ?? (() => undefined)
  }

  createForkChild(command: DomainCommandMetadata, input: ForkCreateRequest): Promise<ForkWorkflowResult> {
    return this.#accept(command, input, 'child')
  }

  createForkSibling(command: DomainCommandMetadata, input: ForkCreateRequest): Promise<ForkWorkflowResult> {
    return this.#accept(command, input, 'sibling')
  }

  async executeFork(command: DomainCommandMetadata, input: ExecuteForkInput): Promise<ForkWorkflowResult> {
    const intent = requireRow(this.#database.get<ForkIntentRow & {
      target_execution_context_id: string | null
      worktree_path: string | null
      branch_name: string | null
    }>('SELECT * FROM session_fork_intents WHERE operation_id = ?', input.operationId), 'Fork intent')
    const session = requireRow(this.#database.get<{ task_id: string }>(
      'SELECT task_id FROM sessions WHERE id = ?', intent.session_id
    ), 'Fork Session')
    const owner = requireRow(this.#database.get<{ workspace_id: string }>(
      'SELECT workspace_id FROM tasks WHERE id = ?', session.task_id
    ), 'Fork owner')
    const focusedSessionId = projectSceneGraphFrom(
      this.#database, input.sceneId, input.windowId
    ).focusedSessionId
    const preserveFocus = focusedSessionId && focusedSessionId !== intent.session_id
      ? focusedSessionId
      : undefined
    return this.#withLeaseHeartbeat(input, async (renew, now) => {
      if (intent.worktree_mode === 'current') {
        const advanced = this.#forkIntents.advanceStage({
          operationId: input.operationId, lease: input.lease,
          stage: 'restoring-provider', now: renew()
        })
        if (advanced.kind === 'stale') throw new Error('stale Fork lease')
        return this.#readAcceptedResult(
          input.windowId, input.sceneId, intent.session_id, input.now,
          preserveFocus, advanced.progress
        )
      }
      const worktree = requireRow(
        intent.worktree_id ? this.#worktrees.get(intent.worktree_id) : undefined,
        'Fork Worktree intent'
      )
      try {
        let progress = this.#forkIntents.progressByOperation(input.operationId)!
        if (progress.stage === 'queued') {
          const advanced = this.#forkIntents.advanceStage({
            operationId: input.operationId, lease: input.lease,
            stage: 'creating-worktree', now: renew()
          })
          if (advanced.kind === 'stale') throw new Error('stale Fork lease')
          progress = advanced.progress
        }
        renew()
        const ready = worktree.state === 'ready'
          ? worktree
          : await this.#worktrees.create(derivedCommand(command, 'worktree'), {
              id: worktree.id,
              executionContextId: worktree.executionContextId,
              workspaceId: owner.workspace_id,
              repositoryRoot: worktree.repositoryRoot,
              path: worktree.path,
              branch: worktree.branch,
              baseRef: worktree.baseRevision ?? 'HEAD',
              setupPolicy: worktree.setupPolicy as WorktreeSetupStep[],
              now: input.now,
              beforeExternalSideEffect: renew,
              onSetupStarted: () => {
                if (progress.stage !== 'creating-worktree') return
                progress = this.#publishProgressStage(
                  derivedCommand(command, 'applying-setup'), input, intent.session_id,
                  input.operationId, input.lease, 'applying-setup', renew(), preserveFocus
                )
              },
              onCheckpoint: async (point) => {
                const operation = this.#forkIntents.operationById(input.operationId)
                if (operation) await input.observer?.reach(point, operation)
              }
            })
        renew()
        if (progress.stage === 'creating-worktree') {
          const advanced = this.#forkIntents.advanceStage({
            operationId: input.operationId, lease: input.lease,
            stage: 'applying-setup', now: renew()
          })
          if (advanced.kind === 'stale') throw new Error('stale Fork lease')
          progress = advanced.progress
        }
        if (progress.stage === 'applying-setup') {
          const advanced = this.#forkIntents.advanceStage({
            operationId: input.operationId, lease: input.lease,
            stage: 'binding-session', now: renew()
          })
          if (advanced.kind === 'stale') throw new Error('stale Fork lease')
          progress = advanced.progress
        }
        renew()
        await this.#gitStates.refresh(ready.executionContextId, now())
        renew()
        const bound = this.#bindReadyWorktree(
          derivedCommand(command, 'bind-worktree'), input, intent.session_id, ready,
          input.operationId, input.lease, now(), preserveFocus
        )
        const boundOperation = this.#forkIntents.operationById(input.operationId)
        if (boundOperation) await input.observer?.reach('session-bound', boundOperation)
        return bound
      } catch (error) {
        const failed = this.#markFailed(
          derivedCommand(command, 'failed'), input, intent.session_id, input.operationId,
          input.lease, errorMessage(error), now(), preserveFocus
        )
        if (!failed) throw error
        return failed
      }
    })
  }

  async #withLeaseHeartbeat<T>(
    input: ExecuteForkInput,
    operation: (renew: () => number, now: () => number) => Promise<T>
  ): Promise<T> {
    const wallStartedAt = Date.now()
    const now = () => input.now + Math.max(0, Date.now() - wallStartedAt)
    const ttlMs = Math.max(1, input.lease.expiresAt - input.now)
    let leaseLost = false
    const renew = () => {
      if (leaseLost) throw new StaleForkLeaseError()
      const heartbeatAt = now()
      const heartbeat = this.#forkIntents.heartbeat({
        operationId: input.operationId,
        lease: input.lease,
        now: heartbeatAt,
        ttlMs
      })
      if (heartbeat.kind === 'stale') {
        leaseLost = true
        throw new StaleForkLeaseError()
      }
      return heartbeatAt
    }
    renew()
    const heartbeatIntervalMs = Math.min(2_000, Math.max(10, Math.floor(ttlMs / 3)))
    const timer = setInterval(() => {
      try {
        renew()
      } catch {
        leaseLost = true
      }
    }, heartbeatIntervalMs)
    timer.unref?.()
    try {
      return await operation(renew, now)
    } finally {
      clearInterval(timer)
    }
  }

  async retryFork(command: DomainCommandMetadata, input: RetryForkInput): Promise<ForkWorkflowResult> {
    const intent = requireRow(this.#database.get<ForkIntentRow>(
      'SELECT * FROM session_fork_intents WHERE session_id = ?', input.sessionId
    ), 'Fork intent')
    if (intent.state !== 'failed') {
      throw new ForkWorkflowError('FORK_NOT_FAILED', '当前分支无需重试')
    }
    const owner = requireRow(this.#database.get<{ workspace_id: string }>(
      `SELECT tasks.workspace_id FROM sessions
       JOIN tasks ON tasks.id = sessions.task_id WHERE sessions.id = ?`,
      input.sessionId
    ), 'Fork owner')
    const prepared = this.#transactions.execute(command, ({ tx, emit }) => {
      const forkProgress = this.#forkIntents.retry(intent.operation_id, input.now, tx)
      tx.run(
        `UPDATE sessions SET status = 'starting', updated_at = ?, version = version + 1
         WHERE id = ? AND archived_at IS NULL`,
        input.now, input.sessionId
      )
      registerWindow(tx, input.windowId, input.now)
      activateSessionInTransaction(tx, input.windowId, input.sessionId, input.now)
      const result = readResult(tx, input.windowId, input.sceneId, input.sessionId)
      emit({
        eventId: `${command.commandId}:fork-retrying`, eventType: 'session.fork-retrying',
        aggregateType: 'session', aggregateId: input.sessionId,
        workspaceId: owner.workspace_id, taskId: result.session!.taskId,
        sessionId: input.sessionId,
        payload: { graph: result.graph }, occurredAt: input.now
      })
      return { ...result, forkState: 'pending' as const, forkProgress }
    }).result
    return prepared
  }

  #publishProgressStage(
    command: DomainCommandMetadata,
    input: MutationLocation,
    sessionId: string,
    operationId: string,
    lease: Pick<ForkLease, 'token' | 'fence'>,
    stage: 'applying-setup',
    now: number,
    preserveFocusedSessionId?: string
  ): ForkProgress {
    const progress = this.#transactions.execute(command, ({ tx, emit }) => {
      const advanced = this.#forkIntents.advanceStage({
        operationId, lease, stage, now
      }, tx)
      if (advanced.kind === 'stale') throw new StaleForkLeaseError()
      const result = readCreatedForkResult(
        tx, input.windowId, input.sceneId, sessionId, now, preserveFocusedSessionId
      )
      emit({
        eventId: `${command.commandId}:fork-progress`,
        eventType: 'session.fork-progressed',
        aggregateType: 'session', aggregateId: sessionId,
        taskId: result.session!.taskId, sessionId,
        payload: { graph: result.graph, forkProgress: advanced.progress },
        occurredAt: now
      })
      return advanced.progress
    }).result
    this.#onProgressCommitted()
    return progress
  }

  removeFailedFork(
    command: DomainCommandMetadata,
    input: RemoveFailedForkInput
  ): ForkWorkflowResult {
    const intent = requireRow(this.#database.get<ForkIntentRow>(
      'SELECT * FROM session_fork_intents WHERE session_id = ?', input.sessionId
    ), 'Fork intent')
    if (intent.state !== 'failed') {
      throw new ForkWorkflowError('FORK_NOT_FAILED', '当前分支无需移除')
    }
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const session = requireRow(tx.get<{ task_id: string }>(
        'SELECT task_id FROM sessions WHERE id = ? AND archived_at IS NULL', input.sessionId
      ), 'Fork Session')
      const task = requireRow(tx.get<TaskOwnerRow>(
        'SELECT id, workspace_id FROM tasks WHERE id = ?', session.task_id
      ), 'Task')
      const relation = tx.get<{
        relation_id: string
        to_session_id: string
        relation_kind: 'forked-from'
        metadata_json: string
      }>(
        `SELECT relation_id, to_session_id, relation_kind, metadata_json
         FROM session_relations_current
         WHERE from_session_id = ? AND relation_kind = 'forked-from'`,
        input.sessionId
      )
      const mount = tx.get<{ id: string; scene_node_id: string | null }>(
        `SELECT id, scene_node_id FROM session_mounts
         WHERE session_id = ? AND scene_id = ? ORDER BY created_at, id LIMIT 1`,
        input.sessionId, input.sceneId
      )
      if (relation) {
        tx.run(
          `INSERT INTO session_relation_events (
             event_id, relation_id, operation, task_id, from_session_id, to_session_id,
             relation_kind, metadata_json, command_id, occurred_at
           ) VALUES (?, ?, 'revoked', ?, ?, ?, ?, ?, ?, ?)`,
          `${command.commandId}:fork-relation-revoked`, relation.relation_id, task.id,
          input.sessionId, relation.to_session_id, relation.relation_kind,
          relation.metadata_json, command.commandId, input.now
        )
        tx.run('DELETE FROM session_relations_current WHERE relation_id = ?', relation.relation_id)
      }
      tx.run('DELETE FROM session_canvas_memberships WHERE session_id = ?', input.sessionId)
      if (mount) {
        tx.run('DELETE FROM session_mounts WHERE id = ?', mount.id)
        if (mount.scene_node_id) removeMountNode(tx, input.sceneId, mount.scene_node_id)
      }
      tx.run(
        `UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?,
           version = version + 1 WHERE id = ?`,
        input.now, input.now, input.sessionId
      )
      registerWindow(tx, input.windowId, input.now)
      if (relation) activateSessionInTransaction(tx, input.windowId, relation.to_session_id, input.now)
      const hierarchy = readHierarchyResult(tx, input.windowId)
      const graph = projectSceneGraphFrom(tx, input.sceneId, input.windowId)
      emit({
        eventId: `${command.commandId}:fork-removed`, eventType: 'session.fork-removed',
        aggregateType: 'session', aggregateId: input.sessionId,
        workspaceId: task.workspace_id, taskId: task.id, sessionId: input.sessionId,
        payload: { graph, retainedWorktreeId: intent.worktree_id }, occurredAt: input.now
      })
      return { ...hierarchy, graph, forkState: 'failed' as const }
    }).result
  }

  #accept(
    command: DomainCommandMetadata,
    input: ForkCreateRequest,
    placement: 'child' | 'sibling'
  ): Promise<ForkWorkflowResult> {
    const submissionKey = input.submissionKey ?? command.commandId
    const accepted = this.#forkIntents.findBySubmissionKey(submissionKey)
    if (accepted) {
      const result = readAcceptedForkResult(
        this.#database, input.windowId, accepted.identity.sessionId
      )
      return Promise.resolve({
        ...result,
        forkState: legacyForkState(accepted.progress.stage),
        forkProgress: accepted.progress
      })
    }
    const accepting = this.#accepting.get(submissionKey)
    if (accepting) return accepting
    const pending = this.#acceptNew(command, input, placement, submissionKey)
    this.#accepting.set(submissionKey, pending)
    void pending.then(
      () => { if (this.#accepting.get(submissionKey) === pending) this.#accepting.delete(submissionKey) },
      () => { if (this.#accepting.get(submissionKey) === pending) this.#accepting.delete(submissionKey) }
    )
    return pending
  }

  async #acceptNew(
    command: DomainCommandMetadata,
    input: ForkCreateRequest,
    placement: 'child' | 'sibling',
    submissionKey: string
  ): Promise<ForkWorkflowResult> {
    const ids = createHierarchyIds()
    const relationId = randomUUID()
    const operationId = randomUUID()
    const source = this.#resolveSource(input, placement)
    const providerContext = await this.#freezeProviderContext(source)
    const activeNames = this.#activeChildNames(source.forkSource.id)
    const name = validateDisplayName(input.name, activeNames)
    if (!name.ok) throw displayNameError(name.code, name.message, name.input)

    const environment = await this.#resolveEnvironment(
      input, source, name.displayName, ids.sessionId
    )
    const initial = this.#createPreparingNode(
      command, input, source, name.displayName, ids, relationId, operationId, submissionKey,
      environment,
      providerContext,
      placement === 'sibling' ? source.selected.id : undefined
    )
    return initial
  }

  #readAcceptedResult(
    windowId: string,
    sceneId: string,
    sessionId: string,
    now: number,
    preserveFocusedSessionId: string | undefined,
    forkProgress: ForkProgress
  ): ForkWorkflowResult {
    return this.#database.transaction((tx) => {
      const result = readCreatedForkResult(
        tx, windowId, sceneId, sessionId, now, preserveFocusedSessionId
      )
      return {
        ...result,
        forkState: legacyForkState(forkProgress.stage),
        forkProgress
      }
    })
  }

  #resolveSource(input: ForkCreateRequest, placement: 'child' | 'sibling'): SourceContext {
    const scene = requireRow(this.#database.get<SceneRow>(
      'SELECT id, task_id FROM scenes WHERE id = ? AND archived_at IS NULL', input.sceneId
    ), 'Scene')
    const task = requireRow(this.#database.get<TaskOwnerRow>(
      'SELECT id, workspace_id FROM tasks WHERE id = ? AND archived_at IS NULL', scene.task_id
    ), 'Task')
    assertWorkspacePathAvailable(this.#database, task.workspace_id)
    const selected = requireRow(this.#database.get<SessionRow>(
      `SELECT sessions.id, sessions.task_id, sessions.execution_context_id,
              sessions.kind, sessions.title, sessions.cwd
       FROM sessions
       JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
       WHERE sessions.id = ? AND membership.scene_id = ? AND sessions.archived_at IS NULL`,
      input.sourceSessionId, input.sceneId
    ), 'Session')
    const selectedMount = requireRow(this.#database.get<MountRow>(
      `SELECT id, scene_node_id, scene_window_id FROM session_mounts
       WHERE scene_id = ? AND session_id = ? ORDER BY created_at, id LIMIT 1`,
      input.sceneId, input.sourceSessionId
    ), 'SessionMount')
    if (selectedMount.scene_window_id !== null || selectedMount.scene_node_id === null) {
      throw new ForkWorkflowError('DETACHED_SOURCE', '请先把会话返回当前画布')
    }

    let forkSource = selected
    if (placement === 'sibling') {
      const parent = this.#database.get<{ parent_session_id: string }>(
        `SELECT to_session_id AS parent_session_id FROM session_relations_current
         WHERE from_session_id = ? AND relation_kind IN ('derived-from', 'forked-from')`,
        selected.id
      )
      if (!parent) {
        throw new ForkWorkflowError('ROOT_HAS_NO_FORK_PARENT', '根层会话可创建子分支')
      }
      forkSource = requireRow(this.#database.get<SessionRow>(
        `SELECT id, task_id, execution_context_id, kind, title, cwd
         FROM sessions WHERE id = ? AND archived_at IS NULL`,
        parent.parent_session_id
      ), 'Parent Session')
    }
    const binding = this.#validForkBinding(forkSource)
    return { scene, task, selected, selectedMount, forkSource, binding }
  }

  #validForkBinding(source: SessionRow): BindingRow {
    if (source.kind !== 'claude-code' && source.kind !== 'codex') {
      throw new ForkWorkflowError(
        'FORK_SOURCE_NOT_READY', '完成首轮 AI 对话后可创建分支'
      )
    }
    const binding = this.#database.get<BindingRow>(
      `SELECT provider_session_id, metadata_json, restore_state
       FROM provider_bindings
       WHERE session_id = ? AND provider = ?
         AND resume_state IN ('available', 'resumed')
         AND validated_at IS NOT NULL AND invalidated_at IS NULL
       ORDER BY updated_at DESC, id DESC LIMIT 1`,
      source.id,
      source.kind
    )
    if (
      !binding || binding.restore_state !== 'none' ||
      (source.kind === 'claude-code' && metadata(binding.metadata_json).canFork !== true)
    ) {
      throw new ForkWorkflowError(
        'FORK_SOURCE_NOT_READY', '完成首轮 AI 对话后可创建分支'
      )
    }
    return binding
  }

  async #freezeProviderContext(source: SourceContext): Promise<FrozenProviderContext> {
    const profile = source.forkSource.kind
    if (profile !== 'claude-code' && profile !== 'codex') {
      throw new ForkWorkflowError('FORK_SOURCE_NOT_READY', '完成首轮 AI 对话后可创建分支')
    }
    const persisted = metadata(source.binding.metadata_json)
    const providerConfigId = typeof persisted.providerConfigId === 'string' &&
      persisted.providerConfigId.trim()
      ? persisted.providerConfigId
      : undefined
    const selected = await this.#providerConfigs.launchSelection(profile, providerConfigId)
    return {
      profile,
      providerSessionId: source.binding.provider_session_id,
      providerConfigId: selected.providerConfigId,
      model: typeof persisted.model === 'string' ? persisted.model : selected.model,
      permissionMode: providerPermissionMode(persisted.permissionMode)
    }
  }

  #activeChildNames(parentSessionId: string): string[] {
    return this.#database.all<{ title: string }>(
      `SELECT sessions.title
       FROM session_relations_current AS relation
       JOIN sessions ON sessions.id = relation.from_session_id
       WHERE relation.to_session_id = ?
         AND relation.relation_kind IN ('derived-from', 'forked-from')
         AND sessions.archived_at IS NULL`,
      parentSessionId
    ).map(({ title }) => title)
  }

  async #resolveEnvironment(
    input: ForkCreateRequest,
    source: SourceContext,
    displayName: string,
    sessionId: string
  ): Promise<ForkEnvironmentPlan> {
    if (!('environment' in input) && input.worktreeMode === 'current') {
      return {
        worktreeMode: 'current',
        executionContextId: source.forkSource.execution_context_id,
        cwd: source.forkSource.cwd
      }
    }
    const environment = 'environment' in input
      ? input.environment
      : {
          mode: 'new-worktree' as const,
          branch: createGitBranchName(displayName, sessionId)
        }
    if (environment.mode === 'new-worktree') {
      const gitPlan = await this.#resolveGitPlan(source, environment.branch, sessionId)
      return {
        worktreeMode: 'new',
        executionContextId: source.forkSource.execution_context_id,
        cwd: source.forkSource.cwd,
        gitPlan
      }
    }
    if (environment.mode === 'existing-worktree') {
      const target = this.#assertExistingWorktreeAvailable(
        this.#database, source.task.workspace_id, environment
      )
      return {
        worktreeMode: 'current',
        executionContextId: target.execution_context_id,
        cwd: target.cwd,
        targetExecutionContextId: target.execution_context_id,
        existingWorktree: environment
      }
    }
    const target = this.#database.get<{ id: string; cwd: string; workspace_id: string }>(
      `SELECT id, cwd, workspace_id FROM execution_contexts
       WHERE id = ? AND archived_at IS NULL`,
      environment.executionContextId
    )
    if (!target || target.workspace_id !== source.task.workspace_id) {
      throw new ForkWorkflowError('WORKTREE_CONFLICT', '当前执行环境已不可用')
    }
    return {
      worktreeMode: 'current',
      executionContextId: target.id,
      cwd: target.cwd
    }
  }

  #assertExistingWorktreeAvailable(
    source: DatabaseTransaction,
    workspaceId: string,
    environment: Extract<ResolvedForkEnvironment, { mode: 'existing-worktree' }>
  ): { execution_context_id: string; cwd: string } {
    const target = source.get<{
      execution_context_id: string
      cwd: string
      workspace_id: string
      branch_name: string
      worktree_state: Worktree['state']
      worktree_repository_root: string
      git_state: 'ready' | 'unavailable' | null
      git_repository_root: string | null
      git_branch: string | null
    }>(
      `SELECT execution_contexts.id AS execution_context_id, execution_contexts.cwd,
              execution_contexts.workspace_id, worktrees.branch_name,
              worktrees.state AS worktree_state,
              worktrees.repository_root AS worktree_repository_root,
              git_state.state AS git_state,
              git_state.repository_root AS git_repository_root,
              git_state.branch AS git_branch
       FROM execution_contexts
       JOIN worktrees ON worktrees.execution_context_id = execution_contexts.id
       LEFT JOIN execution_context_git_states AS git_state
         ON git_state.execution_context_id = execution_contexts.id
       WHERE execution_contexts.id = ? AND worktrees.id = ?
         AND execution_contexts.archived_at IS NULL`,
      environment.executionContextId, environment.worktreeId
    )
    if (
      !target || target.workspace_id !== workspaceId ||
      !['ready', 'dirty', 'retained'].includes(target.worktree_state) ||
      target.git_state !== 'ready' || target.git_repository_root === null ||
      target.git_repository_root !== target.worktree_repository_root ||
      environment.worktreeRef !== `worktree:${environment.worktreeId}`
    ) {
      throw new ForkWorkflowError(
        'WORKTREE_CONFLICT', '指定的 Worktree 已不可用', environment.worktreeRef
      )
    }
    if (target.branch_name !== environment.branch || target.git_branch !== environment.branch) {
      throw new ForkWorkflowError(
        'BRANCH_CONFLICT', `Worktree 当前分支与提交的 ${environment.branch} 不一致`,
        environment.branch
      )
    }
    return target
  }

  async #resolveGitPlan(
    source: SourceContext,
    branch: string,
    sessionId: string
  ): Promise<GitPlan> {
    const git = this.#database.get<{ repository_root: string }>(
      `SELECT repository_root FROM execution_context_git_states
       WHERE execution_context_id = ? AND state = 'ready' AND repository_root IS NOT NULL`,
      source.forkSource.execution_context_id
    )
    if (!git) throw new ForkWorkflowError('GIT_REPOSITORY_REQUIRED', '新工作树需要 Git 仓库')
    await assertBranchAvailable(git.repository_root, branch)
    const baseRevision = (await exec(
      'git', ['-C', git.repository_root, 'rev-parse', 'HEAD']
    )).stdout.trim()
    const canonicalDataRoot = this.#dataRoot
    return {
      repositoryRoot: git.repository_root,
      baseRef: baseRevision,
      baseRevision,
      path: join(canonicalDataRoot, 'worktrees', source.task.workspace_id, sessionId),
      branch,
      worktreeId: randomUUID(),
      executionContextId: randomUUID()
    }
  }

  #createPreparingNode(
    command: DomainCommandMetadata,
    input: ForkCreateRequest,
    source: SourceContext,
    displayName: string,
    ids: ReturnType<typeof createHierarchyIds>,
    relationId: string,
    operationId: string,
    submissionKey: string,
    environment: ForkEnvironmentPlan,
    providerContext: FrozenProviderContext,
    preserveFocusedSessionId?: string
  ): ForkWorkflowResult {
    const gitPlan = environment.gitPlan
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const duplicate = this.#forkIntents.findBySubmissionKey(submissionKey, tx)
      if (duplicate) {
        const result = readAcceptedForkResult(
          tx, input.windowId, duplicate.identity.sessionId
        )
        return {
          ...result,
          forkState: legacyForkState(duplicate.progress.stage),
          forkProgress: duplicate.progress
        }
      }
      if (gitPlan) {
        assertBranchAvailableSync(gitPlan.repositoryRoot, gitPlan.branch)
        assertDurableBranchAvailable(
          tx, gitPlan, operationId, submissionKey
        )
      }
      if (environment.existingWorktree) {
        this.#assertExistingWorktreeAvailable(
          tx, source.task.workspace_id, environment.existingWorktree
        )
      }
      registerWindow(tx, input.windowId, input.now)
      const acceptedName = validateDisplayName(
        displayName,
        activeChildNamesFrom(tx, source.forkSource.id)
      )
      if (!acceptedName.ok) {
        throw displayNameError(acceptedName.code, acceptedName.message, acceptedName.input)
      }
      const sourceNode = requireRow(tx.get<SceneNodeRow>(
        'SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?',
        source.selectedMount.scene_node_id!
      ), 'SceneNode')
      insertHorizontalMount(tx, input.sceneId, sourceNode, ids, input.now)
      if (gitPlan) {
        tx.run(
          `INSERT INTO execution_contexts (
             id, workspace_id, kind, cwd, created_at
           ) VALUES (?, ?, 'git-worktree', ?, ?)`,
          gitPlan.executionContextId, source.task.workspace_id, gitPlan.path, input.now
        )
        tx.run(
          `INSERT INTO worktrees (
             id, execution_context_id, repository_root, worktree_path, branch_name,
             base_ref, base_revision, state, setup_policy_json, setup_result_json, cleanup_policy,
             created_at, updated_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, 'creating', ?, '[]', 'retain-dirty', ?, ?)`,
          gitPlan.worktreeId, gitPlan.executionContextId, gitPlan.repositoryRoot,
          gitPlan.path, gitPlan.branch, gitPlan.baseRef, gitPlan.baseRevision,
          JSON.stringify(this.#setupPolicyForWorkspace(source.task.workspace_id)),
          input.now, input.now
        )
      }
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, ?, 'starting', ?, ?, ?, ?, ?, 1)`,
        ids.sessionId, source.task.id, environment.executionContextId, providerContext.profile,
        displayName, environment.cwd, input.now, input.now, input.now
      )
      tx.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, restore_state,
           metadata_json, created_at, updated_at, validated_at
         ) VALUES (?, ?, ?, ?, 'unknown', 'none', ?, ?, ?, NULL)`,
        randomUUID(), ids.sessionId, providerContext.profile,
        providerContext.providerSessionId,
        JSON.stringify({
          providerConfigId: providerContext.providerConfigId,
          model: providerContext.model,
          permissionMode: providerContext.permissionMode,
          inheritedConversation: true
        }),
        input.now,
        input.now
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
      const acceptance = this.#forkIntents.accept({
        operationId,
        submissionKey,
        sessionId: ids.sessionId,
        sourceSessionId: source.forkSource.id,
        sourceProviderSessionId: source.binding.provider_session_id,
        displayName,
        worktreeMode: environment.worktreeMode,
        totalSteps: gitPlan ? 5 : 2,
        now: input.now,
        ...(gitPlan ? {
          worktreeId: gitPlan.worktreeId,
          executionContextId: gitPlan.executionContextId,
          worktreePath: gitPlan.path,
          branchName: gitPlan.branch
        } : environment.targetExecutionContextId === undefined ? {} : {
          executionContextId: environment.targetExecutionContextId
        })
      }, tx)
      if (gitPlan) {
        tx.run(
          `UPDATE session_environment_bindings
           SET managed_worktree_id = ?, active_target = 'worktree', state = 'recovering',
               error_message = NULL, updated_at = ? WHERE session_id = ?`,
          gitPlan.worktreeId, input.now, ids.sessionId
        )
      }
      const relationInsertion = tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id, to_session_id,
           relation_kind, metadata_json, command_id, occurred_at
         ) VALUES (?, ?, 'created', ?, ?, ?, 'forked-from', ?, ?, ?)`,
        `${command.commandId}:fork-relation-created`, relationId, source.task.id,
        ids.sessionId, source.forkSource.id,
        JSON.stringify({ worktreeMode: environment.worktreeMode }), command.commandId, input.now
      )
      tx.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           metadata_json, created_at, updated_at, source_event_sequence
         ) VALUES (?, ?, ?, ?, 'forked-from', ?, ?, ?, ?)`,
        relationId, source.task.id, ids.sessionId, source.forkSource.id,
        JSON.stringify({ worktreeMode: environment.worktreeMode }), input.now, input.now,
        Number(relationInsertion.lastInsertRowid)
      )
      const result = readCreatedForkResult(
        tx, input.windowId, input.sceneId, ids.sessionId, input.now, preserveFocusedSessionId
      )
      emitForkEvents(emit, command.commandId, source, result, relationId, input.now)
      return {
        ...result,
        forkState: 'pending' as const,
        forkProgress: acceptance.progress
      }
    }).result
  }

  #bindReadyWorktree(
    command: DomainCommandMetadata,
    input: MutationLocation,
    sessionId: string,
    worktree: Worktree,
    operationId: string,
    lease: Pick<ForkLease, 'token' | 'fence'>,
    now: number,
    preserveFocusedSessionId?: string
  ): ForkWorkflowResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const advanced = this.#forkIntents.advanceStage({
        operationId, lease, stage: 'restoring-provider', now
      }, tx)
      if (advanced.kind === 'stale') throw new StaleForkLeaseError()
      this.#environments.bindOwnedWorktree({
        sessionId,
        worktreeId: worktree.id,
        activate: true,
        now
      }, tx)
      tx.run(
        `UPDATE session_fork_intents SET worktree_id = ?, target_execution_context_id = ?,
           worktree_path = ?, branch_name = ?, updated_at = ? WHERE session_id = ?`,
        worktree.id, worktree.executionContextId, worktree.path, worktree.branch,
        now, sessionId
      )
      const result = readCreatedForkResult(
        tx, input.windowId, input.sceneId, sessionId, now, preserveFocusedSessionId
      )
      emit({
        eventId: `${command.commandId}:worktree-ready`,
        eventType: 'session.fork-worktree-ready',
        aggregateType: 'session', aggregateId: sessionId,
        taskId: result.session!.taskId, sessionId,
        payload: { session: result.session, worktree, graph: result.graph },
        occurredAt: now
      })
      return {
        ...result,
        forkState: 'starting' as const,
        forkProgress: advanced.progress,
        worktree
      }
    }).result
  }

  #markFailed(
    command: DomainCommandMetadata,
    input: MutationLocation,
    sessionId: string,
    operationId: string,
    lease: Pick<ForkLease, 'token' | 'fence'>,
    reason: string,
    now: number,
    preserveFocusedSessionId?: string
  ): ForkWorkflowResult | undefined {
    try {
      return this.#transactions.execute(command, ({ tx, emit }) => {
        const failed = this.#forkIntents.failOperation({
          operationId, lease, error: reason, now
        }, tx)
        if (failed.kind === 'stale') throw new StaleForkLeaseError()
        tx.run(
          `UPDATE sessions SET status = 'interrupted', updated_at = ?,
             version = version + 1 WHERE id = ?`,
          now, sessionId
        )
        const result = readCreatedForkResult(
          tx, input.windowId, input.sceneId, sessionId, now, preserveFocusedSessionId
        )
        emit({
          eventId: `${command.commandId}:fork-failed`, eventType: 'session.fork-failed',
          aggregateType: 'session', aggregateId: sessionId,
          taskId: result.session!.taskId, sessionId,
          payload: { error: reason, graph: result.graph }, occurredAt: now
        })
        return {
          ...result, forkState: 'failed' as const, error: reason,
          forkProgress: failed.progress
        }
      }).result
    } catch (error) {
      if (error instanceof StaleForkLeaseError) return undefined
      throw error
    }
  }
}

function insertHorizontalMount(
  tx: DatabaseTransaction,
  sceneId: string,
  sourceNode: SceneNodeRow,
  ids: ReturnType<typeof createHierarchyIds>,
  now: number
): void {
  tx.run(
    `INSERT INTO scene_nodes (
       id, scene_id, parent_node_id, kind, direction, ordinal, created_at
     ) VALUES (?, ?, ?, 'split', 'horizontal', ?, ?)`,
    ids.secondaryNodeId, sceneId, sourceNode.parent_node_id, sourceNode.ordinal, now
  )
  tx.run(
    `UPDATE scene_nodes SET parent_node_id = ?, kind = 'mount', direction = NULL, ordinal = 0
     WHERE id = ?`,
    ids.secondaryNodeId, sourceNode.id
  )
  tx.run(
    `INSERT INTO scene_nodes (
       id, scene_id, parent_node_id, kind, ordinal, created_at
     ) VALUES (?, ?, ?, 'mount', 1, ?)`,
    ids.rootNodeId, sceneId, ids.secondaryNodeId, now
  )
}

function removeMountNode(tx: DatabaseTransaction, sceneId: string, nodeId: string): void {
  const node = tx.get<{ parent_node_id: string | null }>(
    'SELECT parent_node_id FROM scene_nodes WHERE id = ? AND scene_id = ?', nodeId, sceneId
  )
  if (!node) return
  if (node.parent_node_id === null) {
    tx.run('DELETE FROM scene_nodes WHERE id = ?', nodeId)
    return
  }
  const parent = tx.get<{ parent_node_id: string | null; ordinal: number }>(
    'SELECT parent_node_id, ordinal FROM scene_nodes WHERE id = ?', node.parent_node_id
  )
  const siblings = tx.all<{ id: string }>(
    'SELECT id FROM scene_nodes WHERE parent_node_id = ? AND id <> ? ORDER BY ordinal, id',
    node.parent_node_id, nodeId
  )
  tx.run('DELETE FROM scene_nodes WHERE id = ?', nodeId)
  if (!parent || siblings.length !== 1) return
  const survivor = siblings[0]!.id
  tx.run(
    'UPDATE scene_nodes SET parent_node_id = ?, ordinal = ? WHERE id = ?',
    parent.parent_node_id, parent.ordinal, survivor
  )
  tx.run(
    `UPDATE scenes SET root_node_id = CASE WHEN root_node_id = ? THEN ? ELSE root_node_id END,
     layout_revision = layout_revision + 1 WHERE id = ?`,
    node.parent_node_id, survivor, sceneId
  )
  tx.run('DELETE FROM scene_nodes WHERE id = ?', node.parent_node_id)
}

function readResult(
  tx: DatabaseTransaction,
  windowId: string,
  sceneId: string,
  sessionId: string
): WorkspaceHierarchyResult & { graph: SceneSessionGraph } {
  const hierarchy = readHierarchyResult(tx, windowId)
  if (hierarchy.session?.id !== sessionId) throw new Error('Fork Session did not become active')
  return { ...hierarchy, graph: projectSceneGraphFrom(tx, sceneId, windowId) }
}

function readAcceptedForkResult(
  tx: DatabaseTransaction,
  windowId: string,
  sessionId: string
): WorkspaceHierarchyResult & { graph: SceneSessionGraph } {
  const hierarchy = readHierarchyResultForSession(tx, windowId, sessionId)
  return {
    ...hierarchy,
    graph: projectSceneGraphFrom(tx, hierarchy.scene!.id, windowId)
  }
}

function readCreatedForkResult(
  tx: DatabaseTransaction,
  windowId: string,
  sceneId: string,
  createdSessionId: string,
  now: number,
  preserveFocusedSessionId?: string
): WorkspaceHierarchyResult & { graph: SceneSessionGraph } {
  activateSessionInTransaction(tx, windowId, createdSessionId, now)
  const created = readResult(tx, windowId, sceneId, createdSessionId)
  if (preserveFocusedSessionId === undefined) return created

  // A sibling is appended to the common parent's queue, but creating it is
  // not a navigation action. Restore the selected sibling inside the same
  // transaction so no intermediate projection can move the user's viewport.
  activateSessionInTransaction(tx, windowId, preserveFocusedSessionId, now)
  const active = readHierarchyResult(tx, windowId)
  return {
    ...created,
    navigation: active.navigation,
    graph: projectSceneGraphFrom(tx, sceneId, windowId)
  }
}

function emitForkEvents(
  emit: DomainMutationContext['emit'],
  commandId: string,
  source: SourceContext,
  result: WorkspaceHierarchyResult & { graph: SceneSessionGraph },
  relationId: string,
  now: number
): void {
  const session = result.session!
  emit({
    eventId: `${commandId}:session-created`, eventType: 'session.created',
    aggregateType: 'session', aggregateId: session.id,
    workspaceId: source.task.workspace_id, taskId: source.task.id, sessionId: session.id,
    payload: session, occurredAt: now
  })
  emit({
    eventId: `${commandId}:relation-created`, eventType: 'session.structural-relation-created',
    aggregateType: 'session-relation', aggregateId: relationId,
    workspaceId: source.task.workspace_id, taskId: source.task.id, sessionId: session.id,
    payload: { graph: result.graph }, occurredAt: now
  })
  emit({
    eventId: `${commandId}:fork-created`, eventType: 'session.graph-summary-changed',
    aggregateType: 'scene', aggregateId: source.scene.id,
    workspaceId: source.task.workspace_id, taskId: source.task.id, sessionId: session.id,
    payload: { graph: result.graph }, occurredAt: now
  })
}

function displayNameError(
  code: 'EMPTY' | 'TOO_LONG' | 'DUPLICATE',
  message: string,
  input: string
): ForkWorkflowError {
  const mapped = code === 'EMPTY'
    ? 'EMPTY_NAME'
    : code === 'TOO_LONG'
      ? 'TOO_LONG_NAME'
      : 'DUPLICATE_NAME'
  return new ForkWorkflowError(mapped, message, input)
}

function metadata(value: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(value) as unknown
    return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : {}
  } catch {
    return {}
  }
}

function providerPermissionMode(
  value: unknown
): FrozenProviderContext['permissionMode'] {
  return value === 'auto' || value === 'acceptEdits' || value === 'plan' ||
    value === 'bypassPermissions'
    ? value
    : 'default'
}

function activeChildNamesFrom(tx: DatabaseTransaction, parentSessionId: string): string[] {
  return tx.all<{ title: string }>(
    `SELECT sessions.title
     FROM session_relations_current AS relation
     JOIN sessions ON sessions.id = relation.from_session_id
     WHERE relation.to_session_id = ?
       AND relation.relation_kind IN ('derived-from', 'forked-from')
       AND sessions.archived_at IS NULL`,
    parentSessionId
  ).map(({ title }) => title)
}

function legacyForkState(stage: ForkProgress['stage']): ForkWorkflowResult['forkState'] {
  if (stage === 'succeeded') return 'succeeded'
  if (stage === 'failed') return 'failed'
  if (stage === 'queued') return 'pending'
  return 'starting'
}

function derivedCommand(command: DomainCommandMetadata, suffix: string): DomainCommandMetadata {
  return {
    commandId: `${command.commandId}:${suffix}`,
    commandType: `${command.commandType}.${suffix}`,
    requestHash: `${command.requestHash}:${suffix}`,
    ...(command.causationId === undefined ? {} : { causationId: command.causationId }),
    ...(command.correlationId === undefined ? {} : { correlationId: command.correlationId })
  }
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (row === undefined) throw new Error(`${label} does not exist`)
  return row
}

async function assertBranchAvailable(repositoryRoot: string, branch: string): Promise<void> {
  try {
    const checked = await exec('git', ['check-ref-format', '--branch', branch])
    if (checked.stdout.trim() !== branch) throw new Error('branch shorthand is not stable')
  } catch {
    throw new ForkWorkflowError('INVALID_BRANCH', '分支名称无效', branch)
  }
  try {
    await exec('git', [
      '-C', repositoryRoot, 'show-ref', '--verify', '--quiet', `refs/heads/${branch}`
    ])
  } catch (error) {
    if (processExitCode(error) === 1) return
    throw error
  }
  throw new ForkWorkflowError('BRANCH_CONFLICT', `分支 ${branch} 已存在`, branch)
}

function assertBranchAvailableSync(repositoryRoot: string, branch: string): void {
  const checked = spawnSync('git', ['check-ref-format', '--branch', branch], {
    encoding: 'utf8'
  })
  if (checked.error || checked.status !== 0 || checked.stdout.trim() !== branch) {
    throw new ForkWorkflowError('INVALID_BRANCH', '分支名称无效', branch)
  }
  const collision = spawnSync(
    'git', ['-C', repositoryRoot, 'show-ref', '--verify', '--quiet', `refs/heads/${branch}`],
    { encoding: 'utf8' }
  )
  if (collision.error) throw collision.error
  if (collision.status === 1) return
  if (collision.status === 0) {
    throw new ForkWorkflowError('BRANCH_CONFLICT', `分支 ${branch} 已存在`, branch)
  }
  throw new Error(collision.stderr.trim() || `Git branch check failed with ${collision.status}`)
}

function assertDurableBranchAvailable(
  source: DatabaseTransaction,
  gitPlan: GitPlan,
  operationId: string,
  submissionKey: string
): void {
  const conflict = source.get<{ worktree_id: string }>(
    `SELECT worktrees.id AS worktree_id
     FROM worktrees
     LEFT JOIN session_fork_intents AS fork ON fork.worktree_id = worktrees.id
     WHERE worktrees.repository_root = ? AND worktrees.branch_name = ?
       AND (
         worktrees.state <> 'removed' OR fork.stage NOT IN ('succeeded', 'failed')
       )
       AND NOT (
         COALESCE(fork.operation_id, '') = ? AND COALESCE(fork.submission_key, '') = ?
       )
     LIMIT 1`,
    gitPlan.repositoryRoot, gitPlan.branch, operationId, submissionKey
  )
  if (conflict) {
    throw new ForkWorkflowError(
      'BRANCH_CONFLICT', `分支 ${gitPlan.branch} 已被其他 Fork 预留`, gitPlan.branch
    )
  }
}

function processExitCode(error: unknown): number | undefined {
  if (typeof error !== 'object' || error === null || !('code' in error)) return undefined
  return typeof error.code === 'number' ? error.code : undefined
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
