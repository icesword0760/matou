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
import { ProviderModeService } from './provider-mode-service'

let database: RuntimeDatabase
let providerModes: ProviderModeService
let canvas: SessionCanvasService
let hierarchy: HierarchyApplicationService
let workspaceRoot: string

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-provider-mode-'))
  workspaceRoot = join(root, 'workspace')
  await mkdir(workspaceRoot)
  database = RuntimeDatabase.open(join(root, 'data', 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  canvas = new SessionCanvasService(database, transactions)
  providerModes = new ProviderModeService(database, transactions)
})

afterEach(() => database.close())

describe('ProviderModeService', () => {
  it('returns a manually exited Claude node to ordinary Shell and preserves its children', () => {
    const initial = bootstrapClaudeTree()

    const result = providerModes.markUserExited(command('manual-exit'), {
      sessionId: initial.parentSessionId, now: 30
    })

    expect(result.session).toMatchObject({ kind: 'shell', title: 'Shell' })
    expect(result.binding).toMatchObject({
      providerSessionId: 'provider-parent',
      resumeState: 'expired',
      restoreState: 'none',
      userExitedAt: 30
    })
    expect(result.binding.restoreError).toBeUndefined()
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ currentMode: 'shell', activeChildCount: 1, canFork: false })
    expect(result.graph.edges).toContainEqual(expect.objectContaining({
      parentSessionId: initial.parentSessionId,
      childSessionId: initial.childSessionId
    }))
  })

  it('keeps restore failure as a retryable Shell state on the same graph node', () => {
    const initial = bootstrapClaudeTree()

    const failed = providerModes.markRestoreFailed(command('restore-failed'), {
      sessionId: initial.parentSessionId,
      bindingId: 'binding-parent',
      reason: 'provider session not found',
      now: 30
    })

    expect(failed.session).toMatchObject({ kind: 'shell', title: 'Shell' })
    expect(failed.binding).toMatchObject({
      resumeState: 'failed',
      restoreState: 'failed',
      restoreError: 'provider session not found'
    })
    expect(failed.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ currentMode: 'shell', providerRestoreState: 'failed', activeChildCount: 1 })
    expect(latestRecoveryNotification(initial.parentSessionId)).toMatchObject({
      eventType: 'error',
      title: 'Claude Code 恢复失败',
      body: 'provider session not found',
      replacementKey: `provider-restore:${initial.parentSessionId}`
    })

    const retrying = providerModes.retryRestore(command('restore-retry'), {
      sessionId: initial.parentSessionId, now: 31
    })
    expect(retrying.session).toMatchObject({ kind: 'claude-code', title: 'Claude' })
    expect(retrying.binding).toMatchObject({
      id: 'binding-parent', providerSessionId: 'provider-parent',
      resumeState: 'available', restoreState: 'restoring'
    })
    expect(retrying.graph.edges).toEqual(failed.graph.edges)
    expect(latestRecoveryNotification(initial.parentSessionId)).toMatchObject({
      eventType: 'attention',
      title: '正在恢复 Claude Code',
      sound: false,
      replacementKey: `provider-restore:${initial.parentSessionId}`
    })

    providerModes.markClaudeActive(command('restore-succeeded'), {
      sessionId: initial.parentSessionId, bindingId: 'binding-parent', now: 32
    })
    expect(latestRecoveryNotification(initial.parentSessionId)).toMatchObject({
      operation: 'dismiss',
      replacementKey: `provider-restore:${initial.parentSessionId}`
    })
  })

  it('enables Fork only after a real prompt, durable identity, and normal Stop', () => {
    const initial = bootstrapClaudeTree({ canFork: false })

    const prompted = providerModes.observeHook(command('hook-prompt'), {
      sessionId: initial.parentSessionId,
      providerSessionId: 'provider-parent',
      eventName: 'UserPromptSubmit',
      now: 30
    })
    expect(prompted.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ currentMode: 'claude-code', canFork: false })

    const stopped = providerModes.observeHook(command('hook-stop'), {
      sessionId: initial.parentSessionId,
      providerSessionId: 'provider-parent',
      eventName: 'Stop',
      now: 31
    })
    expect(stopped.binding.metadata).toMatchObject({
      observedUserPrompt: true,
      observedNormalStop: true,
      canFork: true
    })
    expect(stopped.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ currentMode: 'claude-code', canFork: true })
  })

  it('replays a transition without incrementing versions or emitting a second event', () => {
    const initial = bootstrapClaudeTree()
    const input = { sessionId: initial.parentSessionId, now: 30 }

    const first = providerModes.markUserExited(command('manual-exit-replay'), input)
    const replay = providerModes.markUserExited(command('manual-exit-replay'), input)

    expect(replay).toEqual(first)
    expect(database.get<{ count: number }>(
      `SELECT COUNT(*) AS count FROM domain_events
       WHERE event_type = 'session.mode-changed' AND session_id = ?`,
      initial.parentSessionId
    )?.count).toBe(1)
  })
})

function bootstrapClaudeTree(options: { canFork?: boolean } = {}) {
  const initial = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'workspace', now: 10
  })
  const child = canvas.createShellSibling(command('child-shell'), {
    windowId: 'window-1', sceneId: initial.scene!.id,
    sourceSessionId: initial.session!.id, now: 20
  })
  const relationEvent = database.run(
    `INSERT INTO session_relation_events (
       event_id, relation_id, operation, task_id, from_session_id, to_session_id,
       relation_kind, metadata_json, command_id, occurred_at
     ) VALUES ('tree-event', 'tree-relation', 'created', ?, ?, ?,
               'derived-from', '{}', 'tree-command', 21)`,
    initial.task!.id, child.session!.id, initial.session!.id
  )
  database.run(
    `INSERT INTO session_relations_current (
       relation_id, task_id, from_session_id, to_session_id, relation_kind,
       metadata_json, created_at, updated_at, source_event_sequence
     ) VALUES ('tree-relation', ?, ?, ?, 'derived-from', '{}', 21, 21, ?)`,
    initial.task!.id, child.session!.id, initial.session!.id,
    Number(relationEvent.lastInsertRowid)
  )
  database.run(
    `UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?`,
    initial.session!.id
  )
  database.run(
    `INSERT INTO provider_bindings (
       id, session_id, provider, provider_session_id, resume_state, restore_state,
       metadata_json, created_at, updated_at, validated_at
     ) VALUES ('binding-parent', ?, 'claude-code', 'provider-parent', 'available', 'none',
               ?, 22, 22, 22)`,
    initial.session!.id,
    JSON.stringify({ canFork: options.canFork ?? true })
  )
  return {
    sceneId: initial.scene!.id,
    parentSessionId: initial.session!.id,
    childSessionId: child.session!.id
  }
}

function command(commandId: string) {
  return { commandId, commandType: 'provider-mode', requestHash: `hash-${commandId}` }
}

function latestRecoveryNotification(sessionId: string): Record<string, unknown> | undefined {
  const row = database.get<{ payload_json: string }>(
     `SELECT payload_json FROM domain_events
     WHERE event_type = 'agent.notification' AND session_id = ?
     ORDER BY seq DESC LIMIT 1`,
    sessionId
  )
  if (!row) return undefined
  const payload = JSON.parse(row.payload_json) as { event?: Record<string, unknown> }
  return payload.event
}
