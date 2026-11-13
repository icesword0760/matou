import { EventEmitter } from 'node:events'
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, type RuntimeMessage } from '@matou/contracts'
import { AntigravityPluginInstaller } from './agents/antigravity-plugin-installer'
import { SessionRepository } from './domain/session-repository'
import { HierarchyApplicationService } from './hierarchy/hierarchy-application-service'
import { AgentNotificationRepository } from './notifications/agent-notification-repository'
import { RuntimeServer, type PortMessageEvent, type RuntimePort } from './runtime-server'
import { ForkOperationCoordinator } from './session-canvas/fork-operation-coordinator'
import { ForkWorkflowService } from './session-canvas/fork-workflow-service'
import { SessionCanvasService } from './session-canvas/session-canvas-service'
import { ProviderModeService } from './session-canvas/provider-mode-service'
import { SessionWorkStatusService } from './session-canvas/session-work-status-service'
import { ProviderHookServer } from './session/provider-hook-server'
import { createProviderHudPayloadHandler } from './session/provider-hud-payload-handler'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { SessionForkIntentRepository } from './session/session-fork-intent-repository'
import { SessionHudRegistry } from './session/session-hud-registry'
import { RuntimeDatabase } from './storage/database'
import { DomainTransactionManager } from './storage/domain-transaction'
import { MigrationRunner } from './storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from './storage/migrations'

describe('Antigravity RuntimeServer integration (mocked agy)', () => {
  let root: string
  let database: RuntimeDatabase
  let sessions: RuntimeSessionRegistry
  let hookServer: ProviderHookServer
  let hud: SessionHudRegistry
  let server: RuntimeServer
  let port: MockPort
  let previousCommand: string | undefined
  let previousIgnoreShiftTab: string | undefined
  let pluginDir: string
  let launchLogPath: string
  let forkCoordinator: ForkOperationCoordinator | undefined
  let sessionRepository: SessionRepository
  let providerModes: ProviderModeService
  let workStatuses: SessionWorkStatusService
  let notifications: AgentNotificationRepository

  beforeEach(async () => {
    root = await mkdtemp(join(tmpdir(), 'matou-agy-runtime-'))
    pluginDir = join(root, 'plugin-install', 'matou')
    launchLogPath = join(root, 'launches.jsonl')
    database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    seedWorkspace(database, root)

    const transactions = new DomainTransactionManager(database)
    sessionRepository = new SessionRepository(database, transactions)
    providerModes = new ProviderModeService(database, transactions)
    workStatuses = new SessionWorkStatusService(database, transactions)
    notifications = new AgentNotificationRepository(database, transactions)
    sessions = new RuntimeSessionRegistry()
    hud = new SessionHudRegistry()
    port = new MockPort()

    const onProviderHudPayload = createProviderHudPayloadHandler({
      hud,
      currentRunId: (sessionId) => sessions.get(sessionId)?.runId,
      publish: (sessionId) => { server?.publishSessionHud(sessionId) },
      reportError: () => {}
    })

    hookServer = new ProviderHookServer(root, sessionRepository, {
      antigravitySettingsPath: join(root, 'missing-user-settings.json'),
      currentRunId: (sessionId) => sessions.get(sessionId)?.runId,
      onWorkStatus: ({ runId, sessionId, workStatus }) => {
        if (workStatuses.get(sessionId) === workStatus) return
        const now = Date.now()
        workStatuses.set({
          commandId: `work-${runId}-${now}`,
          commandType: 'session.provider-work-status',
          requestHash: `${sessionId}:${workStatus}:${now}`
        }, { sessionId, workStatus, now })
      },
      onNotification: (notification) => {
        const now = Date.now()
        notifications.publish({
          commandId: `notify-${notification.runId}-${now}`,
          commandType: 'agent.notification.publish',
          requestHash: `${notification.sessionId}:${now}`
        }, { ...notification, eventId: `ev-${now}`, now })
      },
      onHudPayload: onProviderHudPayload,
      onIdentityRecorded: ({ sessionId, runId, provider, providerSessionId, eventName, forkAuthority }) => {
        hud.markResumable(sessionId)
        providerModes.observeHook({
          commandId: `mode-${runId}-${eventName}`,
          commandType: 'provider-hook.mode',
          requestHash: `${sessionId}:${providerSessionId}:${eventName}`
        }, { sessionId, providerSessionId, eventName, provider, now: Date.now() })
        server.providerIdentityRecorded(sessionId, runId)
        server.flushSemanticEvents()
        if (forkAuthority) {
          void forkCoordinator?.confirmAuthoritativeIdentity(
            sessionId,
            providerSessionId,
            forkAuthority.operationId
          )
        }
      },
      onForkSlashTargetReady: ({ sessionId, runId }) => {
        server?.antigravityForkSlashTargetReady({ sessionId, runId })
      }
    })
    await hookServer.start()

    const mockJs = join(root, 'mock-agy.cjs')
    const mockBin = join(root, 'mock-agy.sh')
    await writeFile(mockJs, MOCK_AGY_SCRIPT, 'utf8')
    await writeFile(mockBin, `#!/bin/sh\nexec "${process.execPath}" "${mockJs}" "$@"\n`, 'utf8')
    await chmod(mockBin, 0o755)

    previousCommand = process.env.MATOU_ANTIGRAVITY_COMMAND
    previousIgnoreShiftTab = process.env.MOCK_AGY_IGNORE_SHIFT_TAB
    process.env.MATOU_ANTIGRAVITY_COMMAND = mockBin
    delete process.env.MOCK_AGY_IGNORE_SHIFT_TAB

    server = new RuntimeServer(port, root, database, undefined, undefined, sessions, hookServer, undefined, {
      hudRegistry: hud,
      antigravityPluginInstaller: new AntigravityPluginInstaller(pluginDir),
      antigravityPermissionSwitchTimeoutMs: 600,
      antigravityForkNewIdTimeoutMs: 1_200
    })
    port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'renderer-agy' })
  })

  afterEach(async () => {
    forkCoordinator?.stop()
    server.close()
    await sessions.shutdownAll()
    await hookServer.stop()
    database.close()
    if (previousCommand === undefined) delete process.env.MATOU_ANTIGRAVITY_COMMAND
    else process.env.MATOU_ANTIGRAVITY_COMMAND = previousCommand
    if (previousIgnoreShiftTab === undefined) delete process.env.MOCK_AGY_IGNORE_SHIFT_TAB
    else process.env.MOCK_AGY_IGNORE_SHIFT_TAB = previousIgnoreShiftTab
    await rm(root, { recursive: true, force: true })
  })

  it('launches Antigravity with settings, installs plugin, ingests HUD & quota, switches modes via Shift+Tab + cycle_mode, and respawns for bypassPermissions', async () => {
    registerSession(database, 'agy-s1', 'antigravity')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'agy-s1', executionContextId: 'ctx-1',
      profile: 'antigravity', cols: 100, rows: 30
    })

    await waitUntil(() => hud.snapshot('agy-s1')?.model === 'Gemini 4 Carbon (Medium)')
    const firstLaunch = (await readLaunches(launchLogPath))[0]!
    expect(firstLaunch.args).toContain('--settings')
    expect(firstLaunch.args).not.toContain('--model')
    expect(firstLaunch.env.MATOU_PROVIDER_HOOK_URL).toContain('/hooks/')
    expect(firstLaunch.env.MATOU_PROVIDER_HOOK_TOKEN).toBeTruthy()
    expect(await readFile(join(pluginDir, 'plugin.json'), 'utf8')).toContain('"matou"')

    const snapshot = hud.snapshot('agy-s1')!
    expect(snapshot.provider).toBe('antigravity')
    expect(snapshot.permissionMode).toBe('default')
    expect(snapshot.contextPercent).toBe(12)
    expect(snapshot.usageWindows).toEqual([
      { label: 'Gemini 5h', percent: 25, resetsAt: Date.parse('2026-11-13T11:14:59Z') }
    ])
    expect(workStatuses.get('agy-s1')).toBe('idle')

    // Switch default -> acceptEdits via Shift+Tab (auto -> plan -> null -> accept-edits) WITHOUT respawn.
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
      requestId: 'agy-mode-1', method: 'session.set-permission-mode',
      capability: 'renderer', deadlineAt: Date.now() + 3_000,
      payload: {
        command: { commandId: 'cmd-m1', commandType: 'session.set-permission-mode', requestHash: 'h1' },
        input: { sessionId: 'agy-s1', provider: 'antigravity', permissionMode: 'acceptEdits', respawn: false, now: 2 }
      }
    })
    await waitUntil(() => port.findRpcResponse('agy-mode-1') !== undefined)
    expect(await readLaunches(launchLogPath)).toHaveLength(1)
    expect(hud.snapshot('agy-s1')?.permissionMode).toBe('acceptEdits')

    // Timeout fallback: when Shift+Tab is ignored, switches cleanly by respawning with --mode plan + --conversation.
    await writeFile(join(root, 'ignore-shift-tab'), '1', 'utf8')
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
      requestId: 'agy-mode-2', method: 'session.set-permission-mode',
      capability: 'renderer', deadlineAt: Date.now() + 3_000,
      payload: {
        command: { commandId: 'cmd-m2', commandType: 'session.set-permission-mode', requestHash: 'h2' },
        input: { sessionId: 'agy-s1', provider: 'antigravity', permissionMode: 'plan', respawn: false, now: 3 }
      }
    })
    await waitUntil(() => port.findRpcResponse('agy-mode-2') !== undefined)
    const launchesAfterTimeout = await readLaunches(launchLogPath)
    expect(launchesAfterTimeout).toHaveLength(2)
    expect(launchesAfterTimeout[1]!.args).toEqual(expect.arrayContaining([
      '--conversation', 'agy-conv-1', '--mode', 'plan'
    ]))
    await waitUntil(() => !sessions.providerIdentityPending('agy-s1'))

    // Entering bypassPermissions respawns with --dangerously-skip-permissions.
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
      requestId: 'agy-mode-3', method: 'session.set-permission-mode',
      capability: 'renderer', deadlineAt: Date.now() + 3_000,
      payload: {
        command: { commandId: 'cmd-m3', commandType: 'session.set-permission-mode', requestHash: 'h3' },
        input: { sessionId: 'agy-s1', provider: 'antigravity', permissionMode: 'bypassPermissions', respawn: true, now: 4 }
      }
    })
    await waitUntil(() => port.findRpcResponse('agy-mode-3') !== undefined)
    const launchesAfterBypass = await readLaunches(launchLogPath)
    expect(launchesAfterBypass).toHaveLength(3)
    expect(launchesAfterBypass[2]!.args).toContain('--dangerously-skip-permissions')
  }, 15_000)

  it('promotes Shell panel to Antigravity on agy commands while ignoring non-interactive invocations', async () => {
    registerSession(database, 'agy-shell-1', 'shell')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'agy-shell-1', executionContextId: 'ctx-1',
      profile: 'shell', cols: 100, rows: 30
    })
    await waitUntil(() => sessions.get('agy-shell-1')?.profile === 'shell')

    // Unrecognized flag must stay in Shell.
    port.receive({ type: 'terminal.input', protocolVersion: PROTOCOL_VERSION, sessionId: 'agy-shell-1', data: 'agy --help\r' })
    await new Promise((resolve) => setTimeout(resolve, 120))
    expect(sessions.get('agy-shell-1')?.profile).toBe('shell')

    // Supported interactive `agy --mode accept-edits` promotes to Antigravity.
    port.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'agy-shell-1', data: 'agy --mode accept-edits\r'
    })
    await waitUntil(() => sessions.get('agy-shell-1')?.profile === 'antigravity', 6_000)
    expect(database.get<{ kind: string }>('SELECT kind FROM sessions WHERE id = ?', 'agy-shell-1')).toEqual({
      kind: 'antigravity'
    })
    await waitUntil(() => existsSync(launchLogPath), 6_000)
    const launches = await readLaunches(launchLogPath)
    expect(launches.at(-1)?.args).toEqual(expect.arrayContaining(['--mode', 'accept-edits']))
  })

  it('reports clear installation hint when agy executable is missing', async () => {
    process.env.MATOU_ANTIGRAVITY_COMMAND = join(root, 'missing-agy-bin')
    registerSession(database, 'agy-missing', 'antigravity')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'agy-missing', executionContextId: 'ctx-1',
      profile: 'antigravity', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('protocol.error') !== undefined)
    expect(port.last('protocol.error')?.message).toContain('Antigravity')
  })

  it('forks Antigravity via --conversation + idempotent /fork, preserves parent state, and fails/retries cleanly on timeout', async () => {
    const transactions = new DomainTransactionManager(database)
    const hierarchy = new HierarchyApplicationService(database, transactions)
    const forkWs = join(root, 'fork-ws')
    await mkdir(forkWs, { recursive: true })
    const bootstrapped = hierarchy.createWorkspace(
      { commandId: 'cmd-boot', commandType: 'hierarchy.create-workspace', requestHash: 'boot-agy' },
      { windowId: 'win-agy', rootDirectory: forkWs, name: 'ForkWS', now: 1 }
    )
    const parentId = bootstrapped.created!.session!.id
    const sceneId = bootstrapped.created!.scene!.id
    const executionContextId = bootstrapped.created!.session!.executionContextId

    // Promote parent to Antigravity and start live parent PTY.
    database.run(`UPDATE sessions SET kind = 'antigravity', title = 'Antigravity' WHERE id = ?`, parentId)
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: parentId, executionContextId,
      profile: 'antigravity', cols: 100, rows: 30
    })
    await waitUntil(() => hud.snapshot(parentId)?.model === 'Gemini 4 Carbon (Medium)')

    const forkWorkflow = new ForkWorkflowService(root, database, transactions, {
      stopRuns: async () => undefined
    })
    // Before 1 completed turn, fork must be blocked.
    expect(providerModes.canFork(parentId)).toBe(false)
    await expect(forkWorkflow.createForkChild(
      { commandId: 'cmd-fork-blocked', commandType: 'session.fork', requestHash: 'blocked' },
      { windowId: 'win-agy', sceneId, sourceSessionId: parentId, name: 'Early Child', worktreeMode: 'current', now: 2 }
    )).rejects.toMatchObject({ code: 'FORK_SOURCE_NOT_READY' })

    // Complete 1 turn on parent via PreInvocation + Stop hook payloads.
    const hookUrl = (await readLaunches(launchLogPath))[0]!.env.MATOU_PROVIDER_HOOK_URL!
    await fetch(hookUrl, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ hookEventName: 'PreInvocation', conversationId: 'agy-conv-1' })
    })
    await fetch(hookUrl, {
      method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ hookEventName: 'Stop', conversationId: 'agy-conv-1' })
    })
    expect(providerModes.canFork(parentId)).toBe(true)

    const intents = new SessionForkIntentRepository(database)
    const forkNotifications: unknown[] = []
    forkCoordinator = new ForkOperationCoordinator(intents, {
      ownerId: 'runtime-test',
      heartbeatMs: 50,
      executeFork: (command, input) => forkWorkflow.executeFork(command, input),
      startOrResume: async (sessionId, authority) => {
        const row = database.get<{ execution_context_id: string; kind: 'antigravity' }>(
          'SELECT execution_context_id, kind FROM sessions WHERE id = ?',
          sessionId
        )!
        return server.startOrResumeSession({
          sessionId,
          executionContextId: row.execution_context_id,
          profile: row.kind,
          cols: 100,
          rows: 30
        }, authority)
      },
      notify: (notification) => { forkNotifications.push(notification) }
    })
    forkCoordinator.start()

    const forked = await forkWorkflow.createForkChild(
      { commandId: 'cmd-fork-1', commandType: 'session.fork', requestHash: 'fork-1' },
      { windowId: 'win-agy', sceneId, sourceSessionId: parentId, name: 'Agy Branch', worktreeMode: 'current', now: 10 }
    )
    const childId = forked.session!.id

    await waitUntil(() => sessionRepository.getResumeBinding(childId, 'antigravity')?.providerSessionId === 'agy-forked-child-1', 6_000)
    await waitUntil(() => forkNotifications.length >= 1, 6_000)

    const launches = await readLaunches(launchLogPath)
    const childLaunch = launches.find((item) => item.args.includes('--conversation'))!
    expect(childLaunch.args).toEqual(expect.arrayContaining(['--conversation', 'agy-conv-1']))
    expect(childLaunch.args).not.toContain('--fork-session')

    // /fork\r must have been sent idempotently ONCE despite authenticating + duplicate idle ticks.
    const forkInputs = await readFile(join(root, 'fork-commands.log'), 'utf8')
    expect(forkInputs.trim().split('\n')).toHaveLength(1)

    // Parent binding, HUD and status must remain completely intact.
    expect(sessionRepository.getResumeBinding(parentId, 'antigravity')?.providerSessionId).toBe('agy-conv-1')
    expect(workStatuses.get(parentId)).toBe('idle')
    expect(hud.snapshot(parentId)).toMatchObject({
      provider: 'antigravity',
      model: 'Gemini 4 Carbon (Medium)',
      permissionMode: 'default'
    })

    // Failure & Retry: when /fork never switches ID within timeout, enters failed state; retry recovers cleanly.
    await writeFile(join(root, 'ignore-slash-fork'), '1', 'utf8')
    const failedCreate = await forkWorkflow.createForkChild(
      { commandId: 'cmd-fork-fail', commandType: 'session.fork', requestHash: 'fork-fail' },
      { windowId: 'win-agy', sceneId, sourceSessionId: parentId, name: 'Timeout Branch', worktreeMode: 'current', now: 20 }
    )
    const failedChildId = failedCreate.session!.id
    await waitUntil(() => intents.state(failedChildId) === 'failed', 6_000)

    await rm(join(root, 'ignore-slash-fork'), { force: true })
    await forkWorkflow.retryFork(
      { commandId: 'cmd-fork-retry', commandType: 'session.fork-retry', requestHash: 'fork-retry' },
      { windowId: 'win-agy', sceneId, sessionId: failedChildId, now: 30 }
    )
    await waitUntil(() => sessionRepository.getResumeBinding(failedChildId, 'antigravity')?.providerSessionId === 'agy-forked-child-1', 6_000)
  }, 20_000)
})

const MOCK_AGY_SCRIPT = `
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const argv = process.argv.slice(2);
const logPath = path.join(path.dirname(__filename), 'launches.jsonl');
let settingsPath;
for (let i = 0; i < argv.length; i += 1) {
  if (argv[i] === '--settings') settingsPath = argv[i + 1];
}
let initialMode = 'auto';
if (argv.includes('--dangerously-skip-permissions')) initialMode = undefined;
else {
  const idx = argv.indexOf('--mode');
  if (idx >= 0) initialMode = argv[idx + 1];
}

let conversationId = 'agy-conv-1';
let forkedMode = false;
const convIdx = argv.indexOf('--conversation');
if (convIdx >= 0 && argv[convIdx + 1]) {
  conversationId = argv[convIdx + 1];
  forkedMode = true;
}

fs.appendFileSync(logPath, JSON.stringify({
  args: argv,
  env: {
    MATOU_PROVIDER_HOOK_URL: process.env.MATOU_PROVIDER_HOOK_URL,
    MATOU_PROVIDER_HOOK_TOKEN: process.env.MATOU_PROVIDER_HOOK_TOKEN
  }
}) + '\\n');

let statusCmd;
if (settingsPath && fs.existsSync(settingsPath)) {
  try {
    statusCmd = JSON.parse(fs.readFileSync(settingsPath, 'utf8')).statusLine?.command;
  } catch {}
}

function sendStatus(cycleMode, agentState = 'idle', conv = conversationId) {
  if (!statusCmd) return;
  const payload = {
    conversation_id: conv,
    agent_state: agentState,
    ...(cycleMode === undefined ? {} : { cycle_mode: cycleMode }),
    model: { id: 'Gemini 4 Carbon (Medium)', display_name: 'Gemini 4 Carbon (Medium)', effort: 'medium' },
    context_window: { context_window_size: 1048576, used_percentage: 12 },
    quota: { 'gemini-5h': { remaining_fraction: 0.75, reset_time: '2026-11-13T11:14:59Z' } }
  };
  spawnSync(statusCmd, [], { input: JSON.stringify(payload), encoding: 'utf8' });
}

const cycleOrder = ['auto', 'plan', null, 'accept-edits'];
let cycleIndex = Math.max(0, cycleOrder.indexOf(initialMode ?? 'auto'));
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.setEncoding('utf8');
let inputBuffer = '';
process.stdin.on('data', (chunk) => {
  inputBuffer += chunk;
  while (inputBuffer.includes('/fork\\r') || inputBuffer.includes('/fork\\n')) {
    const idxR = inputBuffer.indexOf('/fork\\r');
    const idxN = inputBuffer.indexOf('/fork\\n');
    const idx = idxR >= 0 && idxN >= 0 ? Math.min(idxR, idxN) : Math.max(idxR, idxN);
    inputBuffer = inputBuffer.slice(idx + 6);
    fs.appendFileSync(path.join(path.dirname(__filename), 'fork-commands.log'), '/fork\\n');
    if (!fs.existsSync(path.join(path.dirname(__filename), 'ignore-slash-fork'))) {
      conversationId = 'agy-forked-child-1';
      sendStatus(initialMode, 'idle', conversationId);
    }
  }
  if (fs.existsSync(path.join(path.dirname(__filename), 'ignore-shift-tab'))) return;
  if (chunk.includes('\\u001b[Z')) {
    cycleIndex = (cycleIndex + 1) % cycleOrder.length;
    sendStatus(cycleOrder[cycleIndex]);
  }
});

if (forkedMode) {
  sendStatus(initialMode, 'authenticating', '');
  sendStatus(initialMode, 'idle', conversationId);
  sendStatus(initialMode, 'idle', conversationId);
} else {
  sendStatus(initialMode);
}
setInterval(() => {}, 10000);
`

class MockPort extends EventEmitter implements RuntimePort {
  readonly sent: RuntimeMessage[] = []
  postMessage(message: RuntimeMessage): void { this.sent.push(message) }
  start(): void {}
  close(): void {}
  receive(data: unknown): void { this.emit('message', { data } satisfies PortMessageEvent) }
  last<T extends RuntimeMessage['type']>(type: T): Extract<RuntimeMessage, { type: T }> | undefined {
    return this.sent.filter((m) => m.type === type).at(-1) as Extract<RuntimeMessage, { type: T }> | undefined
  }
  findRpcResponse(requestId: string): Extract<RuntimeMessage, { type: 'rpc.response' }> | undefined {
    return this.sent.find((m): m is Extract<RuntimeMessage, { type: 'rpc.response' }> =>
      m.type === 'rpc.response' && m.requestId === requestId
    )
  }
}

function seedWorkspace(database: RuntimeDatabase, root: string): void {
  database.transaction((tx) => {
    tx.run('INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'ws-1', 'WS', root, 1, 1)
    tx.run('INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)',
      'ctx-1', 'ws-1', 'plain-directory', root, 1)
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
      'task-1', 'ws-1', 'ctx-1', 'Task', 'active', 1, 1)
  })
}

function registerSession(database: RuntimeDatabase, id: string, kind: 'shell' | 'antigravity'): void {
  database.transaction((tx) => {
    tx.run(
      `INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at)
       VALUES (?, 'task-1', 'ctx-1', ?, 'exited', ?, 1, 1, 1)`,
      id, kind, id
    )
    tx.run(
      `INSERT INTO scenes (
         id, task_id, name, mode, root_node_id, title_pinned, sort_key,
         layout_revision, created_at, updated_at
       ) VALUES (?, 'task-1', 'Canvas', 'tile', ?, 0, 'a0', 1, 1, 1)`,
      `scene-${id}`, `node-${id}`
    )
    tx.run(
      `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
       VALUES (?, ?, 'root', 0, 1)`,
      `node-${id}`, `scene-${id}`
    )
    tx.run(
      `INSERT INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at)
       VALUES (?, ?, ?, ?, 1)`,
      `mount-${id}`, `scene-${id}`, `node-${id}`, id
    )
  })
}

async function readLaunches(path: string): Promise<Array<{ args: string[]; env: Record<string, string | undefined> }>> {
  const raw = await readFile(path, 'utf8')
  return raw.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line))
}

async function waitUntil(predicate: () => boolean, timeoutMs = 4_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error('condition timed out')
    await new Promise((resolve) => setTimeout(resolve, 15))
  }
}
