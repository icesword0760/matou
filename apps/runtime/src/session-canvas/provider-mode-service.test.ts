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
  it('loads an existing provider conversation into the same focused graph node atomically', () => {
    const initial = bootstrapClaudeTree()
    const focusedBefore = database.get<{ active_session_id: string }>(
      'SELECT active_session_id FROM window_scene_focus WHERE scene_id = ?', initial.sceneId
    )

    const result = providerModes.loadClaudeSession(command('load-existing'), {
      sessionId: initial.childSessionId,
      bindingId: 'binding-loaded',
      providerSessionId: 'provider-loaded',
      title: '通知中心聚合',
      permissionMode: 'bypassPermissions',
      model: 'claude-opus-4-6',
      now: 30
    })

    expect(result.session).toMatchObject({
      id: initial.childSessionId, kind: 'claude-code', title: '通知中心聚合'
    })
    expect(result.binding).toMatchObject({
      id: 'binding-loaded', sessionId: initial.childSessionId,
      providerSessionId: 'provider-loaded', restoreState: 'restoring',
      metadata: expect.objectContaining({
        permissionMode: 'bypassPermissions', model: 'claude-opus-4-6', loadedFromCatalog: true
      })
    })
    expect(result.graph.edges).toContainEqual(expect.objectContaining({
      parentSessionId: initial.parentSessionId,
      childSessionId: initial.childSessionId
    }))
    expect(database.get(
      'SELECT active_session_id FROM window_scene_focus WHERE scene_id = ?', initial.sceneId
    )).toEqual(focusedBefore)
  })

  it('loads a conversation already active in another Claude card while preserving both cards', () => {
    const initial = bootstrapClaudeTree()

    const result = providerModes.loadClaudeSession(command('load-duplicate'), {
      sessionId: initial.childSessionId,
      bindingId: 'binding-duplicate',
      providerSessionId: 'provider-parent',
      title: '共享会话',
      permissionMode: 'default',
      now: 30
    })

    expect(result.session).toMatchObject({ id: initial.childSessionId, title: '共享会话' })
    expect(database.all<{ session_id: string }>(
      `SELECT session_id FROM provider_bindings
       WHERE provider = 'claude-code' AND provider_session_id = ? ORDER BY session_id`,
      'provider-parent'
    ).map(({ session_id }) => session_id).sort()).toEqual([
      initial.childSessionId, initial.parentSessionId
    ].sort())
  })

  it('records a loaded Claude title without replacing a card the user renamed', () => {
    const initial = bootstrapClaudeTree()
    database.run(
      `UPDATE sessions SET title = '我的排查窗口', title_source = 'manual' WHERE id = ?`,
      initial.childSessionId
    )

    const result = providerModes.loadClaudeSession(command('load-with-manual-title'), {
      sessionId: initial.childSessionId,
      bindingId: 'binding-manual-title',
      providerSessionId: 'provider-loaded-title',
      title: 'Claude 自动生成标题',
      permissionMode: 'default',
      now: 30
    })

    expect(result.session).toMatchObject({
      title: '我的排查窗口', titleSource: 'manual', providerTitle: 'Claude 自动生成标题'
    })
  })

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
      .toMatchObject({ currentMode: 'shell', workStatus: 'idle', activeChildCount: 1, canFork: false })
    expect(result.graph.edges).toContainEqual(expect.objectContaining({
      parentSessionId: initial.parentSessionId,
      childSessionId: initial.childSessionId
    }))
  })

  it('keeps restore failure as a retryable card state without creating a global notification', () => {
    const initial = bootstrapClaudeTree()

    const failed = providerModes.markRestoreFailed(command('restore-failed'), {
      sessionId: initial.parentSessionId,
      bindingId: 'binding-parent',
      reason: 'provider session not found',
      now: 30
    })

    expect(failed.session).toMatchObject({ kind: 'claude-code', title: 'Claude' })
    expect(failed.binding).toMatchObject({
      resumeState: 'failed',
      restoreState: 'failed',
      restoreError: 'provider session not found'
    })
    expect(failed.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({
        currentMode: 'claude-code', workStatus: 'error',
        providerRestoreState: 'failed', activeChildCount: 1
      })
    expect(latestRecoveryNotification(initial.parentSessionId)).toBeUndefined()

    const retrying = providerModes.retryRestore(command('restore-retry'), {
      sessionId: initial.parentSessionId, now: 31
    })
    expect(retrying.session).toMatchObject({ kind: 'claude-code', title: 'Claude' })
    expect(retrying.binding).toMatchObject({
      id: 'binding-parent', providerSessionId: 'provider-parent',
      resumeState: 'available', restoreState: 'restoring',
      metadata: expect.objectContaining({ spawnRevision: 31 })
    })
    expect(retrying.graph.edges).toEqual(failed.graph.edges)
    expect(latestRecoveryNotification(initial.parentSessionId)).toMatchObject({
      operation: 'dismiss',
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

  it('starts fresh only by explicit action while keeping the same visible node', () => {
    const initial = bootstrapClaudeTree()
    providerModes.markRestoreFailed(command('fresh-failed'), {
      sessionId: initial.parentSessionId, bindingId: 'binding-parent',
      reason: 'provider session not found', now: 30
    })

    const fresh = providerModes.startFreshClaude(command('fresh-start'), {
      sessionId: initial.parentSessionId, now: 31
    })

    expect(fresh.session).toMatchObject({ kind: 'claude-code', title: 'Claude' })
    expect(fresh.binding).toMatchObject({
      id: 'binding-parent', resumeState: 'expired', restoreState: 'none',
      metadata: expect.objectContaining({ spawnRevision: 31 })
    })
    expect(database.get(
      `SELECT 1 FROM provider_bindings WHERE session_id = ? AND resume_state = 'available'`,
      initial.parentSessionId
    )).toBeUndefined()
    expect(fresh.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ currentMode: 'claude-code', providerRestoreState: 'none' })
    expect(latestRecoveryNotification(initial.parentSessionId)).toMatchObject({
      operation: 'dismiss', replacementKey: `provider-restore:${initial.parentSessionId}`
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
      .toMatchObject({ currentMode: 'claude-code', workStatus: 'running', canFork: false })

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
      .toMatchObject({ currentMode: 'claude-code', workStatus: 'idle', canFork: true })
  })

  it('projects an explicit provider permission request as needs-input until work resumes', () => {
    const initial = bootstrapClaudeTree({ canFork: false })
    const waiting = providerModes.observeHook(command('hook-permission'), {
      sessionId: initial.parentSessionId,
      providerSessionId: 'provider-parent',
      eventName: 'PermissionRequest',
      now: 30
    })
    expect(waiting.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ workStatus: 'needs-input' })

    const resumed = providerModes.observeHook(command('hook-pre-tool'), {
      sessionId: initial.parentSessionId,
      providerSessionId: 'provider-parent',
      eventName: 'PreToolUse',
      now: 31
    })
    expect(resumed.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ workStatus: 'running' })
  })

  it('settles a restored Claude session to idle when its statusline confirms the live conversation', () => {
    const initial = bootstrapClaudeTree({ canFork: true })
    database.run(
      `UPDATE sessions SET status = 'running', work_status = 'starting' WHERE id = ?`,
      initial.parentSessionId
    )

    const restored = providerModes.observeHook(command('hook-restored-statusline'), {
      sessionId: initial.parentSessionId,
      providerSessionId: 'provider-parent',
      eventName: 'unknown',
      now: 30
    })

    expect(restored.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ currentMode: 'claude-code', workStatus: 'idle', canFork: true })
  })

  it('dismisses an obsolete restore failure when a replacement Claude identity becomes live', () => {
    const initial = bootstrapClaudeTree({ canFork: false })
    providerModes.markRestoreFailed(command('old-restore-failed'), {
      sessionId: initial.parentSessionId,
      bindingId: 'binding-parent',
      reason: 'provider session not found',
      now: 30
    })
    database.run(
      `UPDATE sessions SET kind = 'claude-code', work_status = 'starting' WHERE id = ?`,
      initial.parentSessionId
    )
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, restore_state,
         metadata_json, created_at, updated_at, validated_at
       ) VALUES ('binding-replacement', ?, 'claude-code', 'provider-replacement',
                 'available', 'none', '{}', 31, 31, 31)`,
      initial.parentSessionId
    )

    const active = providerModes.observeHook(command('replacement-statusline'), {
      sessionId: initial.parentSessionId,
      providerSessionId: 'provider-replacement',
      eventName: 'unknown',
      now: 32
    })

    expect(active.graph.nodes.find(({ sessionId }) => sessionId === initial.parentSessionId))
      .toMatchObject({ currentMode: 'claude-code', providerRestoreState: 'none', workStatus: 'idle' })
    expect(database.get(
      `SELECT resume_state, restore_state, restore_error FROM provider_bindings
       WHERE id = 'binding-parent'`
    )).toEqual({ resume_state: 'expired', restore_state: 'none', restore_error: null })
    expect(latestRecoveryNotification(initial.parentSessionId)).toMatchObject({
      operation: 'dismiss',
      replacementKey: `provider-restore:${initial.parentSessionId}`
    })
  })

  it('preserves the user-visible node name across Claude recovery and mode transitions', () => {
    const initial = bootstrapClaudeTree({ canFork: false })
    database.run(`UPDATE sessions SET title = 'same-visible-name' WHERE id = ?`, initial.parentSessionId)

    expect(providerModes.markRestoreFailed(command('named-failed'), {
      sessionId: initial.parentSessionId, bindingId: 'binding-parent', reason: 'missing', now: 30
    }).session.title).toBe('same-visible-name')
    expect(providerModes.retryRestore(command('named-retry'), {
      sessionId: initial.parentSessionId, now: 31
    }).session.title).toBe('same-visible-name')
    expect(providerModes.markClaudeActive(command('named-active'), {
      sessionId: initial.parentSessionId, bindingId: 'binding-parent', now: 32
    }).session.title).toBe('same-visible-name')
    expect(providerModes.observeHook(command('named-hook'), {
      sessionId: initial.parentSessionId, providerSessionId: 'provider-parent',
      eventName: 'UserPromptSubmit', now: 33
    }).session.title).toBe('same-visible-name')
    expect(providerModes.markUserExited(command('named-exit'), {
      sessionId: initial.parentSessionId, now: 34
    }).session.title).toBe('same-visible-name')
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
