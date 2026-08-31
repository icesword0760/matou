import { execFile } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { realpath } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'

import type {
  DomainCommandMetadata,
  SceneSessionGraph,
  Session,
  Worktree
} from '@matou/domain'

import {
  activateSessionInTransaction,
  assertWorkspacePathAvailable,
  readHierarchyResult,
  registerWindow,
  type WorkspaceHierarchyResult
} from '../hierarchy/hierarchy-application-service'
import { createHierarchyIds } from '../hierarchy/hierarchy-ids'
import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainMutationContext, DomainTransactionManager } from '../storage/domain-transaction'
import { WorktreeService, type WorktreeSetupStep } from '../worktrees/worktree-service'
import { createGitBranchName, validateDisplayName } from './branch-name'
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

export interface CreateForkInput {
  windowId: string
  sceneId: string
  sourceSessionId: string
  name: string
  worktreeMode: ForkWorktreeMode
  now: number
}

export interface ForkWorkflowResult extends WorkspaceHierarchyResult {
  graph: SceneSessionGraph
  forkState: 'pending' | 'starting' | 'succeeded' | 'failed'
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
  path: string
  branch: string
  worktreeId: string
  executionContextId: string
}

interface ForkIntentRow {
  session_id: string
  source_session_id: string
  state: 'pending' | 'starting' | 'succeeded' | 'failed'
  worktree_mode: ForkWorktreeMode
  worktree_id: string | null
}

export class ForkWorkflowService {
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager
  readonly #worktrees: WorktreeService
  readonly #setupPolicyForWorkspace: (workspaceId: string) => WorktreeSetupStep[]

  constructor(
    dataRoot: string,
    database: RuntimeDatabase,
    transactions: DomainTransactionManager,
    dependencies: {
      stopRuns: (runIds: string[]) => Promise<void>
      setupPolicyForWorkspace?: (workspaceId: string) => WorktreeSetupStep[]
    }
  ) {
    this.#dataRoot = dataRoot
    this.#database = database
    this.#transactions = transactions
    this.#worktrees = new WorktreeService(database, transactions, dependencies)
    this.#setupPolicyForWorkspace = dependencies.setupPolicyForWorkspace ?? (() => [])
  }

  createForkChild(command: DomainCommandMetadata, input: CreateForkInput): Promise<ForkWorkflowResult> {
    return this.#create(command, input, 'child')
  }

  createForkSibling(command: DomainCommandMetadata, input: CreateForkInput): Promise<ForkWorkflowResult> {
    return this.#create(command, input, 'sibling')
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
      tx.run(
        `UPDATE session_fork_intents SET state = 'pending', error_message = NULL,
           completed_at = NULL, attempt_count = attempt_count + 1, updated_at = ?
         WHERE session_id = ?`,
        input.now, input.sessionId
      )
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
      return { ...result, forkState: 'pending' as const }
    }).result
    if (intent.worktree_mode === 'current') return prepared

    const worktree = intent.worktree_id ? this.#worktrees.get(intent.worktree_id) : undefined
    if (!worktree) {
      return this.#markFailed(
        derivedCommand(command, 'missing-worktree'), input, input.sessionId,
        '分支工作树记录缺失'
      )
    }
    if (worktree.state === 'ready') {
      return this.#bindReadyWorktree(
        derivedCommand(command, 'bind-existing-worktree'), input, input.sessionId, worktree
      )
    }
    try {
      const ready = await this.#worktrees.create(derivedCommand(command, 'worktree'), {
        id: worktree.id,
        executionContextId: worktree.executionContextId,
        workspaceId: owner.workspace_id,
        repositoryRoot: worktree.repositoryRoot,
        path: worktree.path,
        branch: worktree.branch,
        baseRef: worktree.baseRevision ?? 'HEAD',
        setupPolicy: worktree.setupPolicy as WorktreeSetupStep[],
        now: input.now
      })
      return this.#bindReadyWorktree(
        derivedCommand(command, 'bind-worktree'), input, input.sessionId, ready
      )
    } catch (error) {
      return this.#markFailed(
        derivedCommand(command, 'worktree-failed'), input, input.sessionId, errorMessage(error)
      )
    }
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

  async #create(
    command: DomainCommandMetadata,
    input: CreateForkInput,
    placement: 'child' | 'sibling'
  ): Promise<ForkWorkflowResult> {
    const ids = createHierarchyIds()
    const relationId = randomUUID()
    const source = this.#resolveSource(input, placement)
    const activeNames = this.#activeChildNames(source.forkSource.id)
    const name = validateDisplayName(input.name, activeNames)
    if (!name.ok) throw displayNameError(name.code, name.message, name.input)

    const gitPlan = input.worktreeMode === 'new'
      ? await this.#resolveGitPlan(source, name.displayName, ids.sessionId)
      : undefined
    const initial = this.#createPreparingNode(
      command, input, source, name.displayName, ids, relationId, gitPlan,
      placement === 'sibling' ? source.selected.id : undefined
    )
    if (!gitPlan) return initial

    try {
      const worktree = await this.#worktrees.create(derivedCommand(command, 'worktree'), {
        id: gitPlan.worktreeId,
        executionContextId: gitPlan.executionContextId,
        workspaceId: source.task.workspace_id,
        repositoryRoot: gitPlan.repositoryRoot,
        path: gitPlan.path,
        branch: gitPlan.branch,
        baseRef: gitPlan.baseRef,
        setupPolicy: this.#setupPolicyForWorkspace(source.task.workspace_id),
        now: input.now
      })
      return this.#bindReadyWorktree(
        derivedCommand(command, 'bind-worktree'), input, ids.sessionId, worktree,
        placement === 'sibling' ? source.selected.id : undefined
      )
    } catch (error) {
      return this.#markFailed(
        derivedCommand(command, 'worktree-failed'), input, ids.sessionId, errorMessage(error),
        placement === 'sibling' ? source.selected.id : undefined
      )
    }
  }

  #resolveSource(input: CreateForkInput, placement: 'child' | 'sibling'): SourceContext {
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
    if (source.kind !== 'claude-code') {
      throw new ForkWorkflowError(
        'FORK_SOURCE_NOT_READY', '完成首轮 Claude Code 对话后可创建分支'
      )
    }
    const binding = this.#database.get<BindingRow>(
      `SELECT provider_session_id, metadata_json, restore_state
       FROM provider_bindings
       WHERE session_id = ? AND provider = 'claude-code'
         AND resume_state IN ('available', 'resumed')
         AND validated_at IS NOT NULL AND invalidated_at IS NULL
       ORDER BY updated_at DESC, id DESC LIMIT 1`,
      source.id
    )
    if (!binding || binding.restore_state !== 'none' || metadata(binding.metadata_json).canFork !== true) {
      throw new ForkWorkflowError(
        'FORK_SOURCE_NOT_READY', '完成首轮 Claude Code 对话后可创建分支'
      )
    }
    return binding
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

  async #resolveGitPlan(
    source: SourceContext,
    displayName: string,
    sessionId: string
  ): Promise<GitPlan> {
    try {
      const repositoryRoot = (await exec(
        'git', ['-C', source.forkSource.cwd, 'rev-parse', '--show-toplevel']
      )).stdout.trim()
      const baseRef = (await exec('git', ['-C', repositoryRoot, 'rev-parse', 'HEAD'])).stdout.trim()
      const canonicalDataRoot = await realpath(this.#dataRoot).catch(() => this.#dataRoot)
      return {
        repositoryRoot,
        baseRef,
        path: join(canonicalDataRoot, 'worktrees', source.task.workspace_id, sessionId),
        branch: createGitBranchName(displayName, sessionId),
        worktreeId: randomUUID(),
        executionContextId: randomUUID()
      }
    } catch {
      throw new ForkWorkflowError('GIT_REPOSITORY_REQUIRED', '新工作树需要 Git 仓库')
    }
  }

  #createPreparingNode(
    command: DomainCommandMetadata,
    input: CreateForkInput,
    source: SourceContext,
    displayName: string,
    ids: ReturnType<typeof createHierarchyIds>,
    relationId: string,
    gitPlan: GitPlan | undefined,
    preserveFocusedSessionId?: string
  ): ForkWorkflowResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      registerWindow(tx, input.windowId, input.now)
      const sourceNode = requireRow(tx.get<SceneNodeRow>(
        'SELECT id, parent_node_id, ordinal FROM scene_nodes WHERE id = ?',
        source.selectedMount.scene_node_id!
      ), 'SceneNode')
      insertHorizontalMount(tx, input.sceneId, sourceNode, ids, input.now)
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, title, cwd,
           created_at, updated_at, last_activity_at, version
         ) VALUES (?, ?, ?, 'claude-code', 'starting', ?, ?, ?, ?, ?, 1)`,
        ids.sessionId, source.task.id, source.forkSource.execution_context_id,
        displayName, source.forkSource.cwd, input.now, input.now, input.now
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
      tx.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, error_message, created_at, display_name, worktree_mode,
           worktree_id, target_execution_context_id, worktree_path, branch_name,
           attempt_count, updated_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', NULL, ?, ?, ?, ?, ?, ?, ?, 0, ?)`,
        ids.sessionId, source.forkSource.id, source.binding.provider_session_id,
        input.now, displayName, input.worktreeMode,
        gitPlan?.worktreeId ?? null, gitPlan?.executionContextId ?? null,
        gitPlan?.path ?? null, gitPlan?.branch ?? null, input.now
      )
      const relationInsertion = tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id, to_session_id,
           relation_kind, metadata_json, command_id, occurred_at
         ) VALUES (?, ?, 'created', ?, ?, ?, 'forked-from', ?, ?, ?)`,
        `${command.commandId}:fork-relation-created`, relationId, source.task.id,
        ids.sessionId, source.forkSource.id,
        JSON.stringify({ worktreeMode: input.worktreeMode }), command.commandId, input.now
      )
      tx.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           metadata_json, created_at, updated_at, source_event_sequence
         ) VALUES (?, ?, ?, ?, 'forked-from', ?, ?, ?, ?)`,
        relationId, source.task.id, ids.sessionId, source.forkSource.id,
        JSON.stringify({ worktreeMode: input.worktreeMode }), input.now, input.now,
        Number(relationInsertion.lastInsertRowid)
      )
      const result = readCreatedForkResult(
        tx, input.windowId, input.sceneId, ids.sessionId, input.now, preserveFocusedSessionId
      )
      emitForkEvents(emit, command.commandId, source, result, relationId, input.now)
      return { ...result, forkState: 'pending' as const }
    }).result
  }

  #bindReadyWorktree(
    command: DomainCommandMetadata,
    input: MutationLocation,
    sessionId: string,
    worktree: Worktree,
    preserveFocusedSessionId?: string
  ): ForkWorkflowResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      tx.run(
        `UPDATE sessions SET execution_context_id = ?, cwd = ?, updated_at = ?,
           version = version + 1 WHERE id = ?`,
        worktree.executionContextId, worktree.path, input.now, sessionId
      )
      tx.run(
        `UPDATE session_fork_intents SET worktree_id = ?, target_execution_context_id = ?,
           worktree_path = ?, branch_name = ?, updated_at = ? WHERE session_id = ?`,
        worktree.id, worktree.executionContextId, worktree.path, worktree.branch,
        input.now, sessionId
      )
      const result = readCreatedForkResult(
        tx, input.windowId, input.sceneId, sessionId, input.now, preserveFocusedSessionId
      )
      emit({
        eventId: `${command.commandId}:worktree-ready`,
        eventType: 'session.fork-worktree-ready',
        aggregateType: 'session', aggregateId: sessionId,
        taskId: result.session!.taskId, sessionId,
        payload: { session: result.session, worktree, graph: result.graph },
        occurredAt: input.now
      })
      return { ...result, forkState: 'pending' as const, worktree }
    }).result
  }

  #markFailed(
    command: DomainCommandMetadata,
    input: MutationLocation,
    sessionId: string,
    reason: string,
    preserveFocusedSessionId?: string
  ): ForkWorkflowResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      tx.run(
        `UPDATE session_fork_intents SET state = 'failed', error_message = ?,
           completed_at = ?, updated_at = ? WHERE session_id = ?`,
        reason, input.now, input.now, sessionId
      )
      tx.run(
        `UPDATE sessions SET status = 'interrupted', updated_at = ?,
           version = version + 1 WHERE id = ?`,
        input.now, sessionId
      )
      const result = readCreatedForkResult(
        tx, input.windowId, input.sceneId, sessionId, input.now, preserveFocusedSessionId
      )
      emit({
        eventId: `${command.commandId}:fork-failed`, eventType: 'session.fork-failed',
        aggregateType: 'session', aggregateId: sessionId,
        taskId: result.session!.taskId, sessionId,
        payload: { error: reason, graph: result.graph }, occurredAt: input.now
      })
      return { ...result, forkState: 'failed' as const, error: reason }
    }).result
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

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
