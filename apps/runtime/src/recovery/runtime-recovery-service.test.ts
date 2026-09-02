import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { SegmentJournal, readSessionFrames } from '../journal/segment-journal'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { RuntimeRecoveryService } from './runtime-recovery-service'

const databases: RuntimeDatabase[] = []
afterEach(() => {
  for (const database of databases.splice(0)) database.close()
})

describe('RuntimeRecoveryService', () => {
  it('repairs crash windows after restart and isolates one corrupt Session', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-runtime-recovery-'))
    const databasePath = join(root, 'matou.sqlite')
    const before = RuntimeDatabase.open(databasePath)
    databases.push(before)
    await new MigrationRunner(before, FOUNDATION_MIGRATIONS).migrate()
    seed(before, ['session-good', 'session-corrupt'])
    before.run(
      `INSERT INTO session_runs (
         id, session_id, ordinal, runtime_generation, profile, status,
         cols, rows, started_at
       ) VALUES (?, ?, 1, ?, 'shell', 'running', 80, 24, 2)`,
      'run-stale', 'session-good', before.runtimeGeneration
    )
    before.run(
      `INSERT INTO session_runs (
         id, session_id, ordinal, runtime_generation, profile, status,
         cols, rows, started_at
       ) VALUES (?, ?, 1, ?, 'shell', 'running', 80, 24, 2)`,
      'run-stale-corrupt', 'session-corrupt', before.runtimeGeneration
    )

    const good = await SegmentJournal.open(root, 'session-good')
    await good.appendOutput(1, Uint8Array.from([65]))
    await good.close()
    new DomainTransactionManager(before).execute(
      { commandId: 'event-without-marker', commandType: 'agent.todo', requestHash: 'hash' },
      ({ emit }) => {
        emit({
          eventId: 'todo-1', eventType: 'agent.todo', aggregateType: 'session',
          aggregateId: 'session-good', sessionId: 'session-good', taskId: 'task-1',
          payload: { text: 'recover me' }, requiredTerminalSequence: 1, occurredAt: 10
        })
        return null
      }
    )

    const corrupt = await SegmentJournal.open(root, 'session-corrupt')
    await corrupt.appendOutput(1, Uint8Array.from([66]))
    await corrupt.appendOutput(2, Uint8Array.from([67]))
    const corruptPath = corrupt.path
    await corrupt.close()
    const bytes = await readFile(corruptPath)
    bytes[20] = bytes[20]! ^ 0xff
    await writeFile(corruptPath, bytes)

    const priorGeneration = before.runtimeGeneration
    before.close()
    databases.splice(databases.indexOf(before), 1)
    const after = RuntimeDatabase.open(databasePath)
    databases.push(after)
    await new MigrationRunner(after, FOUNDATION_MIGRATIONS).migrate()

    const report = await new RuntimeRecoveryService(root, after).recoverAll()

    expect(after.runtimeGeneration).not.toBe(priorGeneration)
    expect(report.interruptedRuns).toEqual(['run-stale', 'run-stale-corrupt'])
    expect(after.get('SELECT status FROM session_runs WHERE id = ?', 'run-stale')).toEqual({
      status: 'interrupted'
    })
    expect(after.get('SELECT status FROM session_runs WHERE id = ?', 'run-stale-corrupt')).toEqual({
      status: 'interrupted'
    })
    expect(report.recovered).toEqual([
      expect.objectContaining({ sessionId: 'session-good', repairedAlignment: true })
    ])
    expect(report.failed).toEqual([
      expect.objectContaining({ sessionId: 'session-corrupt', code: 'JOURNAL_CORRUPT' })
    ])
    expect(await readSessionFrames(root, 'session-good')).toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65]) },
      { kind: 'domain-cursor', sequence: 2, domainEventSequence: 2 }
    ])

    const repeated = await new RuntimeRecoveryService(root, after).recoverAll()
    expect(repeated.interruptedRuns).toEqual([])
    expect(repeated.recovered).toEqual([
      expect.objectContaining({ sessionId: 'session-corrupt', repairedAlignment: true }),
      expect.objectContaining({ sessionId: 'session-good', repairedAlignment: false })
    ])
    expect(repeated.failed).toEqual([])
  })

  it('plans restart recovery by current card, foreground list, task, workspace, then background', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-layered-recovery-'))
    const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
    databases.push(database)
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    seedRecoveryHierarchy(database)

    const jobs = new RuntimeRecoveryService(root, database).planSessionRecovery()

    expect(jobs.map(({ sessionId, priority }) => [sessionId, priority])).toEqual([
      ['current', 'active-session'],
      ['foreground-offscreen', 'foreground-scene'],
      ['same-scene-other-level', 'active-task'],
      ['durable-fork', 'active-task'],
      ['same-task', 'active-task'],
      ['same-workspace', 'active-workspace'],
      ['background', 'background']
    ])
    expect(jobs.every(({ executionContextId, profile }) =>
      typeof executionContextId === 'string' &&
      executionContextId.startsWith('context-') &&
      (profile === 'shell' || profile === 'claude-code'))).toBe(true)
    expect(jobs.find(({ sessionId }) => sessionId === 'durable-fork'))
      .toMatchObject({ recoveryAuthority: 'fork' })
    expect(jobs.some(({ sessionId }) => sessionId === 'explicitly-stopped')).toBe(false)
    expect(jobs.some(({ sessionId }) => sessionId === 'deleted')).toBe(false)
  })

  it('describes a newly accepted durable Fork before its provider launch changes work status', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-layered-recovery-live-fork-'))
    const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
    databases.push(database)
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    seedRecoveryHierarchy(database)
    database.run("UPDATE sessions SET work_status = 'idle' WHERE id = 'durable-fork'")

    const service = new RuntimeRecoveryService(root, database)
    expect(service.planSessionRecovery().some(({ sessionId }) => sessionId === 'durable-fork')).toBe(false)
    expect(service.describeExternalForkRecovery('durable-fork')).toMatchObject({
      sessionId: 'durable-fork',
      sceneId: 'scene-active',
      taskId: 'task-active',
      workspaceId: 'workspace-active',
      executionContextId: 'context-workspace-active',
      profile: 'claude-code',
      recoveryAuthority: 'fork'
    })
  })

})

function seed(database: RuntimeDatabase, sessionIds: string[]): void {
  database.transaction((tx) => {
    tx.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'workspace-1', 'Workspace', '/tmp/workspace', 1, 1
    )
    tx.run(
      'INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)',
      'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1
    )
    tx.run(
      'INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
      'task-1', 'workspace-1', 'context-1', 'Task', 'active', 1, 1
    )
    for (const sessionId of sessionIds) {
      tx.run(
        'INSERT INTO sessions (id, task_id, execution_context_id, kind, status, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)',
        sessionId, 'task-1', 'context-1', 'shell', 'running', 1, 1, 1
      )
    }
  })
}


function seedRecoveryHierarchy(database: RuntimeDatabase): void {
  database.transaction((tx) => {
    for (const workspaceId of ['workspace-active', 'workspace-background']) {
      tx.run(
        'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, 1, 1)',
        workspaceId, workspaceId, `/tmp/${workspaceId}`
      )
      tx.run(
        'INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, 1)',
        `context-${workspaceId}`, workspaceId, 'plain-directory', `/tmp/${workspaceId}`
      )
    }
    const tasks = [
      ['task-active', 'workspace-active'],
      ['task-same-workspace', 'workspace-active'],
      ['task-background', 'workspace-background']
    ] as const
    for (const [taskId, workspaceId] of tasks) {
      tx.run(
        'INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 1, 1)',
        taskId, workspaceId, `context-${workspaceId}`, taskId, 'active'
      )
    }
    const scenes = [
      ['scene-active', 'task-active'],
      ['scene-same-task', 'task-active'],
      ['scene-same-workspace', 'task-same-workspace'],
      ['scene-background', 'task-background']
    ] as const
    for (const [sceneId, taskId] of scenes) {
      tx.run(
        'INSERT INTO scenes (id, task_id, name, mode, created_at, updated_at) VALUES (?, ?, ?, ?, 1, 1)',
        sceneId, taskId, sceneId, 'card'
      )
    }
    const sessions = [
      ['current', 'task-active', 'scene-active', 'interrupted', null],
      ['foreground-offscreen', 'task-active', 'scene-active', 'running', null],
      ['same-scene-other-level', 'task-active', 'scene-active', 'running', null],
      ['durable-fork', 'task-active', 'scene-active', 'starting', null],
      ['parent-a', 'task-active', 'scene-active', 'exited', null],
      ['parent-b', 'task-active', 'scene-active', 'exited', null],
      ['same-task', 'task-active', 'scene-same-task', 'needs-input', null],
      ['same-workspace', 'task-same-workspace', 'scene-same-workspace', 'starting', null],
      ['background', 'task-background', 'scene-background', 'interrupted', null],
      ['explicitly-stopped', 'task-active', 'scene-active', 'exited', null],
      ['deleted', 'task-active', 'scene-active', 'interrupted', 9]
    ] as const
    let ordinal = 0
    for (const [sessionId, taskId, sceneId, workStatus, archivedAt] of sessions) {
      const workspaceId = taskId === 'task-background' ? 'workspace-background' : 'workspace-active'
      tx.run(
        `INSERT INTO sessions (
           id, task_id, execution_context_id, kind, status, created_at, updated_at,
           last_activity_at, archived_at, work_status
         ) VALUES (?, ?, ?, 'shell', ?, ?, 1, ?, ?, ?)`,
        sessionId, taskId, `context-${workspaceId}`,
        workStatus === 'exited' ? 'exited' : 'interrupted',
        ++ordinal, ordinal, archivedAt, workStatus
      )
      tx.run(
        'INSERT INTO session_mounts (id, scene_id, session_id, created_at) VALUES (?, ?, ?, ?)',
        `mount-${sessionId}`, sceneId, sessionId, ordinal
      )
    }
    tx.run("UPDATE sessions SET kind = 'claude-code' WHERE id = 'durable-fork'")
    for (const [childId, parentId] of [
      ['current', 'parent-a'],
      ['foreground-offscreen', 'parent-a'],
      ['same-scene-other-level', 'parent-b'],
      ['durable-fork', 'parent-b']
    ] as const) {
      const event = tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id, to_session_id,
           relation_kind, command_id, occurred_at
         ) VALUES (?, ?, 'created', 'task-active', ?, ?, 'derived-from', ?, 10)`,
        `event-${childId}`, `relation-${childId}`, childId, parentId, `command-${childId}`
      )
      tx.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           created_at, updated_at, source_event_sequence
         ) VALUES (?, 'task-active', ?, ?, 'derived-from', 10, 10, ?)`,
        `relation-${childId}`, childId, parentId, Number(event.lastInsertRowid)
      )
    }
    tx.run(
      `INSERT INTO session_fork_intents (
         session_id, source_session_id, source_provider, source_provider_session_id,
         state, created_at, display_name, worktree_mode, attempt_count, updated_at,
         operation_id, submission_key, stage, completed_steps, total_steps, attempt,
         lease_fence
       ) VALUES (
         'durable-fork', 'parent-b', 'claude-code', 'provider-parent', 'starting', 10,
         'Durable Fork', 'current', 0, 10, 'operation-durable', 'submission-durable',
         'restoring-provider', 1, 2, 0, 0
       )`
    )
    tx.run("INSERT INTO app_windows (id, kind, state, created_at, updated_at) VALUES ('window-main', 'main', 'visible', 1, 10)")
    tx.run("INSERT INTO window_navigation (window_id, active_workspace_id, updated_at) VALUES ('window-main', 'workspace-active', 10)")
    tx.run("INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at) VALUES ('window-main', 'workspace-active', 'task-active', 10)")
    tx.run("INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at) VALUES ('window-main', 'task-active', 'scene-active', 10)")
    tx.run("INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at) VALUES ('window-main', 'scene-active', 'current', 10)")
  })
}
