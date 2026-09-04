import { mkdir, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { DetachedSessionService } from '../hierarchy/detached-session-service'
import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { SessionCanvasService } from '../session-canvas/session-canvas-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { HostTopologyProjector } from './host-topology-projector'

let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let canvas: SessionCanvasService
let detached: DetachedSessionService
let projector: HostTopologyProjector
let workspaceRoot: string

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-host-topology-'))
  workspaceRoot = join(root, 'workspace')
  await mkdir(workspaceRoot)
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  canvas = new SessionCanvasService(database, transactions)
  detached = new DetachedSessionService(database, transactions)
  projector = new HostTopologyProjector(database)
})

afterEach(() => database.close())

describe('HostTopologyProjector', () => {
  it('projects the current DAG level without mixing its parent into sibling ordinals', () => {
    const fixture = graphFixture()
    const caller = { runId: 'run-child-1', sessionId: fixture.child1.session!.id }

    expect(projector.identify(caller)).toMatchObject({
      caller,
      target: {
        workspace: { name: 'Workspace', ordinal: 1 },
        task: { name: '默认', ordinal: 1 },
        canvas: { id: fixture.initial.scene!.id, ordinal: 1 },
        session: { ordinal: 1, detached: false },
        dag: { depth: 1, parentRef: `session:${fixture.initial.session!.id}` }
      }
    })
    expect(projector.list(caller, 'current-level').map(({ sessionId }) => sessionId)).toEqual([
      fixture.child1.session!.id,
      fixture.child2.session!.id
    ])
    expect(projector.list(caller, 'current-level').map(({ session }) => session.ordinal)).toEqual([1, 2])
    expect(projector.list(caller, 'current-level').some(
      ({ sessionId }) => sessionId === fixture.initial.session!.id
    )).toBe(false)
  })

  it('resolves left, right, parent, child and stable sibling ordinals', () => {
    const fixture = graphFixture()
    const firstChild = { runId: 'run-child-1', sessionId: fixture.child1.session!.id }
    const secondChild = { runId: 'run-child-2', sessionId: fixture.child2.session!.id }
    const parent = { runId: 'run-parent', sessionId: fixture.initial.session!.id }

    expect(projector.resolve(firstChild, { kind: 'relative', direction: 'right' }))
      .toBe(fixture.child2.session!.id)
    expect(projector.resolve(secondChild, { kind: 'relative', direction: 'left' }))
      .toBe(fixture.child1.session!.id)
    expect(projector.resolve(firstChild, { kind: 'relation', relation: 'parent' }))
      .toBe(fixture.initial.session!.id)
    expect(projector.resolve(parent, { kind: 'relation', relation: 'child', ordinal: 2 }))
      .toBe(fixture.child2.session!.id)
    expect(projector.resolve(secondChild, {
      kind: 'sibling', ordinal: 1, projectionRevision: 'server-validated'
    })).toBe(fixture.child1.session!.id)
  })

  it('keeps detached nodes in their DAG identity and excludes archived nodes', () => {
    const fixture = graphFixture()
    detached.detach(command('detach-child'), {
      mainWindowId: 'window-1', sceneWindowId: 'window-detached',
      sceneId: fixture.initial.scene!.id, mountId: fixture.child2.mount!.id,
      sessionId: fixture.child2.session!.id, nativeWindowKey: 'native-detached', now: 50
    })
    const childTargets = projector.list(
      { runId: 'run-child-1', sessionId: fixture.child1.session!.id },
      'current-level'
    )
    expect(childTargets.find(({ sessionId }) => sessionId === fixture.child2.session!.id))
      .toMatchObject({
        window: { id: 'native-detached', kind: 'detached-terminal' },
        session: { detached: true },
        dag: { parentRef: `session:${fixture.initial.session!.id}` }
      })

    database.run(
      "UPDATE sessions SET archived_at = 60, status = 'archived' WHERE id = ?",
      fixture.child2.session!.id
    )
    expect(projector.list(
      { runId: 'run-child-1', sessionId: fixture.child1.session!.id },
      'current-level'
    ).map(({ sessionId }) => sessionId)).toEqual([fixture.child1.session!.id])
  })
})

function graphFixture() {
  const initial = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'Workspace', now: 1
  })
  const rootSibling = canvas.createShellSibling(command('root-sibling'), {
    windowId: 'window-1', sceneId: initial.scene!.id,
    sourceSessionId: initial.session!.id, now: 10
  })
  const child1 = canvas.createShellSibling(command('child-1'), {
    windowId: 'window-1', sceneId: initial.scene!.id,
    sourceSessionId: initial.session!.id, parentSessionId: initial.session!.id, now: 20
  })
  const child2 = canvas.createShellSibling(command('child-2'), {
    windowId: 'window-1', sceneId: initial.scene!.id,
    sourceSessionId: child1.session!.id, parentSessionId: initial.session!.id, now: 30
  })
  return { initial, rootSibling, child1, child2 }
}

function command(commandId: string) {
  return { commandId, commandType: commandId, requestHash: `hash:${commandId}` }
}

describe('HostTopologyProjector environments', () => {
  it('includes branch and stable worktree refs in all-scope topology', () => {
    const fixture = graphFixture()
    const executionContextId = fixture.child1.session!.executionContextId
    database.run("UPDATE execution_contexts SET kind = 'git-worktree' WHERE id = ?", executionContextId)
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES (?, ?, ?, ?, ?, 'ready', 2, 2)`,
      'worktree-2', executionContextId, workspaceRoot, join(workspaceRoot, 'service-refactor'),
      'feature/service-refactor'
    )
    database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, error_message, updated_at
       ) VALUES (?, ?, 'ready', ?, NULL, 0, NULL, 2)`,
      executionContextId, workspaceRoot, 'feature/service-refactor'
    )

    const target = projector.list(
      { runId: 'run-child-1', sessionId: fixture.child1.session!.id },
      'all'
    ).find(({ sessionId }) => sessionId === fixture.child1.session!.id)

    expect(target?.environment).toEqual({
      executionContextRef: `context:${executionContextId}`,
      mode: 'git-worktree',
      branch: 'feature/service-refactor',
      worktreeRef: 'worktree:worktree-2'
    })
  })
})
