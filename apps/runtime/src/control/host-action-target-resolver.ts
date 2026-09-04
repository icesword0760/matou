import { spawnSync } from 'node:child_process'

import type {
  ForkEnvironmentChoice,
  HostActionErrorCode,
  HostActionTargetSelector,
  HostEntitySelector,
  HostImpactSummary,
  HostResultPath
} from './host-action-types'
import type { HostCallerIdentity, HostListScope, HostTarget } from './host-control-types'
import { HostTopologyProjector } from './host-topology-projector'
import { hostTargetRevision } from './host-target-revision'
import type { RuntimeDatabase } from '../storage/database'

export interface ResolvedHierarchyPath {
  windowId: string
  workspaceId: string
  taskId: string
  sceneId: string
  sessionId?: string
}

export type ResolvedHostEntity =
  | ({ kind: 'workspace'; workspaceId: string } & ResolvedHierarchyPath)
  | ({ kind: 'task'; taskId: string } & ResolvedHierarchyPath)
  | ({ kind: 'canvas'; sceneId: string } & ResolvedHierarchyPath)
  | ({ kind: 'session'; sessionId: string; mountId?: string } & ResolvedHierarchyPath)

export interface RemovalImpact {
  target: ResolvedHostEntity
  scope: 'node' | 'subtree'
  tasks: number
  canvases: number
  sessions: number
  descendants: number
  liveRuns: number
  terminalProcesses: number
  preservesProjectFiles: true
  preservesBranches: true
  preservesWorktrees: true
}

/** A submitted Fork choice after its stable environment reference has been verified. */
export type ResolvedForkEnvironment =
  | { mode: 'current'; executionContextId: string }
  | { mode: 'existing-worktree'; executionContextId: string; worktreeId: string;
      worktreeRef: string; branch: string }
  | { mode: 'new-worktree'; branch: string }

export interface HostActionTargetCandidate {
  ref: string
  path: HostResultPath
  displayPath: string
}

/**
 * The facade maps these stable product faults onto its Host Control response.
 * Candidates deliberately carry only the human-readable hierarchy path.
 */
export class HostActionTargetResolverError extends Error {
  readonly code: HostActionErrorCode
  readonly candidates: readonly HostActionTargetCandidate[]

  constructor(
    code: HostActionErrorCode,
    message: string,
    candidates: readonly HostActionTargetCandidate[] = []
  ) {
    super(message)
    this.name = 'HostActionTargetResolverError'
    this.code = code
    this.candidates = candidates
  }
}

type ResolverSelector = HostEntitySelector

type ForkEnvironmentSource = HostTarget | (ResolvedHostEntity & { kind: 'session' })

interface EntityRow {
  window_id: string | null
  workspace_id: string
  task_id: string
  scene_id: string
  session_id: string | null
  mount_id: string | null
}

interface GitContextRow {
  id: string
  repository_root: string
}

interface WorktreeRow {
  id: string
  execution_context_id: string
  branch_name: string
  state: 'creating' | 'ready' | 'dirty' | 'retained' | 'removing' | 'removed' | 'failed'
  git_state: 'ready' | 'unavailable' | null
  git_branch: string | null
}

interface ImpactCountsRow {
  live_runs: number
  terminal_processes: number
}

interface ResultPathRow {
  window_kind: 'main' | 'detached-terminal' | null
  workspace_id: string
  workspace_name: string
  workspace_path: string
  task_id: string
  task_title: string
  scene_id: string
  scene_name: string
  session_id: string | null
  session_title: string | null
}

/**
 * Resolves every Host action target against Runtime's authoritative hierarchy.
 * It never writes; callers perform all mutation only after this preflight succeeds.
 */
export class HostActionTargetResolver {
  readonly #database: RuntimeDatabase
  readonly #topology: HostTopologyProjector

  constructor(database: RuntimeDatabase, topology = new HostTopologyProjector(database)) {
    this.#database = database
    this.#topology = topology
  }

  /** Matches the Host Control projection hash for the selected list scope. */
  projectionRevision(caller: HostCallerIdentity, scope: HostListScope = 'all'): string {
    return hostTargetRevision(this.#topology.list(caller, scope))
  }

  resolveEntity(
    caller: HostCallerIdentity,
    selector: ResolverSelector,
    expectedRevision: string
  ): ResolvedHostEntity {
    this.#assertFreshRevision(caller, selector, expectedRevision)

    if (selector.kind === 'current') {
      const current = this.#currentTarget(caller)
      if (selector.entity === 'workspace') return this.#resolveWorkspace(current.workspaceId)
      if (selector.entity === 'task') return this.#resolveTask(current.taskId)
      if (selector.entity === 'canvas') return this.#resolveCanvas(current.canvas.id)
      return this.#resolveSession(current.sessionId)
    }
    if (selector.kind === 'ref') return this.#resolveStableRef(caller, selector.ref)
    if (selector.kind === 'session') return this.#resolveSession(selector.sessionId)

    const sessionId = this.#relativeSessionId(caller, selector)
    return this.#resolveSession(sessionId)
  }

  resolveForkEnvironment(
    source: ForkEnvironmentSource,
    choice: ForkEnvironmentChoice
  ): ResolvedForkEnvironment {
    const sourceContextId = this.#sourceExecutionContextId(source)
    if (choice.mode === 'current') {
      return { mode: 'current', executionContextId: sourceContextId }
    }

    const sourceGit = this.#gitContext(sourceContextId)
    if (!sourceGit) {
      throw new HostActionTargetResolverError(
        'WORKTREE_CONFLICT',
        '普通目录只能继续使用当前执行环境'
      )
    }

    if (choice.mode === 'existing-worktree') {
      const worktreeId = parseStableRef(choice.worktreeRef, 'worktree')
      if (!worktreeId) {
        throw new HostActionTargetResolverError('WORKTREE_CONFLICT', '提交的 Worktree 引用无效')
      }
      const worktree = this.#database.get<WorktreeRow>(
        `SELECT worktrees.id, worktrees.execution_context_id, worktrees.branch_name,
                worktrees.state, git_state.state AS git_state, git_state.branch AS git_branch
         FROM worktrees
         JOIN execution_contexts ON execution_contexts.id = worktrees.execution_context_id
         LEFT JOIN execution_context_git_states AS git_state
           ON git_state.execution_context_id = worktrees.execution_context_id
         WHERE worktrees.id = ? AND execution_contexts.archived_at IS NULL`,
        worktreeId
      )
      if (!worktree || !isReusableWorktreeState(worktree.state)) {
        throw new HostActionTargetResolverError('WORKTREE_CONFLICT', '指定的 Worktree 已不可用')
      }
      if (
        worktree.branch_name !== choice.branch ||
        worktree.git_state !== 'ready' ||
        worktree.git_branch !== choice.branch
      ) {
        throw new HostActionTargetResolverError(
          'BRANCH_CONFLICT',
          `Worktree 当前分支与提交的 ${choice.branch} 不一致`
        )
      }
      return {
        mode: 'existing-worktree',
        executionContextId: worktree.execution_context_id,
        worktreeId: worktree.id,
        worktreeRef: `worktree:${worktree.id}`,
        branch: choice.branch
      }
    }

    if (this.#localBranchExists(sourceGit.repository_root, choice.branch)) {
      throw new HostActionTargetResolverError(
        'BRANCH_CONFLICT',
        `分支 ${choice.branch} 已存在`
      )
    }
    return { mode: 'new-worktree', branch: choice.branch }
  }

  previewRemoval(target: ResolvedHostEntity, scope: 'node' | 'subtree'): RemovalImpact {
    const authoritativeTarget = this.#refreshEntity(target)
    const sessionIds = this.#affectedSessionIds(authoritativeTarget, scope)
    const counts = this.#runCounts(sessionIds)
    const tasks = authoritativeTarget.kind === 'workspace'
      ? this.#count('SELECT COUNT(*) AS count FROM tasks WHERE workspace_id = ? AND archived_at IS NULL', authoritativeTarget.workspaceId)
      : authoritativeTarget.kind === 'task' ? 1 : 0
    const canvases = authoritativeTarget.kind === 'workspace'
      ? this.#count(
        `SELECT COUNT(*) AS count FROM scenes
         JOIN tasks ON tasks.id = scenes.task_id
         WHERE tasks.workspace_id = ? AND tasks.archived_at IS NULL AND scenes.archived_at IS NULL`,
        authoritativeTarget.workspaceId
      )
      : authoritativeTarget.kind === 'task'
        ? this.#count('SELECT COUNT(*) AS count FROM scenes WHERE task_id = ? AND archived_at IS NULL', authoritativeTarget.taskId)
        : authoritativeTarget.kind === 'canvas' ? 1 : 0
    const descendants = authoritativeTarget.kind === 'session'
      ? Math.max(0, sessionIds.length - 1)
      : authoritativeTarget.kind === 'canvas'
        ? sessionIds.length
        : authoritativeTarget.kind === 'task'
          ? canvases + sessionIds.length
          : tasks + canvases + sessionIds.length

    return {
      target: authoritativeTarget,
      scope,
      tasks,
      canvases,
      sessions: sessionIds.length,
      descendants,
      liveRuns: counts.live_runs,
      terminalProcesses: counts.terminal_processes,
      preservesProjectFiles: true,
      preservesBranches: true,
      preservesWorktrees: true
    }
  }

  /** Converts the internal ID-bearing snapshot into the product-facing impact contract. */
  toHostImpactSummary(impact: RemovalImpact): HostImpactSummary {
    return {
      target: this.#resultPath(impact.target),
      scope: impact.scope,
      tasks: impact.tasks,
      canvases: impact.canvases,
      sessions: impact.sessions,
      descendants: impact.descendants,
      liveRuns: impact.liveRuns,
      terminalProcesses: impact.terminalProcesses,
      preservesProjectFiles: true,
      preservesBranches: true,
      preservesWorktrees: true
    }
  }

  #assertFreshRevision(
    caller: HostCallerIdentity,
    selector: ResolverSelector,
    expectedRevision: string
  ): void {
    if (selector.kind === 'current' || selector.kind === 'self' || selector.kind === 'session') return
    const scope: HostListScope = selector.kind === 'ref' ? 'all' : 'current-level'
    const currentRevision = this.projectionRevision(caller, scope)
    if (expectedRevision !== currentRevision || selector.projectionRevision !== expectedRevision) {
      throw new HostActionTargetResolverError(
        'STALE_PROJECTION',
        '目标列表已更新，请重新列举后再执行'
      )
    }
  }

  #resolveStableRef(caller: HostCallerIdentity, ref: string): ResolvedHostEntity {
    const workspaceId = parseStableRef(ref, 'workspace')
    if (workspaceId) return this.#resolveWorkspace(workspaceId)
    const taskId = parseStableRef(ref, 'task')
    if (taskId) return this.#resolveTask(taskId)
    const sceneId = parseStableRef(ref, 'scene') ?? parseStableRef(ref, 'canvas')
    if (sceneId) return this.#resolveCanvas(sceneId)
    const sessionId = parseStableRef(ref, 'session')
    if (sessionId) return this.#resolveSession(sessionId)

    // Current topology refs are session refs. Keep this fallback so an older caller
    // receives the same stable not-found/ambiguity result instead of a generic parse fault.
    const matches = this.#topology.list(caller, 'all').filter((target) => target.ref === ref)
    if (matches.length === 1) return this.#resolveSession(matches[0]!.sessionId)
    if (matches.length > 1) {
      const candidates = matches
        .slice()
        .sort(compareProjectedTarget)
        .map((target) => this.#candidateForTarget(target))
      throw new HostActionTargetResolverError(
        'AMBIGUOUS_TARGET',
        `目标 ${ref} 匹配多个层级位置`,
        candidates
      )
    }
    throw new HostActionTargetResolverError('TARGET_NOT_FOUND', `目标 ${ref} 不存在`)
  }

  #resolveWorkspace(workspaceId: string): ResolvedHostEntity {
    const row = this.#database.get<EntityRow>(
      `SELECT placement.window_id, workspaces.id AS workspace_id, tasks.id AS task_id,
              scenes.id AS scene_id, NULL AS session_id, NULL AS mount_id
       FROM workspaces
       JOIN tasks ON tasks.id = (
         SELECT candidate.id FROM tasks AS candidate
         LEFT JOIN window_task_placements AS candidate_placement
           ON candidate_placement.task_id = candidate.id
         WHERE candidate.workspace_id = workspaces.id AND candidate.archived_at IS NULL
         ORDER BY candidate_placement.ordinal, candidate.sort_key, candidate.created_at, candidate.id
         LIMIT 1
       )
       JOIN scenes ON scenes.id = (
         SELECT candidate.id FROM scenes AS candidate
         WHERE candidate.task_id = tasks.id AND candidate.archived_at IS NULL
         ORDER BY candidate.sort_key, candidate.created_at, candidate.id
         LIMIT 1
       )
       LEFT JOIN window_task_placements AS placement ON placement.task_id = tasks.id
       WHERE workspaces.id = ? AND workspaces.archived_at IS NULL`,
      workspaceId
    )
    return this.#entityFromRow('workspace', row, workspaceId)
  }

  #resolveTask(taskId: string): ResolvedHostEntity {
    const row = this.#database.get<EntityRow>(
      `SELECT placement.window_id, workspaces.id AS workspace_id, tasks.id AS task_id,
              scenes.id AS scene_id, NULL AS session_id, NULL AS mount_id
       FROM tasks
       JOIN workspaces ON workspaces.id = tasks.workspace_id AND workspaces.archived_at IS NULL
       JOIN scenes ON scenes.id = (
         SELECT candidate.id FROM scenes AS candidate
         WHERE candidate.task_id = tasks.id AND candidate.archived_at IS NULL
         ORDER BY candidate.sort_key, candidate.created_at, candidate.id
         LIMIT 1
       )
       LEFT JOIN window_task_placements AS placement ON placement.task_id = tasks.id
       WHERE tasks.id = ? AND tasks.archived_at IS NULL`,
      taskId
    )
    return this.#entityFromRow('task', row, taskId)
  }

  #resolveCanvas(sceneId: string): ResolvedHostEntity {
    const row = this.#database.get<EntityRow>(
      `SELECT placement.window_id, workspaces.id AS workspace_id, tasks.id AS task_id,
              scenes.id AS scene_id, NULL AS session_id, NULL AS mount_id
       FROM scenes
       JOIN tasks ON tasks.id = scenes.task_id AND tasks.archived_at IS NULL
       JOIN workspaces ON workspaces.id = tasks.workspace_id AND workspaces.archived_at IS NULL
       LEFT JOIN window_task_placements AS placement ON placement.task_id = tasks.id
       WHERE scenes.id = ? AND scenes.archived_at IS NULL`,
      sceneId
    )
    return this.#entityFromRow('canvas', row, sceneId)
  }

  #resolveSession(sessionId: string): ResolvedHostEntity {
    const row = this.#database.get<EntityRow>(
      `SELECT COALESCE(detached.native_window_key, placement.window_id) AS window_id,
              workspaces.id AS workspace_id, tasks.id AS task_id, scenes.id AS scene_id,
              sessions.id AS session_id, mounts.id AS mount_id
       FROM sessions
       JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
       JOIN scenes ON scenes.id = membership.scene_id AND scenes.archived_at IS NULL
       JOIN tasks ON tasks.id = scenes.task_id AND tasks.archived_at IS NULL
       JOIN workspaces ON workspaces.id = tasks.workspace_id AND workspaces.archived_at IS NULL
       LEFT JOIN session_mounts AS mounts ON mounts.id = (
         SELECT candidate.id FROM session_mounts AS candidate
         WHERE candidate.scene_id = scenes.id AND candidate.session_id = sessions.id
         ORDER BY candidate.created_at, candidate.id LIMIT 1
       )
       LEFT JOIN scene_windows AS detached
         ON detached.id = mounts.scene_window_id AND detached.state = 'detached'
       LEFT JOIN window_task_placements AS placement ON placement.task_id = tasks.id
       WHERE sessions.id = ? AND sessions.archived_at IS NULL`,
      sessionId
    )
    return this.#entityFromRow('session', row, sessionId)
  }

  #entityFromRow(
    kind: ResolvedHostEntity['kind'],
    row: EntityRow | undefined,
    id: string
  ): ResolvedHostEntity {
    if (!row) throw new HostActionTargetResolverError('TARGET_NOT_FOUND', `目标 ${id} 不存在`)
    const path = {
      windowId: row.window_id ?? `unplaced:${row.task_id}`,
      workspaceId: row.workspace_id,
      taskId: row.task_id,
      sceneId: row.scene_id
    }
    if (kind === 'workspace') return { kind, ...path }
    if (kind === 'task') return { kind, ...path }
    if (kind === 'canvas') return { kind, ...path }
    return {
      kind,
      ...path,
      sessionId: row.session_id!,
      ...(row.mount_id === null ? {} : { mountId: row.mount_id })
    }
  }

  #currentTarget(caller: HostCallerIdentity): HostTarget {
    try {
      return this.#topology.identify(caller).target
    } catch (error) {
      throw new HostActionTargetResolverError(
        'TARGET_NOT_FOUND',
        error instanceof Error ? error.message : '当前调用会话不存在'
      )
    }
  }

  #relativeSessionId(caller: HostCallerIdentity, selector: Exclude<HostActionTargetSelector, { kind: 'ref' | 'session' }>): string {
    try {
      return this.#topology.resolve(caller, selector)
    } catch (error) {
      throw new HostActionTargetResolverError(
        'TARGET_NOT_FOUND',
        error instanceof Error ? error.message : '目标会话不存在'
      )
    }
  }

  #sourceExecutionContextId(source: ForkEnvironmentSource): string {
    if ('environment' in source) {
      const contextId = parseStableRef(source.environment.executionContextRef, 'context')
      if (!contextId) {
        throw new HostActionTargetResolverError('TARGET_NOT_FOUND', '来源执行环境引用无效')
      }
      return contextId
    }
    const row = this.#database.get<{ execution_context_id: string }>(
      'SELECT execution_context_id FROM sessions WHERE id = ? AND archived_at IS NULL',
      source.sessionId
    )
    if (!row) throw new HostActionTargetResolverError('TARGET_NOT_FOUND', '来源会话不存在')
    return row.execution_context_id
  }

  #gitContext(executionContextId: string): GitContextRow | undefined {
    return this.#database.get<GitContextRow>(
      `SELECT execution_contexts.id, git_state.repository_root
       FROM execution_contexts
       JOIN execution_context_git_states AS git_state
         ON git_state.execution_context_id = execution_contexts.id
       WHERE execution_contexts.id = ? AND execution_contexts.archived_at IS NULL
         AND git_state.state = 'ready' AND git_state.repository_root IS NOT NULL`,
      executionContextId
    )
  }

  #localBranchExists(repositoryRoot: string, branch: string): boolean {
    const result = spawnSync(
      'git',
      ['-C', repositoryRoot, 'show-ref', '--verify', '--quiet', `refs/heads/${branch}`],
      { stdio: 'ignore' }
    )
    if (result.error || (result.status !== 0 && result.status !== 1)) {
      throw new HostActionTargetResolverError(
        'WORKTREE_CONFLICT',
        `仓库分支状态校验失败: ${branch}`
      )
    }
    return result.status === 0
  }

  #refreshEntity(target: ResolvedHostEntity): ResolvedHostEntity {
    if (target.kind === 'workspace') return this.#resolveWorkspace(target.workspaceId)
    if (target.kind === 'task') return this.#resolveTask(target.taskId)
    if (target.kind === 'canvas') return this.#resolveCanvas(target.sceneId)
    return this.#resolveSession(target.sessionId)
  }

  #affectedSessionIds(target: ResolvedHostEntity, scope: 'node' | 'subtree'): string[] {
    if (target.kind === 'workspace') {
      return this.#database.all<{ id: string }>(
        `SELECT sessions.id FROM sessions
         JOIN tasks ON tasks.id = sessions.task_id
         WHERE tasks.workspace_id = ? AND tasks.archived_at IS NULL AND sessions.archived_at IS NULL
         ORDER BY sessions.created_at, sessions.id`,
        target.workspaceId
      ).map(({ id }) => id)
    }
    if (target.kind === 'task') {
      return this.#database.all<{ id: string }>(
        'SELECT id FROM sessions WHERE task_id = ? AND archived_at IS NULL ORDER BY created_at, id',
        target.taskId
      ).map(({ id }) => id)
    }
    if (target.kind === 'canvas') {
      return this.#database.all<{ id: string }>(
        `SELECT sessions.id FROM sessions
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         WHERE membership.scene_id = ? AND sessions.archived_at IS NULL
         ORDER BY membership.sibling_created_seq, sessions.id`,
        target.sceneId
      ).map(({ id }) => id)
    }
    if (scope === 'node') return [target.sessionId]
    return this.#database.all<{ id: string }>(
      `WITH RECURSIVE branch(session_id) AS (
         SELECT ?
         UNION
         SELECT relation.from_session_id
         FROM session_relations_current AS relation
         JOIN branch ON relation.to_session_id = branch.session_id
         JOIN session_canvas_memberships AS membership ON membership.session_id = relation.from_session_id
         JOIN sessions ON sessions.id = relation.from_session_id
         WHERE membership.scene_id = ? AND sessions.archived_at IS NULL
           AND relation.relation_kind IN ('derived-from', 'forked-from')
       )
       SELECT branch.session_id AS id
       FROM branch
       JOIN session_canvas_memberships AS membership ON membership.session_id = branch.session_id
       ORDER BY CASE WHEN branch.session_id = ? THEN 0 ELSE 1 END,
                membership.sibling_created_seq, branch.session_id`,
      target.sessionId, target.sceneId, target.sessionId
    ).map(({ id }) => id)
  }

  #runCounts(sessionIds: string[]): ImpactCountsRow {
    if (sessionIds.length === 0) return { live_runs: 0, terminal_processes: 0 }
    const placeholders = sessionIds.map(() => '?').join(', ')
    return this.#database.get<ImpactCountsRow>(
      `SELECT
         COUNT(DISTINCT CASE WHEN status IN ('starting', 'running') THEN id END) AS live_runs,
         COUNT(DISTINCT CASE WHEN status IN ('starting', 'running') AND pid IS NOT NULL THEN id END)
           AS terminal_processes
       FROM session_runs WHERE session_id IN (${placeholders})`,
      ...sessionIds
    ) ?? { live_runs: 0, terminal_processes: 0 }
  }

  #count(sql: string, ...params: string[]): number {
    return this.#database.get<{ count: number }>(sql, ...params)?.count ?? 0
  }

  #resultPath(target: ResolvedHostEntity): HostResultPath {
    const row = this.#database.get<ResultPathRow>(
      `SELECT app_windows.kind AS window_kind,
              workspaces.id AS workspace_id, workspaces.name AS workspace_name,
              workspaces.root_directory AS workspace_path,
              tasks.id AS task_id, tasks.title AS task_title,
              scenes.id AS scene_id, scenes.name AS scene_name,
              sessions.id AS session_id, sessions.title AS session_title
       FROM workspaces
       JOIN tasks ON tasks.id = ? AND tasks.workspace_id = workspaces.id
       JOIN scenes ON scenes.id = ? AND scenes.task_id = tasks.id
       LEFT JOIN sessions ON sessions.id = ?
       LEFT JOIN app_windows ON app_windows.id = ?
       WHERE workspaces.id = ?`,
      target.taskId,
      target.sceneId,
      target.kind === 'session' ? target.sessionId : null,
      target.windowId,
      target.workspaceId
    )
    if (!row) throw new HostActionTargetResolverError('TARGET_NOT_FOUND', '影响目标已不存在')
    return {
      window: {
        ref: `window:${target.windowId}`,
        title: windowTitle(row.window_kind)
      },
      workspace: {
        ref: `workspace:${row.workspace_id}`,
        title: row.workspace_name,
        path: row.workspace_path
      },
      task: { ref: `task:${row.task_id}`, title: row.task_title },
      canvas: { ref: `scene:${row.scene_id}`, title: row.scene_name },
      ...(row.session_id === null
        ? {}
        : { session: { ref: `session:${row.session_id}`, title: row.session_title! } })
    }
  }

  #candidateForTarget(target: HostTarget): HostActionTargetCandidate {
    const entity = this.#resolveSession(target.sessionId)
    const path = this.#resultPath(entity)
    return { ref: target.ref, path, displayPath: displayPath(path) }
  }
}

function parseStableRef(ref: string, kind: string): string | undefined {
  const prefix = `${kind}:`
  if (!ref.startsWith(prefix)) return undefined
  const id = ref.slice(prefix.length)
  return id.length > 0 ? id : undefined
}

function isReusableWorktreeState(state: WorktreeRow['state']): boolean {
  return state === 'ready' || state === 'dirty' || state === 'retained'
}

function compareProjectedTarget(left: HostTarget, right: HostTarget): number {
  return left.window.ordinal - right.window.ordinal ||
    left.workspace.ordinal - right.workspace.ordinal ||
    left.task.ordinal - right.task.ordinal ||
    left.canvas.ordinal - right.canvas.ordinal ||
    left.session.ordinal - right.session.ordinal ||
    left.sessionId.localeCompare(right.sessionId)
}

function windowTitle(kind: ResultPathRow['window_kind']): string {
  return kind === 'detached-terminal' ? '独立终端窗口' : '主窗口'
}

function displayPath(path: HostResultPath): string {
  return [
    path.workspace.title,
    path.task?.title,
    path.canvas?.title,
    path.session?.title
  ].filter((value): value is string => Boolean(value)).join(' / ')
}
