import type { RuntimeDatabase } from '../storage/database'
import {
  HostControlTargetNotFoundError,
  type HostCallerIdentity,
  type HostListScope,
  type HostTarget,
  type HostTargetEnvironment,
  type HostTargetSelector
} from './host-control-types'

interface TargetRow {
  session_id: string
  execution_context_id: string
  execution_context_kind: 'plain-directory' | 'git-worktree'
  git_state: 'ready' | 'unavailable' | null
  git_branch: string | null
  worktree_id: string | null
  worktree_branch_name: string | null
  title: string
  kind: 'shell' | 'claude-code' | 'codex' | 'agent-team-member'
  cwd: string
  work_status: string
  workspace_id: string
  workspace_name: string
  workspace_is_pinned: number
  workspace_pin_sort_key: string
  workspace_last_opened_at: number
  workspace_created_at: number
  task_id: string
  task_title: string
  task_sort_key: string
  task_created_at: number
  scene_id: string
  scene_name: string
  scene_sort_key: string
  scene_created_at: number
  mount_id: string | null
  main_window_id: string | null
  task_ordinal: number | null
  detached_window_id: string | null
  parent_session_id: string | null
  sibling_created_seq: number
  last_user_interaction_seq: number
}

export class HostTopologyProjector {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  identify(caller: HostCallerIdentity): { caller: HostCallerIdentity; target: HostTarget } {
    const target = this.#allTargets().find(({ sessionId }) => sessionId === caller.sessionId)
    if (!target) throw new HostControlTargetNotFoundError('当前调用会话已不在 Matou 会话图中')
    return { caller: { ...caller }, target }
  }

  list(caller: HostCallerIdentity, scope: HostListScope): HostTarget[] {
    const targets = this.#allTargets()
    if (scope === 'all') return targets
    const current = targets.find(({ sessionId }) => sessionId === caller.sessionId)
    if (!current) throw new HostControlTargetNotFoundError('当前调用会话已不在 Matou 会话图中')
    const parentRef = current.dag.parentRef
    return targets.filter((target) =>
      target.canvas.id === current.canvas.id && target.dag.parentRef === parentRef
    )
  }

  resolve(caller: HostCallerIdentity, selector: HostTargetSelector): string {
    const targets = this.#allTargets()
    const current = targets.find(({ sessionId }) => sessionId === caller.sessionId)
    if (!current) throw new HostControlTargetNotFoundError('当前调用会话已不在 Matou 会话图中')

    if (selector.kind === 'self') return current.sessionId
    if (selector.kind === 'session') {
      return requireTarget(
        targets.find(({ sessionId }) => sessionId === selector.sessionId),
        '指定会话已不在 Matou 会话图中'
      ).sessionId
    }
    if (selector.kind === 'ref') {
      return requireTarget(
        targets.find(({ ref }) => ref === selector.ref),
        `目标 ${selector.ref} 已不在 Matou 会话图中`
      ).sessionId
    }

    const siblings = targets.filter((target) =>
      target.canvas.id === current.canvas.id && target.dag.parentRef === current.dag.parentRef
    )
    if (selector.kind === 'sibling') {
      return requireTarget(siblings[selector.ordinal - 1], `当前层级没有第 ${selector.ordinal} 个会话`).sessionId
    }
    if (selector.kind === 'relative') {
      const index = siblings.findIndex(({ sessionId }) => sessionId === current.sessionId)
      const offset = selector.direction === 'left' ? -1 : 1
      return requireTarget(
        siblings[index + offset],
        selector.direction === 'left' ? '当前会话左侧没有同层会话' : '当前会话右侧没有同层会话'
      ).sessionId
    }
    if (selector.relation === 'parent') {
      if (!current.dag.parentRef) throw new HostControlTargetNotFoundError('当前会话没有父会话')
      return requireTarget(
        targets.find(({ ref }) => ref === current.dag.parentRef),
        '父会话已不在 Matou 会话图中'
      ).sessionId
    }
    const ordinal = selector.ordinal ?? 1
    const childRef = current.dag.childRefs[ordinal - 1]
    return requireTarget(
      targets.find(({ ref }) => ref === childRef),
      `当前会话没有第 ${ordinal} 个子会话`
    ).sessionId
  }

  #allTargets(): HostTarget[] {
    const rows = this.#database.all<TargetRow>(
      `SELECT
         sessions.id AS session_id,
         execution_context.id AS execution_context_id,
         execution_context.kind AS execution_context_kind,
         git_state.state AS git_state,
         git_state.branch AS git_branch,
         worktree.id AS worktree_id,
         worktree.branch_name AS worktree_branch_name,
         sessions.title,
         sessions.kind,
         sessions.cwd,
         sessions.work_status,
         workspaces.id AS workspace_id,
         workspaces.name AS workspace_name,
         workspaces.is_pinned AS workspace_is_pinned,
         workspaces.pin_sort_key AS workspace_pin_sort_key,
         workspaces.last_opened_at AS workspace_last_opened_at,
         workspaces.created_at AS workspace_created_at,
         tasks.id AS task_id,
         tasks.title AS task_title,
         tasks.sort_key AS task_sort_key,
         tasks.created_at AS task_created_at,
         scenes.id AS scene_id,
         scenes.name AS scene_name,
         scenes.sort_key AS scene_sort_key,
         scenes.created_at AS scene_created_at,
         mounts.id AS mount_id,
         placement.window_id AS main_window_id,
         placement.ordinal AS task_ordinal,
         detached.native_window_key AS detached_window_id,
         structural.to_session_id AS parent_session_id,
         membership.sibling_created_seq,
         membership.last_user_interaction_seq
       FROM session_canvas_memberships AS membership
       JOIN sessions ON sessions.id = membership.session_id
       JOIN scenes ON scenes.id = membership.scene_id
       JOIN execution_contexts AS execution_context
         ON execution_context.id = sessions.execution_context_id
        AND execution_context.archived_at IS NULL
       LEFT JOIN execution_context_git_states AS git_state
         ON git_state.execution_context_id = execution_context.id
       LEFT JOIN worktrees AS worktree
         ON worktree.execution_context_id = execution_context.id
        AND worktree.state <> 'removed'
       JOIN tasks ON tasks.id = scenes.task_id
       JOIN workspaces ON workspaces.id = tasks.workspace_id
       LEFT JOIN session_relations_current AS structural
         ON structural.from_session_id = sessions.id
        AND structural.relation_kind IN ('derived-from', 'forked-from')
       LEFT JOIN session_mounts AS mounts
         ON mounts.id = (
           SELECT candidate.id FROM session_mounts AS candidate
           WHERE candidate.scene_id = scenes.id AND candidate.session_id = sessions.id
           ORDER BY candidate.created_at, candidate.id LIMIT 1
         )
       LEFT JOIN scene_windows AS detached
         ON detached.id = mounts.scene_window_id AND detached.state = 'detached'
       LEFT JOIN window_task_placements AS placement ON placement.task_id = tasks.id
       WHERE sessions.archived_at IS NULL
         AND scenes.archived_at IS NULL
         AND tasks.archived_at IS NULL
         AND workspaces.archived_at IS NULL`
    )
    if (rows.length === 0) return []

    const windowOrder = new Map(this.#database.all<{ id: string }>(
      `SELECT id FROM app_windows WHERE state <> 'closed' ORDER BY created_at, id`
    ).map((row, index) => [row.id, index + 1] as const))
    const workspaceOrder = rankUnique(rows, 'workspace_id', compareWorkspace)
    const taskOrder = rankWithin(rows, 'workspace_id', 'task_id', compareTask)
    const sceneOrder = rankWithin(rows, 'task_id', 'scene_id', compareScene)
    const rowBySession = new Map(rows.map((row) => [row.session_id, row] as const))
    const depthMemo = new Map<string, number>()
    const depthFor = (sessionId: string, trail = new Set<string>()): number => {
      const memoized = depthMemo.get(sessionId)
      if (memoized !== undefined) return memoized
      if (trail.has(sessionId)) return 0
      trail.add(sessionId)
      const parentId = rowBySession.get(sessionId)?.parent_session_id
      const depth = parentId && rowBySession.has(parentId) ? depthFor(parentId, trail) + 1 : 0
      depthMemo.set(sessionId, depth)
      return depth
    }
    const siblingGroups = new Map<string, TargetRow[]>()
    for (const row of rows) {
      const key = siblingKey(row.scene_id, row.parent_session_id)
      const group = siblingGroups.get(key) ?? []
      group.push(row)
      siblingGroups.set(key, group)
    }
    for (const group of siblingGroups.values()) group.sort(compareSibling)

    const provisional = rows.map((row): HostTarget => {
      const detached = row.detached_window_id !== null
      const windowId = row.detached_window_id ?? row.main_window_id ?? `unplaced:${row.task_id}`
      const siblings = siblingGroups.get(siblingKey(row.scene_id, row.parent_session_id)) ?? []
      const sessionOrdinal = siblings.findIndex(({ session_id }) => session_id === row.session_id) + 1
      return {
        ref: sessionRef(row.session_id),
        workspaceId: row.workspace_id,
        taskId: row.task_id,
        sessionId: row.session_id,
        ...(row.mount_id === null ? {} : { mountId: row.mount_id }),
        title: row.title,
        profile: row.kind === 'agent-team-member' ? 'claude-code' : row.kind,
        cwd: row.cwd,
        workStatus: row.work_status,
        environment: targetEnvironment(row),
        window: {
          id: windowId,
          kind: detached ? 'detached-terminal' : 'main',
          ordinal: windowOrder.get(windowId) ?? windowOrder.size + 1
        },
        workspace: {
          id: row.workspace_id,
          name: row.workspace_name,
          ordinal: workspaceOrder.get(row.workspace_id) ?? 1
        },
        task: {
          id: row.task_id,
          name: row.task_title,
          ordinal: row.task_ordinal === null
            ? taskOrder.get(`${row.workspace_id}:${row.task_id}`) ?? 1
            : row.task_ordinal + 1
        },
        canvas: {
          id: row.scene_id,
          name: row.scene_name,
          ordinal: sceneOrder.get(`${row.task_id}:${row.scene_id}`) ?? 1
        },
        session: { id: row.session_id, ordinal: sessionOrdinal, detached },
        dag: {
          depth: depthFor(row.session_id),
          ...(row.parent_session_id === null ? {} : { parentRef: sessionRef(row.parent_session_id) }),
          childRefs: [],
          siblingRefs: siblings.map(({ session_id }) => sessionRef(session_id))
        }
      }
    })
    const byRef = new Map(provisional.map((target) => [target.ref, target] as const))
    for (const target of provisional) {
      if (!target.dag.parentRef) continue
      byRef.get(target.dag.parentRef)?.dag.childRefs.push(target.ref)
    }
    for (const target of provisional) {
      target.dag.childRefs.sort((left, right) => {
        const a = byRef.get(left)?.session.ordinal ?? 0
        const b = byRef.get(right)?.session.ordinal ?? 0
        return a - b || left.localeCompare(right)
      })
    }
    return provisional.sort(compareProjectedTarget)
  }
}

function sessionRef(sessionId: string): string {
  return `session:${sessionId}`
}

function siblingKey(sceneId: string, parentSessionId: string | null): string {
  return `${sceneId}:${parentSessionId ?? '<root>'}`
}

function compareSibling(left: TargetRow, right: TargetRow): number {
  return right.last_user_interaction_seq - left.last_user_interaction_seq ||
    left.sibling_created_seq - right.sibling_created_seq ||
    left.session_id.localeCompare(right.session_id)
}

function compareWorkspace(left: TargetRow, right: TargetRow): number {
  return right.workspace_is_pinned - left.workspace_is_pinned ||
    (left.workspace_is_pinned
      ? left.workspace_pin_sort_key.localeCompare(right.workspace_pin_sort_key)
      : right.workspace_last_opened_at - left.workspace_last_opened_at) ||
    right.workspace_created_at - left.workspace_created_at ||
    left.workspace_id.localeCompare(right.workspace_id)
}

function compareTask(left: TargetRow, right: TargetRow): number {
  return (left.task_ordinal ?? Number.MAX_SAFE_INTEGER) -
    (right.task_ordinal ?? Number.MAX_SAFE_INTEGER) ||
    left.task_sort_key.localeCompare(right.task_sort_key) ||
    left.task_created_at - right.task_created_at ||
    left.task_id.localeCompare(right.task_id)
}

function compareScene(left: TargetRow, right: TargetRow): number {
  return left.scene_sort_key.localeCompare(right.scene_sort_key) ||
    left.scene_created_at - right.scene_created_at ||
    left.scene_id.localeCompare(right.scene_id)
}

function compareProjectedTarget(left: HostTarget, right: HostTarget): number {
  return left.window.ordinal - right.window.ordinal ||
    left.workspace.ordinal - right.workspace.ordinal ||
    left.task.ordinal - right.task.ordinal ||
    left.canvas.ordinal - right.canvas.ordinal ||
    left.dag.depth - right.dag.depth ||
    left.session.ordinal - right.session.ordinal ||
    left.sessionId.localeCompare(right.sessionId)
}

function rankUnique(
  rows: TargetRow[],
  idKey: 'workspace_id',
  compare: (left: TargetRow, right: TargetRow) => number
): Map<string, number> {
  const unique = [...new Map(rows.map((row) => [row[idKey], row] as const)).values()].sort(compare)
  return new Map(unique.map((row, index) => [row[idKey], index + 1] as const))
}

function rankWithin(
  rows: TargetRow[],
  parentKey: 'workspace_id' | 'task_id',
  idKey: 'task_id' | 'scene_id',
  compare: (left: TargetRow, right: TargetRow) => number
): Map<string, number> {
  const groups = new Map<string, Map<string, TargetRow>>()
  for (const row of rows) {
    const group = groups.get(row[parentKey]) ?? new Map<string, TargetRow>()
    group.set(row[idKey], row)
    groups.set(row[parentKey], group)
  }
  const ranks = new Map<string, number>()
  for (const [parentId, group] of groups) {
    ;[...group.values()].sort(compare).forEach((row, index) => {
      ranks.set(`${parentId}:${row[idKey]}`, index + 1)
    })
  }
  return ranks
}

function requireTarget(target: HostTarget | undefined, message: string): HostTarget {
  if (!target) throw new HostControlTargetNotFoundError(message)
  return target
}

function targetEnvironment(row: TargetRow): HostTargetEnvironment {
  const branch = row.git_branch ?? row.worktree_branch_name
  const mode = row.execution_context_kind === 'git-worktree'
    ? 'git-worktree'
    : row.git_state === 'ready' ? 'git-checkout' : 'directory'
  return {
    executionContextRef: `context:${row.execution_context_id}`,
    mode,
    ...(branch === null ? {} : { branch }),
    ...(row.worktree_id === null ? {} : { worktreeRef: `worktree:${row.worktree_id}` })
  }
}
