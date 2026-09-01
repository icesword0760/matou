import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspaceTaskRepository } from './workspace-task-repository'
import { SessionRepository } from './session-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let sessions: SessionRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-session-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), { id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1 })
  workspaces.createPlainExecutionContext(command('context'), { id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 1 })
  workspaces.createTask(command('task'), { id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1', title: 'Task', status: 'active', sortKey: 'a', now: 1 })
  sessions = new SessionRepository(database, transactions)
})

afterEach(() => database.close())

describe('SessionRepository', () => {
  it('returns a completed Agent panel to Shell without invalidating its resumable identity', () => {
    seedSession()
    sessions.recordResumableProviderIdentity(command('binding'), {
      id: 'binding-1', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-1', metadata: { permissionMode: 'default' }, now: 3
    })

    sessions.returnAgentToShell(command('return-shell'), 'session-1', 4)

    expect(sessions.getSession('session-1')).toMatchObject({ kind: 'shell', title: 'Shell' })
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'provider-1', resumeState: 'available'
    })
  })

  it('promotes the same Shell panel identity when the user starts Claude', () => {
    sessions.createSession(command('shell-session'), {
      id: 'shell-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'shell', title: 'Shell', now: 2
    })

    sessions.promoteShellToAgent(command('promote-agent'), 'shell-1', 'claude-code', 3)

    expect(sessions.getSession('shell-1')).toMatchObject({
      id: 'shell-1', kind: 'claude-code', title: 'Claude'
    })
  })

  it('keeps a named branch title while its panel changes between Shell and Claude', () => {
    sessions.createSession(command('named-shell-session'), {
      id: 'named-shell-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'shell', title: '修复登录', now: 2
    })

    sessions.promoteShellToAgent(command('promote-named-agent'), 'named-shell-1', 'claude-code', 3)
    expect(sessions.getSession('named-shell-1')).toMatchObject({
      kind: 'claude-code', title: '修复登录'
    })

    sessions.returnAgentToShell(command('return-named-shell'), 'named-shell-1', 4)
    expect(sessions.getSession('named-shell-1')).toMatchObject({ kind: 'shell', title: '修复登录' })
  })

  it('keeps a manual title above later Claude titles and restores the latest Claude title on request', () => {
    seedSession()

    sessions.observeProviderTitle(command('auto-title-1'), {
      sessionId: 'session-1', title: '修复卡片标题同步', now: 3
    })
    expect(sessions.getSession('session-1')).toMatchObject({
      title: '修复卡片标题同步', titleSource: 'auto', providerTitle: '修复卡片标题同步'
    })

    sessions.updateSession(command('manual-title'), {
      id: 'session-1', title: '我的发布排查', titleSource: 'manual', now: 4
    })
    sessions.observeProviderTitle(command('auto-title-2'), {
      sessionId: 'session-1', title: '排查发布流水线失败', now: 5
    })
    expect(sessions.getSession('session-1')).toMatchObject({
      title: '我的发布排查', titleSource: 'manual', providerTitle: '排查发布流水线失败'
    })

    sessions.restoreProviderTitle(command('restore-auto-title'), {
      sessionId: 'session-1', now: 6
    })
    expect(sessions.getSession('session-1')).toMatchObject({
      title: '排查发布流水线失败', titleSource: 'auto', providerTitle: '排查发布流水线失败'
    })
  })

  it('keeps the manual title when Claude has not produced an automatic title yet', () => {
    seedSession()
    sessions.renameSession(command('manual-before-auto'), {
      sessionId: 'session-1', title: '我的临时标题', now: 3
    })

    sessions.restoreProviderTitle(command('restore-before-auto'), {
      sessionId: 'session-1', now: 4
    })

    expect(sessions.getSession('session-1')).toMatchObject({
      title: '我的临时标题', titleSource: 'manual'
    })
  })

  it('keeps logical Session identity across multiple process runs', () => {
    sessions.createSession(command('session'), {
      id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Claude', now: 2
    })
    const first = sessions.startRun(command('run-1'), {
      id: 'run-1', sessionId: 'session-1', runtimeGeneration: 'generation-1',
      profile: 'claude-code', pid: 101, cols: 80, rows: 24, now: 3
    }).result
    sessions.finishRun(command('finish-1'), 'run-1', { exitCode: 1, now: 4 })
    const second = sessions.startRun(command('run-2'), {
      id: 'run-2', sessionId: 'session-1', runtimeGeneration: 'generation-2',
      profile: 'claude-code', pid: 202, cols: 100, rows: 30, now: 5
    }).result

    expect(first).toMatchObject({ ordinal: 1, pid: 101 })
    expect(second).toMatchObject({ ordinal: 2, pid: 202 })
    expect(sessions.getSession('session-1')).toMatchObject({
      id: 'session-1', status: 'running', version: 4
    })
    expect(sessions.listRuns('session-1').map(({ id }) => id)).toEqual(['run-1', 'run-2'])
  })

  it('uses only validated, non-invalidated provider bindings for resume', () => {
    seedSession()
    sessions.bindProvider(command('binding-old'), {
      id: 'binding-old', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-old', metadata: {}, now: 3
    })
    sessions.bindProvider(command('binding-new'), {
      id: 'binding-new', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-new', metadata: { cwd: '/tmp/workspace' }, now: 4
    })
    sessions.validateProviderBinding(command('validate-old'), 'binding-old', 5)
    sessions.validateProviderBinding(command('validate-new'), 'binding-new', 6)
    sessions.invalidateProviderBinding(command('invalidate-new'), 'binding-new', 'resume failed', 7)

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      id: 'binding-old', providerSessionId: 'provider-old', validatedAt: 5
    })
    expect(sessions.listProviderBindings('session-1')).toHaveLength(2)
  })

  it('does not let an empty provider identity overwrite the last resumable conversation', () => {
    seedSession()
    sessions.bindProvider(command('binding-valid'), {
      id: 'binding-valid', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-valid', metadata: {}, now: 3
    })
    sessions.validateProviderBinding(command('validate-valid'), 'binding-valid', 4)

    expect(() => sessions.bindProvider(command('binding-empty'), {
      id: 'binding-empty', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: '   ', metadata: {}, now: 5
    })).toThrow('Provider session identity must not be empty')
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      id: 'binding-valid', providerSessionId: 'provider-valid'
    })
  })

  it('records a hook-confirmed provider identity as resumable in one transaction', () => {
    seedSession()

    const recorded = sessions.recordResumableProviderIdentity(command('hook-identity'), {
      id: 'binding-hook', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: ' provider-hook-1 ',
      metadata: { permissionMode: 'bypassPermissions', cwd: '/tmp/workspace' }, now: 3
    })

    expect(recorded.result).toMatchObject({
      id: 'binding-hook', sessionId: 'session-1', providerSessionId: 'provider-hook-1',
      resumeState: 'available', validatedAt: 3, metadata: {
        permissionMode: 'bypassPermissions', cwd: '/tmp/workspace'
      }
    })
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      id: 'binding-hook', providerSessionId: 'provider-hook-1'
    })
    expect(recorded.firstEventSequence).toBe(recorded.lastEventSequence)
    expect(database.get<{ event_type: string }>(
      'SELECT event_type FROM domain_events WHERE seq = ?', recorded.firstEventSequence!
    )).toEqual({ event_type: 'provider-binding.recorded' })
  })

  it('settles a one-shot fork intent when the derived provider identity arrives', () => {
    seedSession()
    sessions.createSession(command('fork-source'), {
      id: 'source-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Source', now: 2
    })
    database.run(
      `INSERT INTO session_fork_intents (
         session_id, source_session_id, source_provider, source_provider_session_id,
         state, created_at, started_at
       ) VALUES (?, ?, 'claude-code', ?, 'starting', 2, 3)`,
      'session-1', 'source-1', 'provider-source'
    )

    sessions.recordResumableProviderIdentity(command('fork-hook-identity'), {
      id: 'binding-forked', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-derived', metadata: {}, now: 4
    })

    expect(database.get(
      'SELECT state, completed_at FROM session_fork_intents WHERE session_id = ?', 'session-1'
    )).toEqual({ state: 'succeeded', completed_at: 4 })
  })

  it('does not settle a Fork intent from a provisional statusline identity', () => {
    seedSession()
    sessions.createSession(command('fork-source-provisional'), {
      id: 'source-provisional', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Source', now: 2
    })
    database.run(
      `INSERT INTO session_fork_intents (
         session_id, source_session_id, source_provider, source_provider_session_id,
         state, created_at, started_at, updated_at
       ) VALUES ('session-1', 'source-provisional', 'claude-code', 'provider-source',
                 'starting', 2, 3, 3)`
    )

    const recorded = sessions.recordResumableProviderIdentity(command('fork-provisional'), {
      id: 'binding-provisional', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-derived', metadata: { lastHookEvent: 'unknown' },
      provisional: true, now: 4
    })

    expect(recorded.result).toMatchObject({
      resumeState: 'unknown',
      metadata: { lastHookEvent: 'unknown', provisional: true }
    })
    expect(recorded.result.validatedAt).toBeUndefined()
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
    expect(database.get(
      'SELECT state, completed_at FROM session_fork_intents WHERE session_id = ?', 'session-1'
    )).toEqual({ state: 'starting', completed_at: null })
  })

  it('refreshes the same hook identity without creating duplicates or losing metadata', () => {
    seedSession()
    sessions.recordResumableProviderIdentity(command('hook-first'), {
      id: 'binding-hook', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-hook-1',
      metadata: { permissionMode: 'bypassPermissions', firstEvent: 'SessionStart' }, now: 3
    })

    const refreshed = sessions.recordResumableProviderIdentity(command('hook-refresh'), {
      id: 'binding-unused', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-hook-1',
      metadata: { cwd: '/tmp/workspace', lastEvent: 'Stop' }, now: 8
    })

    expect(refreshed.result).toMatchObject({
      id: 'binding-hook', validatedAt: 8, metadata: {
        permissionMode: 'bypassPermissions', firstEvent: 'SessionStart',
        cwd: '/tmp/workspace', lastEvent: 'Stop'
      }
    })
    expect(sessions.listProviderBindings('session-1')).toHaveLength(1)
  })

  it('changes permission mode without replacing the resumable conversation identity', () => {
    seedSession()
    sessions.recordResumableProviderIdentity(command('hook-identity'), {
      id: 'binding-hook', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-hook-1',
      metadata: { permissionMode: 'bypassPermissions', cwd: '/tmp/workspace' }, now: 3
    })

    const changed = sessions.updateProviderPermissionMode(command('permission-default'), {
      sessionId: 'session-1', provider: 'claude-code', permissionMode: 'default', now: 8
    })

    expect(changed.result).toMatchObject({
      id: 'binding-hook', providerSessionId: 'provider-hook-1',
      resumeState: 'available', validatedAt: 3, updatedAt: 8,
      metadata: { permissionMode: 'default', cwd: '/tmp/workspace' }
    })
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      id: 'binding-hook', providerSessionId: 'provider-hook-1',
      metadata: { permissionMode: 'default', cwd: '/tmp/workspace' }
    })
    expect(database.get<{ event_type: string }>(
      'SELECT event_type FROM domain_events WHERE seq = ?', changed.firstEventSequence!
    )).toEqual({ event_type: 'provider-binding.permission-mode-updated' })
  })

  it('does not invent a resumable identity while changing permission mode', () => {
    seedSession()

    expect(() => sessions.updateProviderPermissionMode(command('permission-plan'), {
      sessionId: 'session-1', provider: 'claude-code', permissionMode: 'plan', now: 3
    })).toThrow('resumable ProviderBinding does not exist')
    expect(sessions.listProviderBindings('session-1')).toEqual([])
  })

  it('keeps a provider conversation associated with every panel that loads it', () => {
    seedSession()
    sessions.createSession(command('session-2'), {
      id: 'session-2', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Claude 2', now: 2
    })
    sessions.recordResumableProviderIdentity(command('hook-owner'), {
      id: 'binding-hook', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-hook-1', metadata: {}, now: 3
    })

    sessions.recordResumableProviderIdentity(command('hook-second-panel'), {
      id: 'binding-other', sessionId: 'session-2', provider: 'claude-code',
      providerSessionId: 'provider-hook-1', metadata: {}, now: 4
    })
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'provider-hook-1'
    })
    expect(sessions.getResumeBinding('session-2', 'claude-code')).toMatchObject({
      id: 'binding-other', providerSessionId: 'provider-hook-1'
    })
    expect(database.all<{ session_id: string }>(
      'SELECT session_id FROM provider_bindings WHERE provider = ? AND provider_session_id = ? ORDER BY session_id',
      'claude-code', 'provider-hook-1'
    )).toEqual([{ session_id: 'session-1' }, { session_id: 'session-2' }])
  })

  it('isolates provider resume failure to its owning Session', () => {
    seedSession()
    sessions.createSession(command('session-2'), {
      id: 'session-2', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'codex', title: 'Codex', now: 2
    })
    sessions.bindProvider(command('binding-1'), {
      id: 'binding-1', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-1', metadata: {}, now: 3
    })
    sessions.bindProvider(command('binding-2'), {
      id: 'binding-2', sessionId: 'session-2', provider: 'codex',
      providerSessionId: 'provider-2', metadata: {}, now: 3
    })
    sessions.validateProviderBinding(command('validate-1'), 'binding-1', 4)
    sessions.validateProviderBinding(command('validate-2'), 'binding-2', 4)

    sessions.invalidateProviderBinding(command('fail-1'), 'binding-1', 'provider missing', 5)

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
    expect(sessions.getResumeBinding('session-2', 'codex')).toMatchObject({ id: 'binding-2' })
  })

  it('clears a failed resume identity and degrades only that Session to Shell atomically', () => {
    seedSession()
    sessions.bindProvider(command('binding'), {
      id: 'binding-1', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-1', metadata: { permissionMode: 'bypassPermissions' }, now: 3
    })
    sessions.validateProviderBinding(command('validate'), 'binding-1', 4)

    sessions.failResumeToShell(
      command('resume-failed'),
      'session-1',
      'binding-1',
      'provider session not found',
      5
    )

    expect(sessions.getSession('session-1')).toMatchObject({ kind: 'shell', title: 'Shell' })
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
    expect(sessions.listProviderBindings('session-1')).toEqual([
      expect.objectContaining({
        id: 'binding-1', resumeState: 'failed', invalidatedAt: 5,
        metadata: expect.objectContaining({ invalidationReason: 'provider session not found' })
      })
    ])
  })

  it('keeps a named branch title when its provider resume fails', () => {
    seedSession()
    database.run(`UPDATE sessions SET title = '修复登录' WHERE id = 'session-1'`)
    sessions.bindProvider(command('named-binding'), {
      id: 'named-binding-1', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'named-provider-1', metadata: {}, now: 3
    })

    sessions.failResumeToShell(
      command('named-resume-failed'), 'session-1', 'named-binding-1', 'missing', 4
    )

    expect(sessions.getSession('session-1')).toMatchObject({ kind: 'shell', title: '修复登录' })
  })

  it('persists the last confirmed working directory independently per Session', () => {
    seedSession()
    sessions.createSession(command('session-2'), {
      id: 'session-2', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'shell', title: 'Shell 2', now: 2
    })

    sessions.updateCwd(command('cwd-1'), 'session-1', '/tmp/workspace/one', 3)
    sessions.updateCwd(command('cwd-2'), 'session-2', '/tmp/workspace/two', 4)

    expect(sessions.getSession('session-1')).toMatchObject({ cwd: '/tmp/workspace/one' })
    expect(sessions.getSession('session-2')).toMatchObject({ cwd: '/tmp/workspace/two' })
  })

  it('isolates malformed provider metadata instead of blocking the remaining work scene', () => {
    seedSession()
    sessions.bindProvider(command('binding-corrupt'), {
      id: 'binding-corrupt', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-corrupt', metadata: {}, now: 3
    })
    database.run(
      'UPDATE provider_bindings SET metadata_json = ? WHERE id = ?',
      '{broken-json', 'binding-corrupt'
    )

    expect(sessions.getSession('session-1')).toMatchObject({ id: 'session-1' })
    expect(sessions.listProviderBindings('session-1')).toEqual([
      expect.objectContaining({ id: 'binding-corrupt', metadata: {} })
    ])
  })
})

function seedSession(): void {
  sessions.createSession(command('session-1'), {
    id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
    kind: 'claude-code', title: 'Claude', now: 2
  })
}

function command(commandId: string) {
  return { commandId, commandType: 'session', requestHash: `hash-${commandId}` }
}
