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
  hooks = new ProviderHookServer(root, sessions, {
    onNotification: (event) => { notificationEvents.push(event) }
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
    }

    expect((await stat(registration.settingsPath)).mode & 0o777).toBe(0o600)
    expect(Object.keys(settings.hooks).sort()).toEqual([
      'Notification', 'PostToolUse', 'PreToolUse', 'SessionEnd',
      'SessionStart', 'Stop', 'UserPromptSubmit'
    ])
    expect(settings.hooks.PreToolUse?.[0]).toMatchObject({
      matcher: 'Bash|Write|Edit|Read|Glob|Grep',
      hooks: [{ type: 'http', url: expect.stringMatching(/^http:\/\/127\.0\.0\.1:\d+\/hooks\//) }]
    })
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
})

function command(commandId: string) {
  return { commandId, commandType: 'session', requestHash: `hash-${commandId}` }
}

function postHook(url: string, body: unknown): Promise<Response> {
  return fetch(url, {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body)
  })
}
