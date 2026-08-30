import { mkdtemp, readFile, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { SessionRepository } from '../domain/session-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ProviderHookServer } from './provider-hook-server'

let root: string
let database: RuntimeDatabase
let sessions: SessionRepository
let hooks: ProviderHookServer
let notificationEvents: unknown[]
let hudEvents: unknown[]
let identityEvents: unknown[]

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-provider-hooks-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const workspaces = new WorkspaceTaskRepository(database, transactions)
  workspaces.createWorkspace(command('workspace'), {
    id: 'workspace-1', name: 'Workspace', rootDirectory: root, now: 1
  })
  workspaces.createPlainExecutionContext(command('context'), {
    id: 'context-1', workspaceId: 'workspace-1', cwd: root, now: 1
  })
  workspaces.createTask(command('task'), {
    id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
    title: 'Task', status: 'active', sortKey: 'a', now: 1
  })
  sessions = new SessionRepository(database, transactions)
  sessions.createSession(command('session-1'), {
    id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
    kind: 'claude-code', title: 'Claude', now: 2
  })
  notificationEvents = []
  hudEvents = []
  identityEvents = []
  hooks = new ProviderHookServer(root, sessions, {
    onNotification: (event) => { notificationEvents.push(event) },
    onHudPayload: (event) => { hudEvents.push(event) },
    onIdentityRecorded: (event) => { identityEvents.push(event) }
  })
  await hooks.start()
})

afterEach(async () => {
  await hooks.stop()
  database.close()
})

describe('ProviderHookServer', () => {
  it('writes a private additive Claude settings file with Kooky-equivalent hook coverage', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-1', sessionId: 'session-1', permissionMode: 'bypassPermissions'
    })
    const settings = JSON.parse(await readFile(registration.settingsPath, 'utf8')) as {
      hooks: Record<string, Array<{ matcher?: string; hooks: Array<{ type: string; url: string }> }>>
      statusLine: { type: string; command: string; padding: number }
    }

    expect((await stat(registration.settingsPath)).mode & 0o777).toBe(0o600)
    expect(Object.keys(settings.hooks).sort()).toEqual([
      'Notification', 'PermissionRequest', 'PostToolUse', 'PostToolUseFailure', 'PreToolUse', 'SessionEnd',
      'SessionStart', 'Stop', 'UserPromptSubmit'
    ])
    expect(settings.hooks.PreToolUse?.[0]).toMatchObject({
      matcher: expect.stringContaining('TodoWrite'),
      hooks: [{ type: 'http', url: expect.stringMatching(/^http:\/\/127\.0\.0\.1:\d+\/hooks\//) }]
    })
    expect(settings.statusLine).toMatchObject({ type: 'command', padding: 0 })
    expect((await stat(settings.statusLine.command)).mode & 0o111).not.toBe(0)
  })

  it('forwards statusline metrics and tool hooks to the Session HUD authority', async () => {
    const registration = await hooks.registerClaudeSession({ runId: 'run-1', sessionId: 'session-1' })
    await postHook(registration.hookUrl, {
      session_id: 'provider-1', model: { display_name: 'Claude Opus 4.6' },
      context_window: { used_percentage: 72 }
    })
    await postHook(registration.hookUrl, {
      hook_event_name: 'PreToolUse', session_id: 'provider-1',
      tool_name: 'Read', tool_use_id: 'tool-1', tool_input: { file_path: '/tmp/a.ts' }
    })

    expect(hudEvents).toEqual([
      expect.objectContaining({ sessionId: 'session-1', payload: expect.objectContaining({
        context_window: { used_percentage: 72 }
      }) }),
      expect.objectContaining({ sessionId: 'session-1', payload: expect.objectContaining({
        hook_event_name: 'PreToolUse', tool_name: 'Read'
      }) })
    ])
  })

  it('keeps Fork unavailable when statusline reports an identity before the first conversation event', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-1', sessionId: 'session-1', permissionMode: 'bypassPermissions'
    })

    await postHook(registration.hookUrl, {
      session_id: 'provider-not-durable-yet', cwd: root,
      model: { display_name: 'Claude Opus 4.6' }
    })

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()

    await postHook(registration.hookUrl, {
      hook_event_name: 'UserPromptSubmit', session_id: 'provider-not-durable-yet', cwd: root
    })
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'provider-not-durable-yet', resumeState: 'available'
    })
  })

  it('accepts statusline identity as launch confirmation for a resume or Fork', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-resume', sessionId: 'session-1', acceptStatuslineIdentity: true
    })

    await postHook(registration.hookUrl, {
      session_id: 'provider-resumed-or-forked', cwd: root,
      model: { display_name: 'Claude Opus 4.6' }
    })

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'provider-resumed-or-forked', resumeState: 'available',
      metadata: { cwd: root, lastHookEvent: 'unknown' }
    })
    expect(identityEvents).toEqual([{
      runId: 'run-resume', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-resumed-or-forked', eventName: 'unknown'
    }])
  })

  it('keeps a Fork statusline identity provisional until a real conversation event arrives', async () => {
    sessions.createSession(command('fork-source'), {
      id: 'session-source', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Source', now: 2
    })
    database.run(
      `INSERT INTO session_fork_intents (
         session_id, source_session_id, source_provider, source_provider_session_id,
         state, created_at, started_at, updated_at
       ) VALUES ('session-1', 'session-source', 'claude-code', 'provider-source',
                 'starting', 2, 3, 3)`
    )
    const registration = await hooks.registerClaudeSession({
      runId: 'run-fork', sessionId: 'session-1',
      acceptStatuslineIdentity: true, provisionalStatuslineIdentity: true
    })

    await postHook(registration.hookUrl, {
      session_id: 'provider-derived', cwd: root,
      model: { display_name: 'Claude Opus 4.6' }
    })

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
    expect(sessions.listProviderBindings('session-1')).toContainEqual(expect.objectContaining({
      providerSessionId: 'provider-derived', resumeState: 'unknown',
      metadata: expect.objectContaining({ provisional: true })
    }))
    expect(database.get<{ state: string }>(
      'SELECT state FROM session_fork_intents WHERE session_id = ?', 'session-1'
    )).toEqual({ state: 'starting' })

    await postHook(registration.hookUrl, {
      hook_event_name: 'UserPromptSubmit', session_id: 'provider-derived', cwd: root
    })

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'provider-derived', resumeState: 'available'
    })
    expect(sessions.getResumeBinding('session-1', 'claude-code')?.metadata)
      .not.toHaveProperty('provisional')
    expect(database.get<{ state: string }>(
      'SELECT state FROM session_fork_intents WHERE session_id = ?', 'session-1'
    )).toEqual({ state: 'succeeded' })
  })

  it('persists identity from the first supported follow-up hook when HTTP SessionStart does not fire', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-1', sessionId: 'session-1', permissionMode: 'bypassPermissions'
    })

    const response = await postHook(registration.hookUrl, {
      hook_event_name: 'UserPromptSubmit', session_id: 'claude-session-42', cwd: root
    })

    expect(response.status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'claude-session-42', resumeState: 'available',
      metadata: {
        permissionMode: 'bypassPermissions', cwd: root, lastHookEvent: 'UserPromptSubmit'
      }
    })
  })

  it('publishes Stop and Notification as semantic Agent events but ignores SessionEnd', async () => {
    const registration = await hooks.registerClaudeSession({ runId: 'run-1', sessionId: 'session-1' })

    await postHook(registration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'claude-session', cwd: '/tmp/matou',
      last_assistant_message: '任务完成'
    })
    await postHook(registration.hookUrl, {
      hook_event_name: 'SessionEnd', session_id: 'claude-session'
    })

    expect(notificationEvents).toHaveLength(1)
    expect(notificationEvents[0]).toMatchObject({
      runId: 'run-1', sessionId: 'session-1', provider: 'claude-code',
      event: {
        eventType: 'completed', title: 'Claude Code', subtitle: 'Completed in matou', body: '任务完成'
      }
    })
  })

  it('publishes an explicit permission request as a needs-attention provider event', async () => {
    const registration = await hooks.registerClaudeSession({ runId: 'run-permission', sessionId: 'session-1' })
    await postHook(registration.hookUrl, {
      hook_event_name: 'PermissionRequest', session_id: 'claude-session',
      tool_name: 'Write', tool_input: { file_path: '/tmp/matou/README.md' }
    })

    expect(notificationEvents.at(-1)).toMatchObject({
      runId: 'run-permission', sessionId: 'session-1',
      event: { eventType: 'permission', subtitle: 'Permission', body: 'Write: /tmp/matou/README.md' }
    })
  })

  it('does not let a late hook from an old run overwrite a newer persisted permission mode', async () => {
    sessions.recordResumableProviderIdentity(command('existing-binding'), {
      id: 'binding-existing', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'claude-existing', metadata: { permissionMode: 'default' }, now: 3
    })
    const oldRun = await hooks.registerClaudeSession({
      runId: 'run-old', sessionId: 'session-1', permissionMode: 'default'
    })
    sessions.updateProviderPermissionMode(command('switch-mode'), {
      sessionId: 'session-1', provider: 'claude-code', permissionMode: 'plan', now: 4
    })

    expect((await postHook(oldRun.hookUrl, {
      hook_event_name: 'Stop', session_id: 'claude-existing'
    })).status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'claude-existing',
      metadata: { permissionMode: 'plan', lastHookEvent: 'Stop' }
    })
  })

  it('uses a transcript UUID when a hook version omits the direct session field', async () => {
    const registration = await hooks.registerClaudeSession({ runId: 'run-1', sessionId: 'session-1' })

    await postHook(registration.hookUrl, {
      hook_event_name: 'Stop',
      transcript_path: `/tmp/project/123e4567-e89b-12d3-a456-426614174000.jsonl`
    })

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: '123e4567-e89b-12d3-a456-426614174000'
    })
  })

  it('ignores empty hook identities instead of erasing the last confirmed conversation', async () => {
    sessions.recordResumableProviderIdentity(command('existing-binding'), {
      id: 'binding-existing', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'claude-existing', metadata: { permissionMode: 'default' }, now: 3
    })
    const registration = await hooks.registerClaudeSession({ runId: 'run-1', sessionId: 'session-1' })

    expect((await postHook(registration.hookUrl, {
      hook_event_name: 'Notification', session_id: '   ', message: 'Waiting'
    })).status).toBe(200)
    expect(sessions.listProviderBindings('session-1')).toHaveLength(1)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'claude-existing'
    })
  })

  it('rejects an unknown or retired run token without changing persisted identity', async () => {
    const registration = await hooks.registerClaudeSession({ runId: 'run-1', sessionId: 'session-1' })
    await registration.dispose()

    expect((await postHook(registration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'claude-should-not-bind'
    })).status).toBe(404)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
  })

  it('keeps the hook token alive briefly so Claude can deliver SessionEnd after its PTY exits', async () => {
    const registration = await hooks.registerClaudeSession({ runId: 'run-1', sessionId: 'session-1' })

    registration.retire(50)
    expect((await postHook(registration.hookUrl, {
      hook_event_name: 'SessionEnd', session_id: 'provider-ending'
    })).status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()

    await new Promise((resolve) => setTimeout(resolve, 75))
    expect((await postHook(registration.hookUrl, {
      hook_event_name: 'SessionEnd', session_id: 'provider-too-late'
    })).status).toBe(404)
  })

  it('keeps final notifications but blocks late identity changes after a provider run retires', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-retiring', sessionId: 'session-1', acceptStatuslineIdentity: true
    })

    registration.retire(100)
    expect((await postHook(registration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'provider-too-late',
      last_assistant_message: '已结束'
    })).status).toBe(200)

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
    expect(identityEvents).toEqual([])
    expect(notificationEvents).toHaveLength(1)
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'session', requestHash: `hash-${commandId}` }
}

function postHook(url: string, body: unknown): Promise<Response> {
  return fetch(url, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body)
  })
}
