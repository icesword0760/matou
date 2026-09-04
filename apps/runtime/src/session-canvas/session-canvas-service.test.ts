import { mkdir, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { SessionCanvasService } from './session-canvas-service'

let database: RuntimeDatabase
let service: SessionCanvasService
let hierarchy: HierarchyApplicationService
let workspaceRoot: string

beforeEach(async () => {
  const testRoot = await mkdtemp(join(tmpdir(), 'matou-session-canvas-'))
  workspaceRoot = join(testRoot, 'workspace')
  await mkdir(workspaceRoot)
  database = RuntimeDatabase.open(join(testRoot, 'data', 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  service = new SessionCanvasService(database, transactions)
})

afterEach(() => database.close())

describe('SessionCanvasService', () => {
  it('creates a named canvas while preserving the previously focused canvas', () => {
    const initial = bootstrap()

    const result = service.createCanvas(command('named-canvas'), {
      windowId: 'window-1', taskId: initial.task!.id,
      title: '发布检查', navigation: 'preserve', now: 20
    })

    expect(result.created.scene).toMatchObject({ name: '发布检查', titlePinned: true })
    expect(result.created.session).toMatchObject({ kind: 'shell', title: 'Shell' })
    expect(result.scene?.id).toBe(initial.scene?.id)
    expect(result.navigation.sceneByTask[initial.task!.id]).toBe(initial.scene?.id)
    const createdEvent = database.get<{ payload_json: string }>(
      `SELECT payload_json FROM domain_events
       WHERE event_type = 'scene.created' AND aggregate_id = ?`,
      result.created.scene.id
    )
    expect(JSON.parse(createdEvent!.payload_json)).toMatchObject({ name: '发布检查' })
  })

  it.each(['shell', 'claude-code', 'codex'] as const)(
    'creates a %s sibling with its final title and no provider history while preserving focus',
    (profile) => {
      const initial = bootstrap()

      const result = service.createSessionSibling(command(`named-${profile}`), {
        windowId: 'window-1', sceneId: initial.scene!.id,
        sourceSessionId: initial.session!.id, profile, title: `New ${profile}`,
        navigation: 'preserve', now: 20
      })

      expect(result.created.session).toMatchObject({ kind: profile, title: `New ${profile}` })
      expect(result.session?.id).toBe(initial.session?.id)
      expect(result.navigation.sessionByScene[initial.scene!.id]).toBe(initial.session?.id)
      expect(result.graph.focusedSessionId).toBe(initial.session?.id)
      expect(database.get(
        'SELECT id FROM provider_bindings WHERE session_id = ?',
        result.created.session.id
      )).toBeUndefined()
      const createdEvent = database.get<{ payload_json: string }>(
        `SELECT payload_json FROM domain_events
         WHERE event_type = 'session.created' AND aggregate_id = ?`,
        result.created.session.id
      )
      expect(JSON.parse(createdEvent!.payload_json)).toMatchObject({
        kind: profile, title: `New ${profile}`
      })
    }
  )

  it('creates a sequentially named Scene and focused root Shell atomically', () => {
    const initial = bootstrap()

    const first = service.createCanvas(command('canvas-1'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 20
    })
    const second = service.createCanvas(command('canvas-2'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 21
    })

    expect(first.scene).toMatchObject({ name: '新画布', taskId: initial.task!.id })
    expect(first.session).toMatchObject({ kind: 'shell', title: 'Shell' })
    expect(first.mount?.sessionId).toBe(first.session?.id)
    expect(first.graph).toMatchObject({
      sceneId: first.scene!.id,
      focusedSessionId: first.session!.id
    })
    expect(first.graph.nodes).toEqual([
      expect.objectContaining({
        sessionId: first.session!.id,
        currentMode: 'shell'
      })
    ])
    expect(first.graph.nodes[0]?.parentSessionId).toBeUndefined()
    expect(second.scene?.name).toBe('新画布 2')
    expect(membershipSequence(second.session!.id))
      .toBeGreaterThan(membershipSequence(first.session!.id))
    expect(second.navigation.sessionByScene[second.scene!.id]).toBe(second.session!.id)
    expect(eventTypes()).toEqual(expect.arrayContaining([
      'scene.canvas-created',
      'session.canvas-membership-created'
    ]))
  })

  it('replays one canvas command without duplicating Scene, Session or membership', () => {
    const initial = bootstrap()
    const input = { windowId: 'window-1', taskId: initial.task!.id, now: 20 }

    const first = service.createCanvas(command('canvas-replay'), input)
    const replay = service.createCanvas(command('canvas-replay'), input)

    expect(replay).toEqual(first)
    expect(count('scenes', `task_id = '${initial.task!.id}'`)).toBe(2)
    expect(count('session_canvas_memberships')).toBe(2)
  })

  it('rejects a new canvas while its Workspace directory needs relinking', () => {
    const initial = bootstrap()
    database.run(
      `INSERT INTO workspace_path_state (
         workspace_id, status, reason, checked_at, validation_generation
       ) VALUES (?, 'invalid', 'missing', 20, 1)`,
      initial.workspace!.id
    )

    expect(() => service.createCanvas(command('canvas-invalid-path'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 21
    })).toThrow('工作区目录不可用，请先在本地恢复原路径，或移出该工作区')
    expect(count('scenes', `task_id = '${initial.task!.id}'`)).toBe(1)
  })

  it('appends a root Shell sibling with no structural parent and focuses it', () => {
    const initial = bootstrap()

    const result = service.createShellSibling(command('root-sibling'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })

    expect(result.session).toMatchObject({ kind: 'shell', title: 'Shell' })
    expect(result.graph.nodes).toHaveLength(2)
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === result.session!.id))
      .toMatchObject({ currentMode: 'shell', lastUserInteractionSeq: 0 })
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === result.session!.id)?.parentSessionId)
      .toBeUndefined()
    expect(result.graph.edges).toEqual([])
    expect(result.graph.focusedSessionId).toBe(result.session!.id)
    expect(result.graph.nodes.at(-1)?.sessionId).toBe(result.session!.id)
  })

  it('creates a Shell sibling in an explicitly selected execution context', async () => {
    const initial = bootstrap()
    const worktreeRoot = join(workspaceRoot, 'feature-worktree')
    await mkdir(worktreeRoot)
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('context-worktree', ?, 'git-worktree', ?, 19)`,
      initial.workspace!.id, worktreeRoot
    )

    const result = service.createShellSibling(command('worktree-shell'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id,
      executionContextId: 'context-worktree', now: 20
    })

    expect(result.session).toMatchObject({
      executionContextId: 'context-worktree', cwd: worktreeRoot,
      kind: 'shell', title: 'Shell'
    })
  })

  it('adds a child-list Shell as derived-from the same structural parent', () => {
    const initial = bootstrap()
    const firstChild = service.createShellSibling(command('first-child-shell'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    insertStructuralRelation(
      'make-child', firstChild.session!.id, initial.session!.id, 21
    )

    const result = service.createShellSibling(command('second-child-shell'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: firstChild.session!.id, now: 22
    })

    expect(result.graph.nodes.find(({ sessionId }) => sessionId === result.session!.id))
      .toMatchObject({
        parentSessionId: initial.session!.id,
        relationKind: 'derived-from',
        currentMode: 'shell'
      })
    expect(result.graph.edges).toContainEqual(expect.objectContaining({
      parentSessionId: initial.session!.id,
      childSessionId: result.session!.id,
      relationKind: 'derived-from'
    }))
    expect(result.graph.focusedSessionId).toBe(result.session!.id)
  })

  it('adds a Shell to a child level after every prior child stopped', () => {
    const initial = bootstrap()
    const historical = service.createShellSibling(command('only-child-shell'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    insertStructuralRelation('only-child-parent', historical.session!.id, initial.session!.id, 21)
    archiveSession(historical.session!.id, 22)

    const result = service.createShellSibling(command('new-child-after-history'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: historical.session!.id,
      parentSessionId: initial.session!.id,
      now: 23
    })

    expect(result.session).toMatchObject({ kind: 'shell', cwd: workspaceRoot })
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === result.session!.id))
      .toMatchObject({
        parentSessionId: initial.session!.id,
        relationKind: 'derived-from',
        currentMode: 'shell'
      })
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === historical.session!.id)?.archivedAt)
      .toBe(22)
    expect(result.graph.focusedSessionId).toBe(result.session!.id)
  })

  it('restarts a stopped Shell in the same graph node', () => {
    const initial = bootstrap()
    const historical = service.createShellSibling(command('historical-shell'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    insertStructuralRelation('historical-parent', historical.session!.id, initial.session!.id, 21)
    archiveSession(historical.session!.id, 22)

    const reopened = service.restartStoppedSession(command('restart-shell'), {
      windowId: 'window-1', sessionId: historical.session!.id, now: 23
    })

    expect(reopened.session).toMatchObject({ kind: 'shell', status: 'created' })
    expect(reopened.session!.id).toBe(historical.session!.id)
    expect(reopened.graph.nodes.find(({ sessionId }) => sessionId === historical.session!.id)?.archivedAt).toBeUndefined()
    expect(reopened.graph.nodes.find(({ sessionId }) => sessionId === historical.session!.id))
      .toMatchObject({ parentSessionId: initial.session!.id, relationKind: 'derived-from' })
    expect(reopened.graph.focusedSessionId).toBe(historical.session!.id)
  })

  it('restarts the same stopped Session after a closed canvas is restored', () => {
    const initial = bootstrap()
    service.createCanvas(command('second-canvas-before-close'), {
      windowId: 'window-1', taskId: initial.task!.id, now: 20
    })
    hierarchy.closeScene(command('close-canvas-for-history'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 21
    })
    hierarchy.reopenScene(command('restore-closed-canvas'), {
      windowId: 'window-1', sceneId: initial.scene!.id, now: 22
    })

    const reopened = service.restartStoppedSession(command('restart-first-stopped'), {
      windowId: 'window-1', sessionId: initial.session!.id, now: 23
    })

    expect(reopened.session).toMatchObject({ kind: 'shell', status: 'created' })
    expect(reopened.session!.id).toBe(initial.session!.id)
    expect(reopened.graph.focusedSessionId).toBe(initial.session!.id)
    expect(reopened.graph.nodes.find(({ sessionId }) => sessionId === initial.session!.id)?.archivedAt)
      .toBeUndefined()
  })

  it('removes an exited leaf from the canvas without changing its surviving parent', () => {
    const initial = bootstrap()
    const historical = service.createShellSibling(command('history-leaf'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    insertStructuralRelation('history-parent', historical.session!.id, initial.session!.id, 21)
    archiveSession(historical.session!.id, 22)

    const result = service.removeSessionBranch(command('remove-history-leaf'), {
      windowId: 'window-1', sceneId: initial.scene!.id, sessionId: historical.session!.id,
      scope: 'node-only', now: 23
    })

    expect(result.graph.nodes.map(({ sessionId }) => sessionId)).not.toContain(historical.session!.id)
    expect(result.graph.nodes.map(({ sessionId }) => sessionId)).toContain(initial.session!.id)
    expect(result.graph.focusedSessionId).toBe(initial.session!.id)
    expect(database.get('SELECT session_id FROM session_canvas_memberships WHERE session_id = ?', historical.session!.id))
      .toBeUndefined()
  })

  it('removes only an exited parent and reconnects its direct child to the surviving grandparent', () => {
    const initial = bootstrap()
    const parent = service.createShellSibling(command('history-parent-node'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    insertStructuralRelation('history-parent-edge', parent.session!.id, initial.session!.id, 21)
    const child = service.createShellSibling(command('history-child-node'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: parent.session!.id, parentSessionId: parent.session!.id, now: 22
    })
    archiveSession(parent.session!.id, 23)

    const result = service.removeSessionBranch(command('remove-parent-without-branch'), {
      windowId: 'window-1', sceneId: initial.scene!.id, sessionId: parent.session!.id,
      scope: 'node-only', now: 25
    })

    expect(result.removedSessionIds).toEqual([parent.session!.id])
    expect(result.disposedSessionIds).toEqual([])
    expect(result.graph.nodes.map(({ sessionId }) => sessionId)).not.toContain(parent.session!.id)
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === child.session!.id)).toMatchObject({
      parentSessionId: initial.session!.id,
      relationKind: 'derived-from'
    })
    expect(database.all<{ operation: string; from_session_id: string; to_session_id: string }>(
      `SELECT operation, from_session_id, to_session_id
       FROM session_relation_events
       WHERE command_id = 'remove-parent-without-branch'
       ORDER BY sequence`
    )).toEqual(expect.arrayContaining([
      { operation: 'revoked', from_session_id: parent.session!.id, to_session_id: initial.session!.id },
      { operation: 'revoked', from_session_id: child.session!.id, to_session_id: parent.session!.id },
      { operation: 'created', from_session_id: child.session!.id, to_session_id: initial.session!.id }
    ]))
  })

  it('removes an exited parent together with its complete descendant branch when requested', () => {
    const initial = bootstrap()
    const parent = service.createShellSibling(command('history-branch-parent-node'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    insertStructuralRelation('history-branch-parent-edge', parent.session!.id, initial.session!.id, 21)
    const child = service.createShellSibling(command('history-branch-child-node'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: parent.session!.id, parentSessionId: parent.session!.id, now: 22
    })
    archiveSession(parent.session!.id, 23)

    const result = service.removeSessionBranch(command('remove-whole-history-branch'), {
      windowId: 'window-1', sceneId: initial.scene!.id, sessionId: parent.session!.id,
      scope: 'node-and-descendants', now: 26
    })
    expect(result.removedSessionIds).toEqual(expect.arrayContaining([parent.session!.id, child.session!.id]))
    expect(result.disposedSessionIds).toEqual([child.session!.id])
    expect(result.graph.nodes.map(({ sessionId }) => sessionId)).not.toContain(parent.session!.id)
    expect(result.graph.nodes.map(({ sessionId }) => sessionId)).not.toContain(child.session!.id)
  })

  it('removes only an active root and promotes its direct child without breaking deeper descendants', () => {
    const initial = bootstrap()
    const child = service.createShellSibling(command('active-branch-child'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, parentSessionId: initial.session!.id, now: 20
    })
    const grandchild = service.createShellSibling(command('active-branch-grandchild'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: child.session!.id, parentSessionId: child.session!.id, now: 21
    })

    const result = service.removeSessionBranch(command('remove-active-parent-without-branch'), {
      windowId: 'window-1', sceneId: initial.scene!.id, sessionId: initial.session!.id,
      scope: 'node-only', now: 22
    })

    expect(result.removedSessionIds).toEqual([initial.session!.id])
    expect(result.disposedSessionIds).toEqual([initial.session!.id])
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === child.session!.id)?.parentSessionId)
      .toBeUndefined()
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === grandchild.session!.id))
      .toMatchObject({ parentSessionId: child.session!.id })
    expect(result.graph.focusedSessionId).toBe(grandchild.session!.id)
  })

  it('removes an active parent and its complete descendant branch from both projections', () => {
    const initial = bootstrap()
    const survivor = service.createShellSibling(command('active-whole-branch-survivor'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 19
    })
    const child = service.createShellSibling(command('active-whole-branch-child'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, parentSessionId: initial.session!.id, now: 20
    })
    const grandchild = service.createShellSibling(command('active-whole-branch-grandchild'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: child.session!.id, parentSessionId: child.session!.id, now: 21
    })

    const result = service.removeSessionBranch(command('remove-active-whole-branch'), {
      windowId: 'window-1', sceneId: initial.scene!.id, sessionId: initial.session!.id,
      scope: 'node-and-descendants', now: 23
    })
    expect(result.removedSessionIds).toEqual(expect.arrayContaining([
      initial.session!.id, child.session!.id, grandchild.session!.id
    ]))
    expect(result.disposedSessionIds).toEqual(expect.arrayContaining([
      initial.session!.id, child.session!.id, grandchild.session!.id
    ]))
    expect(result.graph.nodes.map(({ sessionId }) => sessionId)).toEqual([survivor.session!.id])
  })

  it('protects the final card from structural removal', () => {
    const initial = bootstrap()

    expect(() => service.removeSessionBranch(command('remove-final-card'), {
      windowId: 'window-1', sceneId: initial.scene!.id, sessionId: initial.session!.id,
      scope: 'node-only', now: 20
    })).toThrow('Scene must keep one Session')

    expect(service.projectSceneGraph(initial.scene!.id, 'window-1').nodes.map(({ sessionId }) => sessionId))
      .toEqual([initial.session!.id])
  })

  it('restores a durable Claude identity on the same stopped node', () => {
    const initial = bootstrap()
    const historical = service.createShellSibling(command('historical-claude'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    database.run("UPDATE sessions SET kind = 'claude-code', title = '方案分支' WHERE id = ?", historical.session!.id)
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, metadata_json,
         created_at, updated_at, validated_at, restore_state
       ) VALUES ('historical-binding', ?, 'claude-code', 'provider-history', 'available',
                 '{"canFork":true}', 20, 20, 20, 'none')`,
      historical.session!.id
    )
    archiveSession(historical.session!.id, 22)

    const reopened = service.restartStoppedSession(command('restart-claude'), {
      windowId: 'window-1', sessionId: historical.session!.id, now: 23
    })

    expect(reopened.session).toMatchObject({ kind: 'claude-code', title: '方案分支' })
    expect(reopened.session!.id).toBe(historical.session!.id)
    expect(database.get(
      `SELECT session_id, provider_session_id, resume_state, restore_state
       FROM provider_bindings WHERE id = 'historical-binding'`
    )).toEqual({
      session_id: reopened.session!.id,
      provider_session_id: 'provider-history',
      resume_state: 'available',
      restore_state: 'none'
    })
  })

  it('projects a real Claude Agent Teams teammate as a read-only child without stealing focus', () => {
    const initial = bootstrap()
    database.run(
      "UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?",
      initial.session!.id
    )
    const observe = (service as unknown as {
      upsertAgentTeamMember?: (
        mutation: { commandId: string; commandType: string; requestHash: string },
        input: {
          leadSessionId: string
          teammateId: string
          teamId: string
          name: string
          workStatus: 'running' | 'idle'
          latestLines: string[]
          now: number
        }
      ) => { graph: { focusedSessionId?: string; nodes: Array<Record<string, unknown>> } }
    }).upsertAgentTeamMember

    expect(observe).toBeTypeOf('function')
    const created = observe!.call(service, command('observe-real-teammate'), {
      leadSessionId: initial.session!.id,
      teammateId: 'MATOU_QA_TEAMMATE@session-real',
      teamId: 'session-real',
      name: 'MATOU_QA_TEAMMATE',
      workStatus: 'running',
      latestLines: ['TEAMMATE_REAL_READY'],
      now: 20
    })

    const teammate = created.graph.nodes.find(({ currentMode }) =>
      currentMode === 'agent-team-member'
    )
    expect(teammate).toMatchObject({
      parentSessionId: initial.session!.id,
      relationKind: 'derived-from',
      currentMode: 'agent-team-member',
      title: 'MATOU_QA_TEAMMATE',
      workStatus: 'running',
      latestLines: ['TEAMMATE_REAL_READY']
    })
    expect(created.graph.focusedSessionId).toBe(initial.session!.id)
    expect(created.graph.nodes.find(({ sessionId }) => sessionId === initial.session!.id))
      .toMatchObject({ activeChildCount: 1, childModeCounts: { shell: 0, claudeCode: 1 } })

    const updated = observe!.call(service, command('update-real-teammate'), {
      leadSessionId: initial.session!.id,
      teammateId: 'MATOU_QA_TEAMMATE@session-real',
      teamId: 'session-real',
      name: 'MATOU_QA_TEAMMATE',
      workStatus: 'idle',
      latestLines: ['TEAMMATE_REAL_READY', 'Teammate finished'],
      now: 21
    })
    expect(updated.graph.nodes.filter(({ currentMode }) =>
      currentMode === 'agent-team-member'
    )).toHaveLength(1)
    expect(updated.graph.nodes.find(({ currentMode }) =>
      currentMode === 'agent-team-member'
    )).toMatchObject({ workStatus: 'idle', latestLines: ['TEAMMATE_REAL_READY', 'Teammate finished'] })
  })
})

function bootstrap() {
  return hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'workspace', now: 10
  })
}

function command(commandId: string) {
  return { commandId, commandType: 'session-canvas', requestHash: `hash-${commandId}` }
}

function count(table: string, where = '1 = 1'): number {
  return database.get<{ count: number }>(
    `SELECT COUNT(*) AS count FROM ${table} WHERE ${where}`
  )!.count
}

function eventTypes(): string[] {
  return database.all<{ event_type: string }>(
    'SELECT event_type FROM domain_events ORDER BY seq'
  ).map(({ event_type }) => event_type)
}

function membershipSequence(sessionId: string): number {
  return database.get<{ sibling_created_seq: number }>(
    `SELECT sibling_created_seq FROM session_canvas_memberships WHERE session_id = ?`,
    sessionId
  )!.sibling_created_seq
}

function insertStructuralRelation(
  id: string,
  childSessionId: string,
  parentSessionId: string,
  now: number
): void {
  const insertion = database.run(
    `INSERT INTO session_relation_events (
       event_id, relation_id, operation, task_id, from_session_id, to_session_id,
       relation_kind, metadata_json, command_id, occurred_at
     ) SELECT ?, ?, 'created', task_id, ?, ?, 'derived-from', '{}', ?, ?
       FROM sessions WHERE id = ?`,
    `event-${id}`, `relation-${id}`, childSessionId, parentSessionId,
    `command-${id}`, now, childSessionId
  )
  database.run(
    `INSERT INTO session_relations_current (
       relation_id, task_id, from_session_id, to_session_id, relation_kind,
       metadata_json, created_at, updated_at, source_event_sequence
     ) SELECT ?, task_id, ?, ?, 'derived-from', '{}', ?, ?, ?
       FROM sessions WHERE id = ?`,
    `relation-${id}`, childSessionId, parentSessionId, now, now,
    Number(insertion.lastInsertRowid), childSessionId
  )
}

function archiveSession(sessionId: string, now: number): void {
  database.run(
    `UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ? WHERE id = ?`,
    now, now, sessionId
  )
  database.run('DELETE FROM session_mounts WHERE session_id = ?', sessionId)
}
