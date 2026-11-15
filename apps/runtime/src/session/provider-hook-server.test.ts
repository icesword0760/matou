import { appendFile, mkdir, mkdtemp, readFile, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspaceTaskRepository } from '../domain/workspace-task-repository'
import { SessionRepository } from '../domain/session-repository'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { evaluateAntigravityPreToolUse, ProviderHookServer } from './provider-hook-server'
import { SessionForkIntentRepository } from './session-fork-intent-repository'
import { SessionHudRegistry } from './session-hud-registry'
import { createProviderHudPayloadHandler } from './provider-hud-payload-handler'

let root: string
let database: RuntimeDatabase
let sessions: SessionRepository
let hooks: ProviderHookServer
let notificationEvents: unknown[]
let hudEvents: unknown[]
let identityEvents: unknown[]
let titleEvents: unknown[]
let teamObservations: unknown[]
let workStatusEvents: unknown[]

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
  titleEvents = []
  teamObservations = []
  workStatusEvents = []
  hooks = new ProviderHookServer(root, sessions, {
    onNotification: (event) => { notificationEvents.push(event) },
    onHudPayload: (event) => { hudEvents.push(event) },
    onIdentityRecorded: (event) => { identityEvents.push(event) },
    onTitleObserved: (event) => { titleEvents.push(event) },
    onTeamObservations: (events) => { teamObservations.push(...events) },
    onWorkStatus: (event) => { workStatusEvents.push(event) }
  })
  await hooks.start()
})

afterEach(async () => {
  await hooks.stop()
  database.close()
})

describe('ProviderHookServer', () => {
  it('records a fenced Codex Fork from its exact SessionStart identity', async () => {
    sessions.createSession(command('codex-source'), {
      id: 'codex-source', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'codex', title: 'Codex source', now: 2
    })
    sessions.createSession(command('codex-child'), {
      id: 'codex-child', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'codex', title: 'Codex child', now: 2
    })
    const intents = new SessionForkIntentRepository(database)
    const now = Date.now()
    intents.accept({
      operationId: 'codex-operation', submissionKey: 'codex-submission',
      sessionId: 'codex-child', sourceSessionId: 'codex-source',
      sourceProviderSessionId: 'codex-source-context', permissionMode: 'default',
      displayName: 'Codex child',
      worktreeMode: 'current', totalSteps: 2, now
    })
    const acquired = intents.acquireLease({
      operationId: 'codex-operation', owner: 'runtime', now, ttlMs: 60_000
    })
    if (acquired.kind !== 'acquired') throw new Error('Codex Fork lease missing')
    intents.advanceStage({
      operationId: 'codex-operation', lease: acquired.lease,
      stage: 'restoring-provider', now
    })
    const registration = await hooks.registerCodexSession({
      runId: 'codex-run', sessionId: 'codex-child',
      providerConfigId: 'codex-config', model: 'gpt-session',
      permissionMode: 'bypassPermissions', inheritedConversation: true,
      forkAuthority: {
        operationId: 'codex-operation', runId: 'codex-run', lease: acquired.lease
      }
    })
    const config = JSON.parse(await readFile(registration.settingsPath, 'utf8')) as {
      hooks: Record<string, Array<{
        hooks: Array<{ type: string; command: string; timeout: number; statusMessage: string }>
      }>>
    }
    const startHook = config.hooks.SessionStart?.[0]?.hooks[0]
    const endHook = config.hooks.SessionEnd?.[0]?.hooks[0]
    expect(startHook).toMatchObject({
      type: 'command', timeout: 5, statusMessage: '正在确认会话'
    })
    expect(endHook).toMatchObject({
      type: 'command', timeout: 3, statusMessage: '正在确认会话'
    })
    expect(registration.codexHooksConfig).toBe(
      `{ SessionStart = [{ hooks = [{ type = "command", command = ${JSON.stringify(startHook?.command)}, timeout = 5, statusMessage = "正在确认会话" }] }], ` +
      `UserPromptSubmit = [{ hooks = [{ type = "command", command = ${JSON.stringify(startHook?.command)}, timeout = 5, statusMessage = "正在确认会话" }] }], ` +
      `PreToolUse = [{ hooks = [{ type = "command", command = ${JSON.stringify(startHook?.command)}, timeout = 10, statusMessage = "正在确认会话" }] }], ` +
      `PermissionRequest = [{ hooks = [{ type = "command", command = ${JSON.stringify(startHook?.command)}, timeout = 10, statusMessage = "正在确认会话" }] }], ` +
      `PostToolUse = [{ hooks = [{ type = "command", command = ${JSON.stringify(startHook?.command)}, timeout = 10, statusMessage = "正在确认会话" }] }], ` +
      `Stop = [{ hooks = [{ type = "command", command = ${JSON.stringify(startHook?.command)}, timeout = 5, statusMessage = "正在确认会话" }] }], ` +
      `SessionEnd = [{ hooks = [{ type = "command", command = ${JSON.stringify(endHook?.command)}, timeout = 3, statusMessage = "正在确认会话" }] }] }`
    )
    expect(registration.codexHooksConfig).not.toContain(registration.hookUrl)

    await postHook(registration.hookUrl, {
      hook_event_name: 'SessionStart', session_id: 'codex-derived-context', cwd: root
    })

    expect(sessions.getResumeBinding('codex-child', 'codex')).toMatchObject({
      providerSessionId: 'codex-derived-context', resumeState: 'available',
      metadata: expect.objectContaining({
        providerConfigId: 'codex-config', model: 'gpt-session',
        permissionMode: 'bypassPermissions', inheritedConversation: true
      })
    })
    expect(intents.state('codex-child')).toBe('succeeded')
    expect(identityEvents).toContainEqual(expect.objectContaining({
      runId: 'codex-run', sessionId: 'codex-child', provider: 'codex',
      providerSessionId: 'codex-derived-context', eventName: 'SessionStart'
    }))
  })

  it('registers only the seven observed Codex events in both launch representations', async () => {
    const registration = await hooks.registerCodexSession({ runId: 'codex-run', sessionId: 'session-1' })
    const settings = JSON.parse(await readFile(registration.settingsPath, 'utf8')) as {
      hooks: Record<string, Array<{ hooks: Array<{ type: string; timeout: number }> }>>
    }
    const events = [
      'SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PermissionRequest', 'PostToolUse', 'Stop', 'SessionEnd'
    ]
    expect(Object.keys(settings.hooks)).toEqual(events)
    expect([...registration.codexHooksConfig!.matchAll(/\b([A-Z]\w*) = \[/g)].map((match) => match[1])).toEqual(events)
    expect(settings.hooks.PermissionRequest?.[0]?.hooks[0]).toMatchObject({ type: 'command', timeout: 10 })
    expect(settings.hooks.SessionEnd?.[0]?.hooks[0]?.timeout).toBe(3)
    expect(settings).not.toHaveProperty('statusLine')
  })

  it('routes Codex identity, lifecycle HUD payloads, statuses and notifications through the shared callbacks', async () => {
    const registration = await hooks.registerCodexSession({ runId: 'codex-run', sessionId: 'session-1' })
    const common = {
      session_id: '01a1ddc1-88fd-7fc1-8c66-ab8d38719da8',
      cwd: root, model: 'gpt-6-astra', permission_mode: 'default'
    }
    const events = [
      { hook_event_name: 'SessionStart', source: 'startup' },
      { hook_event_name: 'UserPromptSubmit', prompt: "Run python3 -c 'print(2)'" },
      { hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_use_id: 'exec-1', tool_input: { command: "python3 -c 'print(2)'" } },
      // The real PermissionRequest payload has no tool_use_id.
      { hook_event_name: 'PermissionRequest', tool_name: 'Bash', tool_input: { command: "python3 -c 'print(2)'" } },
      { hook_event_name: 'PostToolUse', tool_name: 'Bash', tool_use_id: 'exec-1', tool_response: '2\n' },
      { hook_event_name: 'Stop', stop_hook_active: false, last_assistant_message: 'Output: 2' },
      { hook_event_name: 'SessionEnd', reason: 'other' }
    ]
    for (const event of events) {
      expect((await postHook(registration.hookUrl, { ...common, ...event })).status).toBe(200)
    }
    expect(sessions.getResumeBinding('session-1', 'codex')).toMatchObject({
      providerSessionId: common.session_id,
      metadata: expect.objectContaining({ model: 'gpt-6-astra', permissionMode: 'default' })
    })
    expect(identityEvents).toHaveLength(6)
    expect(hudEvents).toEqual(events.map((event) => ({
      runId: 'codex-run', sessionId: 'session-1', provider: 'codex',
      payload: { ...common, ...event }, isStatusline: false
    })))
    expect(workStatusEvents).toEqual(['idle', 'running', 'running', 'needs-input', 'running', 'idle', 'exited'].map((workStatus) => ({
      runId: 'codex-run', sessionId: 'session-1', provider: 'codex', workStatus
    })))
    expect(notificationEvents).toEqual([
      expect.objectContaining({ provider: 'codex', event: expect.objectContaining({
        eventType: 'permission', title: 'Codex', body: "Bash: python3 -c 'print(2)'", sound: true, cooldownKey: 'Notification'
      }) }),
      expect.objectContaining({ provider: 'codex', event: expect.objectContaining({
        eventType: 'completed', title: 'Codex', body: 'Output: 2', sound: true, cooldownKey: 'Stop'
      }) })
    ])
    expect(titleEvents).toEqual([])
    expect(teamObservations).toEqual([])
  })

  it('updates the existing Codex HUD state immediately and clears approval waiting on tool completion', async () => {
    const hud = new SessionHudRegistry()
    hud.spawn({ sessionId: 'session-1', profile: 'codex', cwd: root })
    const pipeline = createProviderHudPayloadHandler({
      hud, currentRunId: () => 'codex-hud-run', publish: () => {},
      reportError: (error) => { throw error }
    })
    const server = new ProviderHookServer(root, sessions, { onHudPayload: pipeline })
    await server.start()
    try {
      const registration = await server.registerCodexSession({ runId: 'codex-hud-run', sessionId: 'session-1' })
      for (const [event, expected] of [
        ['UserPromptSubmit', 'running'], ['SessionStart', 'idle'], ['UserPromptSubmit', 'running'],
        ['PreToolUse', 'running'], ['PermissionRequest', 'needs-input'], ['PostToolUse', 'running'], ['Stop', 'idle'], ['SessionEnd', 'idle']
      ]) {
        await postHook(registration.hookUrl, {
          hook_event_name: event, session_id: 'codex-hud-id',
          tool_name: 'Bash', tool_use_id: 'exec-1'
        })
        expect(hud.snapshot('session-1')?.taskStatus).toBe(expected)
      }
      expect(hud.snapshot('session-1')?.runningTools).toEqual([])
    } finally {
      await server.stop()
    }
  })

  it('ignores all late Codex effects after a newer run owns the card', async () => {
    let currentRun = 'codex-old-run'
    const server = new ProviderHookServer(root, sessions, {
      currentRunId: () => currentRun,
      onNotification: (event) => { notificationEvents.push(event) },
      onHudPayload: (event) => { hudEvents.push(event) },
      onWorkStatus: (event) => { workStatusEvents.push(event) },
      onIdentityRecorded: (event) => { identityEvents.push(event) },
      onTitleObserved: (event) => { titleEvents.push(event) }
    })
    await server.start()
    try {
      const oldRun = await server.registerCodexSession({ runId: currentRun, sessionId: 'session-1' })
      currentRun = 'codex-new-run'
      const newRun = await server.registerCodexSession({ runId: currentRun, sessionId: 'session-1' })
      await postHook(newRun.hookUrl, { hook_event_name: 'SessionStart', session_id: 'codex-current-id' })
      const currentBinding = sessions.getResumeBinding('session-1', 'codex')
      const counts = [identityEvents.length, hudEvents.length, workStatusEvents.length]
      oldRun.retire(1_000)
      for (const hook_event_name of ['SessionStart', 'PermissionRequest', 'Stop', 'SessionEnd']) {
        expect((await postHook(oldRun.hookUrl, {
          hook_event_name, session_id: 'codex-old-id', last_assistant_message: 'stale answer'
        })).status).toBe(200)
      }
      expect(sessions.getResumeBinding('session-1', 'codex')).toEqual(currentBinding)
      expect([identityEvents.length, hudEvents.length, workStatusEvents.length]).toEqual(counts)
      expect(notificationEvents).toEqual([])
      expect(titleEvents).toEqual([])
    } finally {
      await server.stop()
    }
  })

  it('rejects a mismatched Codex restore identity before any visible effect', async () => {
    const registration = await hooks.registerCodexSession({
      runId: 'codex-restore-run', sessionId: 'session-1', expectedProviderSessionId: 'expected-codex-id'
    })
    for (const hook_event_name of ['SessionStart', 'PermissionRequest', 'Stop', 'SessionEnd']) {
      expect((await postHook(registration.hookUrl, { hook_event_name, session_id: 'wrong-codex-id' })).status).toBe(200)
    }
    expect(sessions.getResumeBinding('session-1', 'codex')).toBeUndefined()
    expect([identityEvents, hudEvents, notificationEvents, workStatusEvents, titleEvents]).toEqual([[], [], [], [], []])
  })

  it('preserves Codex retirement grace without reactivating identity, and suppresses effects when requested', async () => {
    const registration = await hooks.registerCodexSession({ runId: 'codex-ending', sessionId: 'session-1' })
    registration.retire(1_000)
    await postHook(registration.hookUrl, { hook_event_name: 'Stop', session_id: 'codex-ending-id', last_assistant_message: 'Done' })
    expect(notificationEvents).toHaveLength(1)
    expect(sessions.getResumeBinding('session-1', 'codex')).toBeUndefined()
    const muted = await hooks.registerCodexSession({ runId: 'codex-muted', sessionId: 'session-1' })
    muted.retire(1_000, { suppressSideEffects: true })
    await postHook(muted.hookUrl, { hook_event_name: 'Stop', session_id: 'codex-muted-id' })
    expect(notificationEvents).toHaveLength(1)
    expect(hudEvents).toHaveLength(1)
    expect(workStatusEvents).toHaveLength(1)
    expect(identityEvents).toEqual([])
    await registration.dispose()
    expect((await postHook(registration.hookUrl, { hook_event_name: 'Stop' })).status).toBe(404)
  })

  it('ignores the observed transcript-less Codex memories run before and after the main identity', async () => {
    const registration = await hooks.registerCodexSession({ runId: 'codex-main', sessionId: 'session-1' })
    const noise = {
      session_id: '01a1ddc1-e832-7cf2-83b7-5adb2f12224b',
      hook_event_name: 'SessionStart', transcript_path: null,
      cwd: '/private/tmp/matou-stage2-probe/codex-home/memories',
      source: 'startup', model: 'gpt-5.6-terra', permission_mode: 'bypassPermissions'
    }
    await postHook(registration.hookUrl, noise)
    expect(sessions.getResumeBinding('session-1', 'codex')).toBeUndefined()
    expect([identityEvents, hudEvents, workStatusEvents, notificationEvents]).toEqual([[], [], [], []])

    await postHook(registration.hookUrl, {
      hook_event_name: 'SessionStart', session_id: 'codex-main-id', transcript_path: '/tmp/main-rollout.jsonl'
    })
    const binding = sessions.getResumeBinding('session-1', 'codex')
    for (const hook_event_name of ['SessionStart', 'PreToolUse', 'PermissionRequest', 'Stop', 'SessionEnd']) {
      await postHook(registration.hookUrl, { ...noise, hook_event_name })
    }
    expect(sessions.getResumeBinding('session-1', 'codex')).toEqual(binding)
    expect([identityEvents.length, hudEvents.length, workStatusEvents.length]).toEqual([1, 1, 1])
    expect(notificationEvents).toEqual([])
  })

  it('pins the confirmed Codex identity even if another conversation has a transcript', async () => {
    const registration = await hooks.registerCodexSession({ runId: 'codex-main', sessionId: 'session-1' })
    await postHook(registration.hookUrl, { hook_event_name: 'SessionStart', session_id: 'codex-main-id' })
    const binding = sessions.getResumeBinding('session-1', 'codex')
    for (const hook_event_name of ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PermissionRequest', 'Stop', 'SessionEnd']) {
      await postHook(registration.hookUrl, {
        hook_event_name, session_id: 'codex-other-id', transcript_path: '/tmp/other-rollout.jsonl'
      })
    }
    expect(sessions.getResumeBinding('session-1', 'codex')).toEqual(binding)
    expect([identityEvents.length, hudEvents.length, workStatusEvents.length]).toEqual([1, 1, 1])
    expect(notificationEvents).toEqual([])
    await postHook(registration.hookUrl, { hook_event_name: 'UserPromptSubmit', session_id: 'codex-main-id' })
    expect(workStatusEvents.at(-1)).toMatchObject({ provider: 'codex', workStatus: 'running' })
  })

  it('does not discard Codex events with a valid alternate transcript path', async () => {
    const registration = await hooks.registerCodexSession({ runId: 'codex-main', sessionId: 'session-1' })
    await postHook(registration.hookUrl, {
      hook_event_name: 'SessionStart', session_id: 'codex-main-id',
      transcript_path: null, transcriptPath: '/tmp/main-rollout.jsonl'
    })
    expect(sessions.getResumeBinding('session-1', 'codex')?.providerSessionId).toBe('codex-main-id')
    expect(workStatusEvents).toHaveLength(1)
  })

  it('retains the Codex lease fence before allowing HUD, status or notification effects', async () => {
    sessions.createSession(command('codex-parent'), {
      id: 'codex-parent', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'codex', title: 'Codex', now: 2
    })
    const intents = new SessionForkIntentRepository(database)
    const now = Date.now() - 5_000
    intents.accept({
      operationId: 'codex-stale-op', submissionKey: 'codex-stale-key',
      sessionId: 'session-1', sourceSessionId: 'codex-parent',
      sourceProviderSessionId: 'codex-parent', permissionMode: 'default',
      displayName: 'Codex', worktreeMode: 'current', totalSteps: 2, now
    })
    const lease = intents.acquireLease({ operationId: 'codex-stale-op', owner: 'old-run', ttlMs: 1, now })
    if (lease.kind !== 'acquired') throw new Error('Expected test lease')
    intents.advanceStage({ operationId: 'codex-stale-op', lease: lease.lease, stage: 'restoring-provider', now })
    const registration = await hooks.registerCodexSession({
      runId: 'codex-stale-run', sessionId: 'session-1',
      forkAuthority: { operationId: 'codex-stale-op', runId: 'codex-stale-run', lease: lease.lease }
    })
    for (const hook_event_name of ['SessionStart', 'PermissionRequest', 'Stop']) {
      expect((await postHook(registration.hookUrl, { hook_event_name, session_id: 'codex-stale-child' })).status).toBe(200)
    }
    expect(sessions.getResumeBinding('session-1', 'codex')).toBeUndefined()
    expect([identityEvents, hudEvents, workStatusEvents, notificationEvents]).toEqual([[], [], [], []])
  })

  it('writes a private additive Claude settings file with reference product-equivalent hook coverage', async () => {
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
      hooks: [{ type: 'http', url: expect.stringMatching(/^http:\/\/127\.0\.0\.1:\d+\/hooks\//) }]
    })
    expect(settings.hooks.PreToolUse?.[0]).not.toHaveProperty('matcher')
    expect(settings.hooks.PostToolUse?.[0]).not.toHaveProperty('matcher')
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

  it('observes the latest Claude ai-title from the transcript without turning it into a hook failure', async () => {
    const registration = await hooks.registerClaudeSession({ runId: 'run-title', sessionId: 'session-1' })
    const transcriptPath = join(root, 'provider-title.jsonl')
    await writeFile(transcriptPath, [
      JSON.stringify({
        type: 'user', sessionId: 'provider-title', cwd: root,
        message: { role: 'user', content: '处理登录失败' }
      }),
      JSON.stringify({ type: 'ai-title', sessionId: 'provider-title', aiTitle: '修复登录恢复流程' })
    ].join('\n'))

    await postHook(registration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'provider-title', transcript_path: transcriptPath
    })

    expect(titleEvents).toEqual([{
      runId: 'run-title', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-title', title: '修复登录恢复流程'
    }])
  })

  it('watches a stopped active transcript until Claude writes its first ai-title', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-delayed-title', sessionId: 'session-1'
    })
    const transcriptPath = join(root, 'provider-delayed-title.jsonl')
    await writeFile(transcriptPath, JSON.stringify({
      type: 'user', sessionId: 'provider-delayed-title', cwd: root,
      message: { role: 'user', content: '排查会话标题生成' }
    }))

    await postHook(registration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'provider-delayed-title', transcript_path: transcriptPath
    })
    await appendFile(transcriptPath, `\n${JSON.stringify({
      type: 'ai-title', sessionId: 'provider-delayed-title', aiTitle: '排查会话标题生成'
    })}`)

    await waitFor(() => titleEvents.length === 1, 250)
    expect(titleEvents).toEqual([{
      runId: 'run-delayed-title', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'provider-delayed-title', title: '排查会话标题生成'
    }])

    await appendFile(transcriptPath, `\n${JSON.stringify({
      type: 'ai-title', sessionId: 'provider-delayed-title', aiTitle: '后续标题不再触发监听'
    })}`)
    await new Promise((resolve) => setTimeout(resolve, 80))
    expect(titleEvents).toHaveLength(1)
  })

  it('projects real Agent Teams transcript activity into teammate observations', async () => {
    await hooks.stop()
    const observations: unknown[] = []
    hooks = new ProviderHookServer(root, sessions, {
      onTeamObservations: (events: unknown[]) => observations.push(...events)
    } as unknown as ConstructorParameters<typeof ProviderHookServer>[2])
    await hooks.start()
    const registration = await hooks.registerClaudeSession({
      runId: 'run-team', sessionId: 'session-1'
    })
    const transcriptPath = join(root, 'provider-team-real.jsonl')
    await writeFile(transcriptPath, [
      JSON.stringify({
        type: 'user',
        toolUseResult: {
          status: 'teammate_spawned',
          teammate_id: 'MATOU_QA_TEAMMATE@session-real',
          agent_id: 'MATOU_QA_TEAMMATE@session-real',
          name: 'MATOU_QA_TEAMMATE',
          team_name: 'session-real',
          prompt: 'Reply exactly TEAMMATE_REAL_READY'
        }
      }),
      JSON.stringify({
        type: 'queue-operation', operation: 'enqueue',
        content: '<agent-message from="MATOU_QA_TEAMMATE">\nTEAMMATE_REAL_READY\n</agent-message>'
      }),
      JSON.stringify({
        type: 'user',
        toolUseResult: {
          listing: 'Teammates (1):\n  MATOU_QA_TEAMMATE [96bcfc]  ·  idle  ·  started 8s ago'
        }
      })
    ].join('\n'))

    await postHook(registration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'provider-team', transcript_path: transcriptPath
    })

    expect(observations).toContainEqual({
      runId: 'run-team', leadSessionId: 'session-1',
      teammateId: 'MATOU_QA_TEAMMATE@session-real', teamId: 'session-real',
      name: 'MATOU_QA_TEAMMATE', workStatus: 'idle',
      latestLines: ['Reply exactly TEAMMATE_REAL_READY', 'TEAMMATE_REAL_READY']
    })
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

  it('keeps refreshing HUD metrics without appending identical statusline identities', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-statusline', sessionId: 'session-1', acceptStatuslineIdentity: true
    })
    const payload = { session_id: 'provider-statusline', cwd: root, model: { id: 'claude-opus-4-6' } }
    await postHook(registration.hookUrl, payload)
    const eventCount = () => database.get<{ count: number }>('SELECT COUNT(*) AS count FROM domain_events')!.count
    const commandCount = () => database.get<{ count: number }>('SELECT COUNT(*) AS count FROM command_deduplication')!.count
    const before = [eventCount(), commandCount()]
    for (let used = 1; used <= 20; used++) {
      expect((await postHook(registration.hookUrl, {
        ...payload, context_window: { used_percentage: used }
      })).status).toBe(200)
    }
    expect([eventCount(), commandCount()]).toEqual(before)
    expect(identityEvents).toHaveLength(1)
    expect(hudEvents).toHaveLength(21)

    // Real lifecycle events and provider setting/identity changes remain durable.
    await postHook(registration.hookUrl, { ...payload, hook_event_name: 'UserPromptSubmit' })
    await postHook(registration.hookUrl, payload)
    expect(identityEvents).toHaveLength(2)
    await postHook(registration.hookUrl, { ...payload, permission_mode: 'plan' })
    expect(identityEvents).toHaveLength(3)
    await postHook(registration.hookUrl, { ...payload, model: { id: 'claude-sonnet-4-6' } })
    expect(identityEvents).toHaveLength(4)
    await postHook(registration.hookUrl, { ...payload, session_id: 'provider-replacement' })
    expect(identityEvents).toHaveLength(5)
    await registration.dispose()

    const resumed = await hooks.registerClaudeSession({
      runId: 'run-statusline-restarted', sessionId: 'session-1', acceptStatuslineIdentity: true
    })
    await postHook(resumed.hookUrl, { ...payload, session_id: 'provider-replacement' })
    expect(identityEvents).toHaveLength(6)
  })

  it('rejects a different statusline identity while restoring one specific conversation', async () => {
    const mismatches: unknown[] = []
    const rejectedHudEvents: unknown[] = []
    const rejectedNotificationEvents: unknown[] = []
    const rejectedTeamObservations: unknown[] = []
    const rejectedTitleEvents: unknown[] = []
    const strictHooks = new ProviderHookServer(root, sessions, {
      onIdentityMismatch: (event) => { mismatches.push(event) },
      onHudPayload: (event) => { rejectedHudEvents.push(event) },
      onNotification: (event) => { rejectedNotificationEvents.push(event) },
      onTitleObserved: (event) => { rejectedTitleEvents.push(event) },
      onTeamObservations: (events) => { rejectedTeamObservations.push(...events) }
    })
    await strictHooks.start()
    try {
      const registration = await strictHooks.registerClaudeSession({
        runId: 'run-strict-resume', sessionId: 'session-1',
        acceptStatuslineIdentity: true,
        expectedProviderSessionId: 'provider-old'
      })
      const transcriptPath = join(root, 'provider-wrong-team.jsonl')
      await writeFile(transcriptPath, [
        JSON.stringify({
          type: 'user',
          toolUseResult: {
            status: 'teammate_spawned',
            teammate_id: 'WRONG_TEAMMATE@wrong-team',
            name: 'WRONG_TEAMMATE',
            team_name: 'wrong-team',
            prompt: 'This observation belongs to another conversation'
          }
        }),
        JSON.stringify({
          type: 'ai-title', sessionId: 'provider-new', aiTitle: 'Wrong conversation title'
        })
      ].join('\n'))

      expect((await postHook(registration.hookUrl, {
        hook_event_name: 'Stop', session_id: 'provider-new', cwd: root,
        model: { display_name: 'Claude Opus 4.6' },
        transcript_path: transcriptPath,
        last_assistant_message: 'Wrong conversation completed'
      })).status).toBe(200)
      expect((await postHook(registration.hookUrl, {
        hook_event_name: 'PermissionRequest', session_id: 'provider-new', cwd: root,
        tool_name: 'Write', tool_input: { file_path: '/tmp/wrong-conversation' }
      })).status).toBe(200)

      expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
      expect(rejectedHudEvents).toEqual([])
      expect(rejectedNotificationEvents).toEqual([])
      expect(rejectedTeamObservations).toEqual([])
      expect(rejectedTitleEvents).toEqual([])
      expect(mismatches).toEqual([{
        runId: 'run-strict-resume', sessionId: 'session-1', provider: 'claude-code',
        expectedProviderSessionId: 'provider-old', actualProviderSessionId: 'provider-new',
        eventName: 'Stop'
      }])
    } finally {
      await strictHooks.stop()
    }
  })

  it('limits the expected identity gate to resume confirmation so later conversation changes remain valid', async () => {
    const mismatches: unknown[] = []
    const strictHooks = new ProviderHookServer(root, sessions, {
      onIdentityMismatch: (event) => { mismatches.push(event) }
    })
    await strictHooks.start()
    try {
      const registration = await strictHooks.registerClaudeSession({
        runId: 'run-confirmed-resume', sessionId: 'session-1',
        acceptStatuslineIdentity: true,
        expectedProviderSessionId: 'provider-old'
      })

      await postHook(registration.hookUrl, {
        session_id: 'provider-old', cwd: root,
        model: { display_name: 'Claude Opus 4.6' }
      })
      await postHook(registration.hookUrl, {
        hook_event_name: 'UserPromptSubmit', session_id: 'provider-next', cwd: root
      })

      expect(mismatches).toEqual([])
      expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
        providerSessionId: 'provider-next', resumeState: 'available'
      })
    } finally {
      await strictHooks.stop()
    }
  })

  it('accepts a Fork statusline identity as an immediately forkable inherited conversation', async () => {
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
      acceptStatuslineIdentity: true, inheritedConversation: true
    })

    await postHook(registration.hookUrl, {
      session_id: 'provider-derived', cwd: root,
      model: { display_name: 'Claude Opus 4.6' }
    })

    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'provider-derived', resumeState: 'available',
      metadata: expect.objectContaining({ inheritedConversation: true, canFork: true })
    })
    expect(sessions.getResumeBinding('session-1', 'claude-code')?.metadata)
      .not.toHaveProperty('provisional')
    expect(database.get<{ state: string }>(
      'SELECT state FROM session_fork_intents WHERE session_id = ?', 'session-1'
    )).toEqual({ state: 'succeeded' })
  })

  it('ignores a provider identity from a durable Fork run whose lease fence was replaced', async () => {
    sessions.createSession(command('fork-source-fenced'), {
      id: 'session-source-fenced', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Source', now: 2
    })
    const intents = new SessionForkIntentRepository(database)
    const now = Date.now()
    intents.accept({
      operationId: 'operation-fenced', submissionKey: 'submission-fenced',
      sessionId: 'session-1', sourceSessionId: 'session-source-fenced',
      sourceProviderSessionId: 'provider-source', permissionMode: 'default', displayName: 'Derived',
      worktreeMode: 'current', totalSteps: 2, now
    })
    const first = intents.acquireLease({
      operationId: 'operation-fenced', owner: 'runtime-old', now, ttlMs: 1
    })
    if (first.kind !== 'acquired') throw new Error('first lease missing')
    intents.advanceStage({
      operationId: 'operation-fenced', lease: first.lease,
      stage: 'restoring-provider', now
    })
    const oldRegistration = await hooks.registerClaudeSession({
      runId: 'run-old-fork', sessionId: 'session-1',
      acceptStatuslineIdentity: true, inheritedConversation: true,
      forkAuthority: {
        operationId: 'operation-fenced', runId: 'run-old-fork', lease: first.lease
      }
    })
    const second = intents.acquireLease({
      operationId: 'operation-fenced', owner: 'runtime-new', now: now + 2, ttlMs: 60_000
    })
    if (second.kind !== 'acquired') throw new Error('takeover lease missing')

    const staleTranscript = join(root, 'provider-stale-team.jsonl')
    await writeFile(staleTranscript, JSON.stringify({
      type: 'user',
      toolUseResult: {
        status: 'teammate_spawned', teammate_id: 'STALE@stale-team',
        name: 'STALE', team_name: 'stale-team', prompt: 'stale fork observation'
      }
    }))
    expect((await postHook(oldRegistration.hookUrl, {
      hook_event_name: 'SessionEnd', session_id: 'provider-stale', cwd: root,
      model: { display_name: 'Claude Opus 4.6' },
      transcript_path: staleTranscript,
      last_assistant_message: 'Stale Fork completed'
    })).status).toBe(200)
    expect(hudEvents).toEqual([])
    expect(notificationEvents).toEqual([])
    expect(teamObservations).toEqual([])
    expect((await postHook(oldRegistration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'provider-stale', cwd: root,
      model: { display_name: 'Claude Opus 4.6' },
      transcript_path: staleTranscript,
      last_assistant_message: 'Stale Fork completed'
    })).status).toBe(200)
    expect((await postHook(oldRegistration.hookUrl, {
      hook_event_name: 'PermissionRequest', session_id: 'provider-stale', cwd: root,
      tool_name: 'Write', tool_input: { file_path: '/tmp/stale-fork' }
    })).status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toBeUndefined()
    expect(identityEvents).toEqual([])
    expect(hudEvents).toEqual([])
    expect(notificationEvents).toEqual([])
    expect(teamObservations).toEqual([])
    expect(database.get(
      'SELECT state, stage FROM session_fork_intents WHERE operation_id = ?', 'operation-fenced'
    )).toEqual({ state: 'starting', stage: 'restoring-provider' })

    const currentRegistration = await hooks.registerClaudeSession({
      runId: 'run-current-fork', sessionId: 'session-1',
      acceptStatuslineIdentity: true, inheritedConversation: true,
      forkAuthority: {
        operationId: 'operation-fenced', runId: 'run-current-fork', lease: second.lease
      }
    })
    expect((await postHook(currentRegistration.hookUrl, {
      session_id: 'provider-current', cwd: root,
      model: { display_name: 'Claude Opus 4.6' }
    })).status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'provider-current'
    })
    expect(database.get(
      'SELECT state, stage FROM session_fork_intents WHERE operation_id = ?', 'operation-fenced'
    )).toEqual({ state: 'succeeded', stage: 'succeeded' })
    expect(identityEvents).toContainEqual(expect.objectContaining({
      runId: 'run-current-fork',
      forkAuthority: expect.objectContaining({
        operationId: 'operation-fenced', runId: 'run-current-fork',
        lease: expect.objectContaining({ fence: second.lease.fence })
      })
    }))

    await postHook(currentRegistration.hookUrl, {
      hook_event_name: 'UserPromptSubmit', session_id: 'provider-current', cwd: root
    })
    await postHook(currentRegistration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'provider-current', cwd: root
    })
    expect(sessions.getResumeBinding('session-1', 'claude-code')?.metadata)
      .toMatchObject({ lastHookEvent: 'Stop', inheritedConversation: true, canFork: true })
    expect(identityEvents.at(-1)).toEqual(expect.objectContaining({
      runId: 'run-current-fork', eventName: 'Stop'
    }))
    expect(identityEvents.at(-1)).not.toHaveProperty('forkAuthority')
  })

  it('persists identity from the first supported follow-up hook when HTTP SessionStart does not fire', async () => {
    const registration = await hooks.registerClaudeSession({
      runId: 'run-1', sessionId: 'session-1', permissionMode: 'bypassPermissions',
      providerConfigId: 'bound-provider-config', model: 'session-model'
    })

    const response = await postHook(registration.hookUrl, {
      hook_event_name: 'UserPromptSubmit', session_id: 'claude-session-42', cwd: root
    })

    expect(response.status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'claude-session-42', resumeState: 'available',
      metadata: {
        providerConfigId: 'bound-provider-config', model: 'session-model',
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

  it('does not let an old launch registration overwrite a newer persisted model', async () => {
    sessions.recordResumableProviderIdentity(command('existing-model-binding'), {
      id: 'binding-existing-model', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'claude-existing-model',
      metadata: { model: 'claude-opus-4-6', permissionMode: 'default' }, now: 3
    })
    const oldRun = await hooks.registerClaudeSession({
      runId: 'run-old-model', sessionId: 'session-1', model: 'claude-opus-4-6'
    })
    sessions.updateProviderLaunchSettings(command('switch-model'), {
      bindingId: 'binding-existing-model', providerConfigId: 'anthropic-official',
      model: 'claude-sonnet-4-6', permissionMode: 'default', now: 4
    })

    expect((await postHook(oldRun.hookUrl, {
      hook_event_name: 'Stop', session_id: 'claude-existing-model'
    })).status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'claude-existing-model',
      metadata: { model: 'claude-sonnet-4-6', lastHookEvent: 'Stop' }
    })
  })

  it('reconciles an exact model id reported by the current provider hook', async () => {
    sessions.recordResumableProviderIdentity(command('existing-hook-model-binding'), {
      id: 'binding-hook-model', sessionId: 'session-1', provider: 'claude-code',
      providerSessionId: 'claude-hook-model',
      metadata: { model: 'claude-opus-4-6', permissionMode: 'default' }, now: 3
    })
    const registration = await hooks.registerClaudeSession({
      runId: 'run-hook-model', sessionId: 'session-1', model: 'claude-opus-4-6'
    })

    expect((await postHook(registration.hookUrl, {
      hook_event_name: 'Stop', session_id: 'claude-hook-model',
      model: { id: 'claude-sonnet-4-6', display_name: 'Claude Sonnet 4.6' }
    })).status).toBe(200)
    expect(sessions.getResumeBinding('session-1', 'claude-code')).toMatchObject({
      providerSessionId: 'claude-hook-model',
      metadata: { model: 'claude-sonnet-4-6', lastHookEvent: 'Stop' }
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

  it('registers Antigravity statusLine settings, passes through user statusLine stdout, discards broken statusline transcript_path, and maps work statuses', async () => {
    const userSettingsPath = join(root, 'user-agy-settings.json')
    await writeFile(userSettingsPath, JSON.stringify({
      statusLine: { type: 'command', command: "printf 'CUSTOM_BAR\n'" }
    }), 'utf8')

    const workStatuses: unknown[] = []
    const agyNotifications: unknown[] = []
    const agyHooks = new ProviderHookServer(root, sessions, {
      antigravitySettingsPath: userSettingsPath,
      onWorkStatus: (event) => { workStatuses.push(event) },
      onNotification: (event) => { agyNotifications.push(event) }
    })
    await agyHooks.start()
    try {
      const registration = await agyHooks.registerAntigravitySession({
        runId: 'run-agy-1', sessionId: 'session-1',
        systemMessageMarkdown: 'Antigravity host instructions'
      })

      const settings = JSON.parse(await readFile(registration.settingsPath, 'utf8'))
      expect(settings).toEqual({
        statusLine: {
          type: 'command',
          command: expect.stringContaining('.statusline.sh'),
          stack_with_default: true
        }
      })
      expect(settings.notifications).toBeUndefined()
      expect(JSON.parse(await readFile(registration.systemMessagePath!, 'utf8'))).toEqual({
        injectSteps: [{ systemMessage: { systemMessage: 'Antigravity host instructions' } }]
      })

      // Execute generated .statusline.sh and confirm background POST + user stdout passthrough.
      const { spawn } = await import('node:child_process')
      const statuslineOut = await new Promise<string>((resolve, reject) => {
        const child = spawn(settings.statusLine.command, [], { env: { PATH: process.env.PATH } })
        const out: Buffer[] = []
        child.stdout.on('data', (chunk) => out.push(Buffer.from(chunk)))
        child.on('error', reject)
        child.on('close', (code) => {
          expect(code).toBe(0)
          resolve(Buffer.concat(out).toString('utf8'))
        })
        child.stdin.end(JSON.stringify({
          conversation_id: 'agy-conv-1',
          transcript_path: '/Users/demo/.gemini/antigravity/brain/broken',
          agent_state: 'initializing',
          cycle_mode: 'auto'
        }))
      })
      expect(statuslineOut).toBe('CUSTOM_BAR\n')
      await waitFor(() => sessions.getResumeBinding('session-1', 'antigravity')?.providerSessionId === 'agy-conv-1', 2_000)

      // Statusline transitions: working -> tool_use (deduped) -> idle, plus Stop ERROR.
      await postHook(`${registration.hookUrl}?kind=statusline`, {
        conversation_id: 'agy-conv-1', agent_state: 'working', cycle_mode: 'auto'
      })
      await postHook(`${registration.hookUrl}?kind=statusline`, {
        conversation_id: 'agy-conv-1', agent_state: 'tool_use', cycle_mode: 'auto'
      })
      await postHook(`${registration.hookUrl}?kind=statusline`, {
        conversation_id: 'agy-conv-1', agent_state: 'tool_use', cycle_mode: 'auto'
      })
      await postHook(registration.hookUrl, {
        hookEventName: 'Stop',
        conversationId: 'agy-conv-1',
        terminationReason: 'ERROR',
        error: 'upstream timeout'
      })
      // Error stays latched even if a trailing idle statusline tick arrives.
      await postHook(`${registration.hookUrl}?kind=statusline`, {
        conversation_id: 'agy-conv-1', agent_state: 'idle', cycle_mode: 'auto'
      })

      expect(workStatuses.map((item) => (item as { workStatus: string }).workStatus)).toEqual([
        'starting', 'running', 'needs-input', 'error'
      ])
      expect(agyNotifications).toHaveLength(2)
      expect(agyNotifications[0]).toMatchObject({
        provider: 'antigravity',
        event: { eventType: 'permission', title: 'Antigravity' }
      })
      expect(agyNotifications[1]).toMatchObject({
        provider: 'antigravity',
        event: { eventType: 'error', title: 'Antigravity', body: 'upstream timeout' }
      })
    } finally {
      await agyHooks.stop()
    }
  })

  it('holds Antigravity fork identity until statusline/hook reports a conversation ID different from parent', async () => {
    sessions.createSession(command('agy-fork-source'), {
      id: 'agy-fork-source', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'antigravity', title: 'Antigravity', now: 2
    })
    sessions.createSession(command('agy-fork-child'), {
      id: 'agy-fork-child', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'antigravity', title: 'Agy Child', now: 2
    })
    const intents = new SessionForkIntentRepository(database)
    const now = Date.now()
    intents.accept({
      operationId: 'agy-op', submissionKey: 'agy-sub',
      sessionId: 'agy-fork-child', sourceSessionId: 'agy-fork-source',
      sourceProvider: 'antigravity', sourceProviderSessionId: 'agy-parent-id',
      permissionMode: 'default', displayName: 'Agy Child',
      worktreeMode: 'current', totalSteps: 2, now
    })
    const acquired = intents.acquireLease({
      operationId: 'agy-op', owner: 'runtime', now, ttlMs: 60_000
    })
    if (acquired.kind !== 'acquired') throw new Error('Antigravity fork lease missing')
    intents.advanceStage({
      operationId: 'agy-op', lease: acquired.lease, stage: 'restoring-provider', now
    })

    const slashReadyEvents: Array<{ sessionId: string; runId: string; parentProviderSessionId: string }> = []
    const recordedIdentities: Array<{ sessionId: string; providerSessionId: string }> = []
    const agyHooks = new ProviderHookServer(root, sessions, {
      onForkSlashTargetReady: (event) => { slashReadyEvents.push(event) },
      onIdentityRecorded: (event) => {
        recordedIdentities.push({ sessionId: event.sessionId, providerSessionId: event.providerSessionId })
      }
    })
    await agyHooks.start()
    try {
      const reg = await agyHooks.registerAntigravitySession({
        runId: 'agy-fork-run', sessionId: 'agy-fork-child',
        inheritedConversation: true,
        forkSourceProviderSessionId: 'agy-parent-id',
        forkAuthority: {
          operationId: 'agy-op', runId: 'agy-fork-run', lease: acquired.lease
        }
      })

      // 1. Authenticating / empty ID / parent ID working must NOT accept identity or fire slash-fork.
      await postHook(`${reg.hookUrl}?kind=statusline`, { conversation_id: '', agent_state: 'authenticating' })
      await postHook(`${reg.hookUrl}?kind=statusline`, { conversation_id: 'agy-parent-id', agent_state: 'working' })
      expect(slashReadyEvents).toEqual([])
      expect(recordedIdentities).toEqual([])
      expect(database.get<{ stage: string }>(
        'SELECT stage FROM session_fork_intents WHERE operation_id = ?', 'agy-op'
      )?.stage).toBe('restoring-provider')

      // 2. Idle on parent ID triggers slash-fork ready, still holds child identity until new ID arrives.
      await postHook(`${reg.hookUrl}?kind=statusline`, { conversation_id: 'agy-parent-id', agent_state: 'idle' })
      expect(slashReadyEvents).toEqual([{
        runId: 'agy-fork-run', sessionId: 'agy-fork-child',
        provider: 'antigravity', parentProviderSessionId: 'agy-parent-id'
      }])
      expect(recordedIdentities).toEqual([])
      expect(sessions.getResumeBinding('agy-fork-child', 'antigravity')).toBeUndefined()

      // 3. New conversation ID !== parent ID confirms child identity and completes fork intent.
      await postHook(`${reg.hookUrl}?kind=statusline`, { conversation_id: 'agy-child-id', agent_state: 'idle' })
      expect(recordedIdentities).toEqual([{ sessionId: 'agy-fork-child', providerSessionId: 'agy-child-id' }])
      expect(sessions.getResumeBinding('agy-fork-child', 'antigravity')).toMatchObject({
        providerSessionId: 'agy-child-id',
        resumeState: 'available'
      })
      expect(database.get<{ stage: string }>(
        'SELECT stage FROM session_fork_intents WHERE operation_id = ?', 'agy-op'
      )?.stage).toBe('succeeded')

      // 4. Stale/delayed statusline on parent ID after switch cannot revert child identity.
      await postHook(`${reg.hookUrl}?kind=statusline`, { conversation_id: 'agy-parent-id', agent_state: 'idle' })
      expect(slashReadyEvents).toHaveLength(1)
      expect(sessions.getResumeBinding('agy-fork-child', 'antigravity')?.providerSessionId).toBe('agy-child-id')
    } finally {
      await agyHooks.stop()
    }
  })

  it('observes Antigravity auto title from conversation_metadata.json after Stop without overwriting manual titles', async () => {
    sessions.createSession(command('agy-title-session'), {
      id: 'agy-title-session', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'antigravity', title: 'Antigravity', now: 1
    })
    const antigravityRoot = join(root, 'agy-title-root')
    await mkdir(join(antigravityRoot, 'cache'), { recursive: true })
    const metadataFile = join(antigravityRoot, 'cache', 'conversation_metadata.json')
    await writeFile(metadataFile, JSON.stringify({
      conversations: {
        'agy-title-conv': { summary: { ID: 'agy-title-conv', Title: '', Preview: 'Should not use preview' } }
      }
    }))

    const observed: Array<{ sessionId: string; title: string; provider: string }> = []
    const agyHooks = new ProviderHookServer(root, sessions, {
      antigravityRoot,
      onTitleObserved: ({ sessionId, title, provider }) => {
        observed.push({ sessionId, title, provider })
        sessions.observeProviderTitle(command(`observe-${title}`), {
          sessionId, title, now: Date.now()
        })
      }
    })
    await agyHooks.start()
    try {
      const reg = await agyHooks.registerAntigravitySession({
        runId: 'agy-title-run', sessionId: 'agy-title-session'
      })
      await postHook(reg.hookUrl, {
        hook_event_name: 'Stop', conversation_id: 'agy-title-conv'
      })

      // Write Title shortly after Stop (simulating async metadata flush by agy)
      await new Promise((resolve) => setTimeout(resolve, 25))
      await writeFile(metadataFile, JSON.stringify({
        conversations: {
          'agy-title-conv': { summary: { ID: 'agy-title-conv', Title: '自动汇总会话标题', Preview: 'Preview' } }
        }
      }))

      await waitFor(() => observed.length === 1, 2_000)
      expect(observed[0]).toEqual({
        sessionId: 'agy-title-session',
        title: '自动汇总会话标题',
        provider: 'antigravity'
      })
      expect(sessions.getSession('agy-title-session')).toMatchObject({
        title: '自动汇总会话标题',
        titleSource: 'auto',
        providerTitle: '自动汇总会话标题'
      })

      // Manual rename locks title; subsequent Stop update updates providerTitle only and restore works
      sessions.renameSession(command('manual-rename'), {
        sessionId: 'agy-title-session', title: '手动命名', now: 10
      })
      await writeFile(metadataFile, JSON.stringify({
        conversations: {
          'agy-title-conv': { summary: { ID: 'agy-title-conv', Title: '更新后的自动标题' } }
        }
      }))
      await postHook(reg.hookUrl, {
        hook_event_name: 'Stop', conversation_id: 'agy-title-conv'
      })
      await waitFor(() => observed.length === 2, 2_000)
      expect(sessions.getSession('agy-title-session')).toMatchObject({
        title: '手动命名',
        titleSource: 'manual',
        providerTitle: '更新后的自动标题'
      })

      const restored = sessions.restoreProviderTitle(command('restore-auto'), {
        sessionId: 'agy-title-session', now: 11
      }).result
      expect(restored).toMatchObject({
        title: '更新后的自动标题',
        titleSource: 'auto',
        providerTitle: '更新后的自动标题'
      })
    } finally {
      await agyHooks.stop()
    }
  })

  it('observes Antigravity titles written only to conversation_summaries.db after Stop', async () => {
    sessions.createSession(command('agy-db-title-session'), {
      id: 'agy-db-title-session', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'antigravity', title: 'Antigravity', now: 1
    })
    const antigravityRoot = join(root, 'agy-db-title-root')
    await mkdir(join(antigravityRoot, 'cache'), { recursive: true })
    const db = new DatabaseSync(join(antigravityRoot, 'conversation_summaries.db'))
    db.exec(
      `CREATE TABLE conversation_summaries (
        conversation_id TEXT PRIMARY KEY,
        title TEXT NOT NULL DEFAULT '',
        app_data_dir TEXT NOT NULL DEFAULT ''
      )`
    )

    const observed: Array<{ sessionId: string; title: string }> = []
    const agyHooks = new ProviderHookServer(root, sessions, {
      antigravityRoot,
      onTitleObserved: ({ sessionId, title }) => {
        observed.push({ sessionId, title })
        sessions.observeProviderTitle(command(`observe-db-${title}`), {
          sessionId, title, now: Date.now()
        })
      }
    })
    await agyHooks.start()
    try {
      const reg = await agyHooks.registerAntigravitySession({
        runId: 'agy-db-title-run', sessionId: 'agy-db-title-session'
      })
      await postHook(reg.hookUrl, {
        hook_event_name: 'Stop', conversation_id: 'agy-db-conv'
      })

      await new Promise((resolve) => setTimeout(resolve, 25))
      db.prepare(
        `INSERT INTO conversation_summaries (conversation_id, title, app_data_dir) VALUES (?, ?, ?)`
      ).run('agy-db-conv', '数据库自动标题', 'antigravity-cli')

      await waitFor(() => observed.length === 1, 2_000)
      expect(observed[0]).toEqual({ sessionId: 'agy-db-title-session', title: '数据库自动标题' })
      expect(sessions.getSession('agy-db-title-session')).toMatchObject({
        title: '数据库自动标题',
        titleSource: 'auto',
        providerTitle: '数据库自动标题'
      })
    } finally {
      db.close()
      await agyHooks.stop()
    }
  })

  it('evaluates Antigravity PreToolUse mt commands strictly and leaves other tools/unsafe shells empty', async () => {
    const mtPath = '/Applications/Matou.app/Contents/Resources/runtime/control-assets/bin/mt'
    const spacedMtPath = '/Applications/Matou App/Contents/Resources/runtime/control-assets/bin/mt'
    const expectedAllow = {
      decision: 'allow',
      overwrite: { BypassSandbox: true }
    }

    const allowedCommands = [
      'mt list --json',
      'mt read right --lines 20',
      'mt send right "hello world" --enter',
      '  mt send left \'single quoted ok\' --enter  ',
      `${mtPath} list --json`
    ]
    for (const cmd of allowedCommands) {
      expect(evaluateAntigravityPreToolUse({
        hookEventName: 'PreToolUse',
        toolCall: { name: 'run_command', args: { CommandLine: cmd } }
      }, { mtExecutablePath: mtPath }), cmd).toEqual(expectedAllow)
    }

    expect(evaluateAntigravityPreToolUse({
      hookEventName: 'PreToolUse',
      toolCall: { name: 'run_command', args: { CommandLine: `"${spacedMtPath}" list --json` } }
    }, { mtExecutablePath: spacedMtPath })).toEqual(expectedAllow)

    const rejectedCommands = [
      'mt list; rm -rf x',
      'mt list && curl https://example.com',
      'mt read $(whoami)',
      'mt send right `id`',
      'mt list | cat',
      'mt list > /tmp/out',
      'mt list < /tmp/in',
      'mt {a,b}',
      'mtx list',
      'echo mt',
      'mt list\necho injected',
      'mt list\r\necho injected',
      '/usr/local/bin/mt list',
      'python3 -c "print(1)"',
      ''
    ]
    for (const cmd of rejectedCommands) {
      expect(evaluateAntigravityPreToolUse({
        hookEventName: 'PreToolUse',
        toolCall: { name: 'run_command', args: { CommandLine: cmd } }
      }, { mtExecutablePath: mtPath }), cmd).toBeUndefined()
    }

    expect(evaluateAntigravityPreToolUse({
      hookEventName: 'PreToolUse',
      toolCall: { name: 'view_file', args: { CommandLine: 'mt list --json' } }
    }, { mtExecutablePath: mtPath })).toBeUndefined()

    // End-to-end HTTP endpoint (?kind=pre-tool-use) on ProviderHookServer
    sessions.createSession(command('agy-pretool-session'), {
      id: 'agy-pretool-session', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'antigravity', title: 'Antigravity', now: 1
    })
    const hud = new SessionHudRegistry()
    hud.spawn({ sessionId: 'agy-pretool-session', profile: 'antigravity', cwd: root, startedAt: 1 })
    const workStatuses: string[] = []
    const agyHooks = new ProviderHookServer(root, sessions, {
      onHudPayload: ({ sessionId, payload }) => { hud.ingestProvider(sessionId, payload) },
      onWorkStatus: (ev) => { workStatuses.push(`${ev.sessionId}:${ev.workStatus}`) }
    })
    await agyHooks.start()
    try {
      const reg = await agyHooks.registerAntigravitySession({
        runId: 'agy-pretool-run',
        sessionId: 'agy-pretool-session',
        mtExecutablePath: mtPath
      })
      const allowRes = await postHook(`${reg.hookUrl}?kind=pre-tool-use`, {
        hookEventName: 'PreToolUse',
        conversationId: 'agy-pretool-conv',
        toolCall: { id: 'c1', name: 'run_command', args: { CommandLine: 'mt send right "hello world" --enter' } }
      })
      expect(allowRes.status).toBe(200)
      expect(await allowRes.json()).toEqual(expectedAllow)
      expect(workStatuses).toContain('agy-pretool-session:running')
      expect(hud.snapshot('agy-pretool-session')?.runningTools).toEqual([
        { name: 'run_command', target: 'mt send right "hello world"...' }
      ])

      await postHook(reg.hookUrl, {
        hookEventName: 'PostToolUse',
        conversationId: 'agy-pretool-conv',
        toolCall: { id: 'c1', name: 'run_command', args: { CommandLine: 'mt send right "hello world" --enter' } }
      })
      expect(hud.snapshot('agy-pretool-session')?.runningTools).toEqual([])
      expect(hud.snapshot('agy-pretool-session')?.toolCounts).toEqual([{ name: 'run_command', count: 1 }])

      const absAllowRes = await postHook(`${reg.hookUrl}?kind=pre-tool-use`, {
        hookEventName: 'PreToolUse',
        conversationId: 'agy-pretool-conv',
        toolCall: { id: 'c2', name: 'run_command', args: { CommandLine: `${mtPath} read right --lines 20` } }
      })
      expect(absAllowRes.status).toBe(200)
      expect(await absAllowRes.json()).toEqual(expectedAllow)

      const unsafeRes = await postHook(`${reg.hookUrl}?kind=pre-tool-use`, {
        hookEventName: 'PreToolUse',
        conversationId: 'agy-pretool-conv',
        toolCall: { id: 'c3', name: 'run_command', args: { CommandLine: 'mt list; rm -rf x' } }
      })
      expect(unsafeRes.status).toBe(204)
      expect(await unsafeRes.text()).toBe('')

      const otherToolRes = await postHook(`${reg.hookUrl}?kind=pre-tool-use`, {
        hookEventName: 'PreToolUse',
        conversationId: 'agy-pretool-conv',
        toolCall: { id: 'c4', name: 'view_file', args: { AbsolutePath: '/tmp/foo' } }
      })
      expect(otherToolRes.status).toBe(204)
      expect(await otherToolRes.text()).toBe('')

      reg.retire(1_000, { suppressSideEffects: true })
      const retiredRes = await postHook(`${reg.hookUrl}?kind=pre-tool-use`, {
        hookEventName: 'PreToolUse',
        conversationId: 'agy-pretool-conv',
        toolCall: { id: 'c5', name: 'run_command', args: { CommandLine: 'mt list --json' } }
      })
      expect(retiredRes.status).toBe(204)
      expect(await retiredRes.text()).toBe('')
    } finally {
      await agyHooks.stop()
    }
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

async function waitFor(predicate: () => boolean, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!predicate() && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  expect(predicate()).toBe(true)
}

it('observes late Codex index titles, keeps manual names locked, restores latest auto title, and fences retired runs', async () => {
  const codexRoot = join(root, 'codex-title')
  await mkdir(codexRoot)
  const index = join(codexRoot, 'session_index.jsonl')
  const id = '01a1ddd6-4064-70e1-a006-79d310600776'
  let run = 'codex-title-run'
  const observed: string[] = []
  sessions.createSession(command('codex-title-session'), {
    id: 'codex-title-session', taskId: 'task-1', executionContextId: 'context-1', kind: 'codex', title: 'Codex', now: 1
  })
  const server = new ProviderHookServer(root, sessions, {
    codexRoot, currentRunId: () => run,
    onTitleObserved: ({ sessionId, title }) => {
      observed.push(title)
      sessions.observeProviderTitle(command(`codex-title-${observed.length}`), { sessionId, title, now: observed.length + 10 })
    }
  })
  await server.start()
  try {
    const registration = await server.registerCodexSession({ runId: run, sessionId: 'codex-title-session' })
    const hook = (hook_event_name: string) => postHook(registration.hookUrl, { hook_event_name, session_id: id })
    await hook('SessionStart')
    expect(sessions.getSession('codex-title-session')?.title).toBe('Codex')
    await writeFile(index, JSON.stringify({ id, thread_name: 'First automatic title', updated_at: '2026-11-15T06:13:00Z' }) + '\n')
    await waitFor(() => observed.length === 1, 2000)
    expect(sessions.getSession('codex-title-session')).toMatchObject({ title: 'First automatic title', titleSource: 'auto' })
    sessions.renameSession(command('manual-codex-title'), { sessionId: 'codex-title-session', title: 'My title', now: 20 })
    await appendFile(index, JSON.stringify({ id, thread_name: 'New automatic title', updated_at: '2026-11-15T06:14:00Z' }) + '\n')
    await waitFor(() => observed.length === 2, 2000)
    expect(sessions.getSession('codex-title-session')).toMatchObject({ title: 'My title', titleSource: 'manual', providerTitle: 'New automatic title' })
    expect(sessions.restoreProviderTitle(command('restore-codex-title'), { sessionId: 'codex-title-session', now: 21 }).result)
      .toMatchObject({ title: 'New automatic title', titleSource: 'auto' })
    run = 'replacement-run'
    await appendFile(index, JSON.stringify({ id, thread_name: 'Retired title', updated_at: '2026-11-15T06:15:00Z' }) + '\n')
    await hook('Stop')
    await new Promise(resolve => setTimeout(resolve, 70))
    expect(observed).toEqual(['First automatic title', 'New automatic title'])
  } finally { await server.stop() }
})

it('rejects a Codex Fork returning the parent UUID before binding, and revokes all subsequent side effects', async () => {
  const parentId = '01a1ddd6-4064-70e1-a006-000000000001'
  const childId = '01a1ddd6-4064-70e1-a006-000000000002'
  const mismatches: unknown[] = []
  const effects: unknown[] = []
  const server = new ProviderHookServer(root, sessions, {
    codexRoot: join(root, 'missing-codex'),
    onIdentityMismatch: event => { mismatches.push(event) },
    onHudPayload: event => { effects.push(event) }
  })
  await server.start()
  try {
    const registration = await server.registerCodexSession({
      sessionId: 'session-1', runId: 'codex-parent-returned', forkSourceProviderSessionId: parentId
    })
    await postHook(registration.hookUrl, { hook_event_name: 'SessionStart', session_id: parentId })
    expect(mismatches).toEqual([expect.objectContaining({ provider: 'codex', expectedProviderSessionId: parentId, actualProviderSessionId: parentId })])
    expect(sessions.getResumeBinding('session-1', 'codex')).toBeUndefined()
    await postHook(registration.hookUrl, { hook_event_name: 'SessionStart', session_id: childId })
    expect(sessions.getResumeBinding('session-1', 'codex')).toBeUndefined()
    expect(effects).toEqual([])
  } finally { await server.stop() }
})

it('accepts only a new valid Codex Fork UUID and leaves the parent binding/title unchanged', async () => {
  const parentId = '01a1ddd6-4064-70e1-a006-000000000001'
  const childId = '01a1ddd6-4064-70e1-a006-000000000002'
  sessions.createSession(command('codex-fork-parent-stage4'), {
    id: 'codex-fork-parent-stage4', kind: 'codex', taskId: 'task-1', executionContextId: 'context-1', title: 'Parent title', now: 1
  })
  sessions.recordResumableProviderIdentity(command('codex-parent-stage4-binding'), {
    id: 'codex-parent-stage4-binding', sessionId: 'codex-fork-parent-stage4', provider: 'codex',
    providerSessionId: parentId, metadata: { canFork: true }, now: 2
  })
  const parent = sessions.getResumeBinding('codex-fork-parent-stage4', 'codex')
  const registration = await hooks.registerCodexSession({
    sessionId: 'session-1', runId: 'codex-new-child', forkSourceProviderSessionId: parentId, inheritedConversation: true
  })
  await postHook(registration.hookUrl, { hook_event_name: 'SessionStart', session_id: childId })
  expect(sessions.getResumeBinding('session-1', 'codex')).toMatchObject({ providerSessionId: childId, metadata: { canFork: true } })
  expect(sessions.getResumeBinding('codex-fork-parent-stage4', 'codex')).toEqual(parent)
  expect(sessions.getSession('codex-fork-parent-stage4')?.title).toBe('Parent title')
})
