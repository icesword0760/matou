import { execFileSync } from 'node:child_process'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { SessionCanvasService } from '../session-canvas/session-canvas-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import {
  HostActionTargetResolver,
  HostActionTargetResolverError
} from './host-action-target-resolver'
import { HostTopologyProjector } from './host-topology-projector'

let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let canvas: SessionCanvasService
let projector: HostTopologyProjector
let resolver: HostActionTargetResolver
let root: string
let workspaceRoot: string

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-host-action-resolver-'))
  workspaceRoot = join(root, 'workspace')
  await mkdir(workspaceRoot)
  await writeFile(join(workspaceRoot, 'README.md'), 'baseline\n')
  execFileSync('git', ['init', '-b', 'main'], { cwd: workspaceRoot })
  execFileSync('git', ['config', 'user.name', 'Matou Test'], { cwd: workspaceRoot })
  execFileSync('git', ['config', 'user.email', 'matou@example.test'], { cwd: workspaceRoot })
  execFileSync('git', ['add', 'README.md'], { cwd: workspaceRoot })
  execFileSync('git', ['commit', '-m', 'baseline'], { cwd: workspaceRoot })

  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  canvas = new SessionCanvasService(database, transactions)
  projector = new HostTopologyProjector(database)
  resolver = new HostActionTargetResolver(database, projector)
})

afterEach(async () => {
  database.close()
  await rm(root, { recursive: true, force: true })
})

describe('HostActionTargetResolver', () => {
  it('returns one complete hierarchy path for each unique stable entity ref', () => {
    const fixture = graphFixture()
    const caller = { runId: 'run-parent', sessionId: fixture.parent.session!.id }
    const revision = resolver.projectionRevision(caller)

    expect(resolver.resolveEntity(caller, {
      kind: 'ref', ref: `workspace:${fixture.parent.workspace!.id}`, projectionRevision: revision
    }, revision)).toMatchObject({
      kind: 'workspace', windowId: 'window-1', workspaceId: fixture.parent.workspace!.id,
      taskId: fixture.parent.task!.id, sceneId: fixture.parent.scene!.id
    })
    expect(resolver.resolveEntity(caller, {
      kind: 'ref', ref: `task:${fixture.parent.task!.id}`, projectionRevision: revision
    }, revision)).toMatchObject({
      kind: 'task', windowId: 'window-1', workspaceId: fixture.parent.workspace!.id,
      taskId: fixture.parent.task!.id, sceneId: fixture.parent.scene!.id
    })
    expect(resolver.resolveEntity(caller, {
      kind: 'ref', ref: `scene:${fixture.parent.scene!.id}`, projectionRevision: revision
    }, revision)).toMatchObject({
      kind: 'canvas', windowId: 'window-1', workspaceId: fixture.parent.workspace!.id,
      taskId: fixture.parent.task!.id, sceneId: fixture.parent.scene!.id
    })
    expect(resolver.resolveEntity(caller, {
      kind: 'ref', ref: `session:${fixture.parent.session!.id}`, projectionRevision: revision
    }, revision)).toMatchObject({
      kind: 'session', windowId: 'window-1', workspaceId: fixture.parent.workspace!.id,
      taskId: fixture.parent.task!.id, sceneId: fixture.parent.scene!.id,
      sessionId: fixture.parent.session!.id, mountId: fixture.parent.mount!.id
    })
  })

  it('uses the topology projector for current and relative session selectors', () => {
    const fixture = graphFixture()
    const caller = { runId: 'run-child-1', sessionId: fixture.child1.session!.id }

    expect(resolver.resolveEntity(caller, {
      kind: 'current', entity: 'canvas'
    }, 'not-used')).toMatchObject({ kind: 'canvas', sceneId: fixture.parent.scene!.id })
    const revision = resolver.projectionRevision(caller, 'current-level')
    expect(resolver.resolveEntity(caller, {
      kind: 'relative', direction: 'right', projectionRevision: revision
    }, revision)).toMatchObject({ kind: 'session', sessionId: fixture.child2.session!.id })
  })

  it('rejects every position-based action selector when its projection revision is stale', () => {
    const fixture = graphFixture()
    const childCaller = { runId: 'run-child-1', sessionId: fixture.child1.session!.id }
    const childRevision = resolver.projectionRevision(childCaller, 'current-level')
    database.run(
      'UPDATE session_canvas_memberships SET last_user_interaction_seq = 99 WHERE session_id = ?',
      fixture.child2.session!.id
    )

    expectFault(() => resolver.resolveEntity(childCaller, {
      kind: 'relative', direction: 'right', projectionRevision: childRevision
    }, childRevision), 'STALE_PROJECTION')
    expectFault(() => resolver.resolveEntity(childCaller, {
      kind: 'sibling', ordinal: 1, projectionRevision: childRevision
    }, childRevision), 'STALE_PROJECTION')

  })

  it('invalidates a relation selector when ordered children are inserted, removed, or reordered', () => {
    const fixture = graphFixture()
    const caller = { runId: 'run-parent', sessionId: fixture.parent.session!.id }
    const selector = (projectionRevision: string) => ({
      kind: 'relation' as const,
      relation: 'child' as const,
      ordinal: 1,
      projectionRevision
    })

    const beforeInsert = resolver.projectionRevision(caller, 'current-level')
    const inserted = canvas.createShellSibling(command('inserted-child'), {
      windowId: 'window-1', sceneId: fixture.parent.scene!.id,
      sourceSessionId: fixture.parent.session!.id,
      parentSessionId: fixture.parent.session!.id, now: 30
    })
    expectFault(
      () => resolver.resolveEntity(caller, selector(beforeInsert), beforeInsert),
      'STALE_PROJECTION'
    )

    const beforeRemove = resolver.projectionRevision(caller, 'current-level')
    database.run(
      "UPDATE sessions SET status = 'archived', archived_at = 31 WHERE id = ?",
      inserted.session!.id
    )
    expectFault(
      () => resolver.resolveEntity(caller, selector(beforeRemove), beforeRemove),
      'STALE_PROJECTION'
    )

    const beforeReorder = resolver.projectionRevision(caller, 'current-level')
    const [firstChildRef, secondChildRef] = projector.identify(caller).target.dag.childRefs
    database.run(
      'UPDATE session_canvas_memberships SET last_user_interaction_seq = 0 WHERE session_id = ?',
      firstChildRef!.slice('session:'.length)
    )
    database.run(
      'UPDATE session_canvas_memberships SET last_user_interaction_seq = 100000 WHERE session_id = ?',
      secondChildRef!.slice('session:'.length)
    )
    expectFault(
      () => resolver.resolveEntity(caller, selector(beforeReorder), beforeReorder),
      'STALE_PROJECTION'
    )
  })

  it('counts descendants and live terminal runs before removal', () => {
    const fixture = graphFixture()
    database.run(
      `INSERT INTO session_runs (
         id, session_id, ordinal, runtime_generation, pid, status, started_at
       ) VALUES (?, ?, 1, 'generation-1', ?, 'running', 1)`,
      'run-parent', fixture.parent.session!.id, 1001
    )
    database.run(
      `INSERT INTO session_runs (
         id, session_id, ordinal, runtime_generation, pid, status, started_at
       ) VALUES (?, ?, 1, 'generation-1', NULL, 'starting', 1)`,
      'run-child-1', fixture.child1.session!.id
    )

    const impact = resolver.previewRemoval({
      kind: 'session', windowId: 'window-1', workspaceId: fixture.parent.workspace!.id,
      taskId: fixture.parent.task!.id, sceneId: fixture.parent.scene!.id,
      sessionId: fixture.parent.session!.id, mountId: fixture.parent.mount!.id
    }, 'subtree')

    expect(impact).toMatchObject({
      sessions: 3, descendants: 2, liveRuns: 2, terminalProcesses: 1,
      preservesProjectFiles: true, preservesBranches: true, preservesWorktrees: true
    })
    expect(resolver.toHostImpactSummary(impact)).toMatchObject({
      scope: 'subtree', sessions: 3,
      target: {
        workspace: { title: 'Workspace', path: workspaceRoot },
        task: { title: '默认' },
        canvas: { title: expect.any(String) },
        session: { title: 'Shell' }
      }
    })
  })

  it('resolves main only when the submitted worktree ref carries main', () => {
    const fixture = graphFixture()
    const executionContextId = fixture.parent.session!.executionContextId
    database.run("UPDATE execution_contexts SET kind = 'git-worktree' WHERE id = ?", executionContextId)
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES ('main-worktree', ?, ?, ?, 'main', 'ready', 1, 1)`,
      executionContextId, workspaceRoot, workspaceRoot
    )
    database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, error_message, updated_at
       ) VALUES (?, ?, 'ready', 'main', NULL, 0, NULL, 1)`,
      executionContextId, workspaceRoot
    )
    const source = projector.identify({
      runId: 'run-parent', sessionId: fixture.parent.session!.id
    }).target

    expect(resolver.resolveForkEnvironment(source, {
      mode: 'existing-worktree', branch: 'main', worktreeRef: 'worktree:main-worktree'
    })).toMatchObject({
      mode: 'existing-worktree', executionContextId, worktreeId: 'main-worktree'
    })
    expectFault(() => resolver.resolveForkEnvironment(source, {
      mode: 'existing-worktree', branch: 'feature/other', worktreeRef: 'worktree:main-worktree'
    }), 'BRANCH_CONFLICT')
  })

  it('allows only current for an ordinary directory and catches branch collisions before writes', () => {
    const fixture = graphFixture()
    const source = projector.identify({
      runId: 'run-parent', sessionId: fixture.parent.session!.id
    }).target

    expect(resolver.resolveForkEnvironment(source, { mode: 'current' })).toEqual({
      mode: 'current', executionContextId: fixture.parent.session!.executionContextId
    })
    expectFault(() => resolver.resolveForkEnvironment(source, {
      mode: 'new-worktree', branch: 'feature/new-work'
    }), 'WORKTREE_CONFLICT')

    database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, error_message, updated_at
       ) VALUES (?, ?, 'ready', 'main', NULL, 0, NULL, 1)`,
      fixture.parent.session!.executionContextId, workspaceRoot
    )
    const gitSource = projector.identify({
      runId: 'run-parent', sessionId: fixture.parent.session!.id
    }).target
    expectFault(() => resolver.resolveForkEnvironment(gitSource, {
      mode: 'new-worktree', branch: 'main'
    }), 'BRANCH_CONFLICT')
    expect(resolver.resolveForkEnvironment(gitSource, {
      mode: 'new-worktree', branch: 'feature/new-work'
    })).toEqual({ mode: 'new-worktree', branch: 'feature/new-work' })
  })

  it('exposes only stable resolver faults', () => {
    const fixture = graphFixture()
    const caller = { runId: 'run-parent', sessionId: fixture.parent.session!.id }
    const revision = resolver.projectionRevision(caller)

    expect(() => resolver.resolveEntity(caller, {
      kind: 'ref', ref: 'scene:missing', projectionRevision: revision
    }, revision)).toThrow(HostActionTargetResolverError)
    expectFault(() => resolver.resolveEntity(caller, {
      kind: 'ref', ref: 'scene:missing', projectionRevision: revision
    }, revision), 'TARGET_NOT_FOUND')
  })
})

function graphFixture() {
  const parent = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'Workspace', now: 1
  })
  const rootSibling = canvas.createShellSibling(command('root-sibling'), {
    windowId: 'window-1', sceneId: parent.scene!.id,
    sourceSessionId: parent.session!.id, now: 5
  })
  const child1 = canvas.createShellSibling(command('child-1'), {
    windowId: 'window-1', sceneId: parent.scene!.id,
    sourceSessionId: parent.session!.id, parentSessionId: parent.session!.id, now: 10
  })
  const child2 = canvas.createShellSibling(command('child-2'), {
    windowId: 'window-1', sceneId: parent.scene!.id,
    sourceSessionId: child1.session!.id, parentSessionId: parent.session!.id, now: 20
  })
  return { parent, rootSibling, child1, child2 }
}

function command(commandId: string) {
  return { commandId, commandType: commandId, requestHash: `hash:${commandId}` }
}


function expectFault(action: () => unknown, code: string): void {
  try {
    action()
  } catch (error) {
    expect(error).toMatchObject({ code })
    return
  }
  throw new Error(`expected resolver fault ${code}`)
}
