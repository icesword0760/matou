import { EventEmitter } from 'node:events'
import { execFile } from 'node:child_process'
import { chmod, mkdir, mkdtemp, readFile, readlink, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, type RpcMethod, type RuntimeMessage } from '@matou/contracts'

import { SegmentJournal, readSessionFrames } from './journal/segment-journal'
import { CheckpointManager } from './checkpoints/checkpoint-manager'
import {
  RuntimeServer, terminalSummaryLines, withSessionRuntimeEnvironment,
  type PortMessageEvent, type RuntimePort
} from './runtime-server'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { ProviderHookServer } from './session/provider-hook-server'
import { SessionRepository } from './domain/session-repository'
import { DomainTransactionManager } from './storage/domain-transaction'
import { RuntimeDatabase } from './storage/database'
import { MigrationRunner } from './storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from './storage/migrations'
import { AgentNotificationRepository } from './notifications/agent-notification-repository'

let root: string
let database: RuntimeDatabase
let port: MockPort
let server: RuntimeServer
const execFileAsync = promisify(execFile)

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-server-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seedReplayAuthority(database, root)
  port = new MockPort()
  server = new RuntimeServer(port, root, database)
  port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'renderer-1' })
  await settle()
})

afterEach(() => {
  server.close()
  database.close()
})

describe('RuntimeServer domain RPC', () => {
  it('adds current cwd and Git information to a direct DAG graph response', () => {
    const result = withSessionRuntimeEnvironment({
      sceneId: 'scene-1',
      nodes: [{ sessionId: 'session-1', cwd: '/old' }],
      edges: []
    }, [{
      sessionId: 'session-1', mode: 'shell', cwd: '/repo/deep',
      gitBranch: 'feature/dag', gitDirty: true, startedAt: 1
    }])

    expect(result).toMatchObject({
      nodes: [{
        sessionId: 'session-1', cwd: '/repo/deep',
        git: { branch: 'feature/dag', dirty: true }
      }]
    })
  })

  it('keeps the latest four readable terminal lines for live DAG cards', () => {
    expect(terminalSummaryLines('\u001b[31mone\u001b[0m\r\ntwo\nthree\nfour\nfive\n')).toEqual([
      'two', 'three', 'four', 'five'
    ])
  })
  it('publishes live per-Session HUD state in projection snapshots and terminal updates', async () => {
    registerSession(database, 'hud-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'hud-session',
      executionContextId: 'replay-context', profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('terminal.hud') !== undefined)
    expect(port.last('terminal.hud')).toMatchObject({
      sessionId: 'hud-session', hud: { sessionId: 'hud-session', mode: 'shell' }
    })

    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'hud-snapshot',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {}
    })
    await settle()
    expect(port.findRpcResponse('hud-snapshot')).toMatchObject({
      result: { hierarchy: { sessionHuds: [expect.objectContaining({ sessionId: 'hud-session' })] } }
    })
  })

  it('advertises replay and replays durable output after a Runtime reconnect', async () => {
    const journal = await SegmentJournal.open(root, 'persisted-session')
    registerSession(database, 'persisted-session')
    await journal.appendOutput(1, new TextEncoder().encode('first'))
    await journal.appendResize(2, 100, 40)
    await journal.appendOutput(3, new TextEncoder().encode('second'))
    await journal.appendExit(4, 0)
    await journal.close()

    expect(port.last('protocol.ready')?.capabilities).toContain('replay-v1')
    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'persisted-session', fromSequence: 2
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      sessionId: 'persisted-session', availableFromSequence: 1, liveSequence: 4
    })
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toEqual([
      expect.objectContaining({ sessionId: 'persisted-session', sequence: 3 })
    ])
    expect(port.last('terminal.exited')).toMatchObject({ sequence: 4, exitCode: 0 })
    expect(port.last('terminal.replay-complete')).toMatchObject({ throughSequence: 4 })
  })

  it('reports a retention gap before replaying the available suffix', async () => {
    const journal = await SegmentJournal.open(root, 'retained-session')
    registerSession(database, 'retained-session')
    await journal.appendOutput(7, new TextEncoder().encode('retained'))
    await journal.close()

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'retained-session', fromSequence: 1
    })
    await settle()

    expect(port.last('terminal.gap')).toMatchObject({
      requestedFromSequence: 1, availableFromSequence: 7, reason: 'retention'
    })
    expect(port.last('terminal.data')).toMatchObject({ sequence: 7 })
    expect(port.last('terminal.replay-complete')).toMatchObject({ throughSequence: 7 })
  })

  it('applies cumulative ACK backpressure while replaying a large persisted journal', async () => {
    const journal = await SegmentJournal.open(root, 'large-replay')
    registerSession(database, 'large-replay')
    for (let sequence = 1; sequence <= 3; sequence += 1) {
      await journal.appendOutput(sequence, new Uint8Array(600 * 1024))
    }
    await journal.close()

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'large-replay', fromSequence: 0
    })
    await settle()
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toHaveLength(2)
    expect(port.last('terminal.replay-complete')).toBeUndefined()

    port.receive({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'large-replay', throughSequence: 2
    })
    await settle()
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toHaveLength(3)
    expect(port.last('terminal.replay-complete')).toMatchObject({ throughSequence: 3 })
    expect(port.last('protocol.error')).toBeUndefined()
  })

  it('restores the latest paired checkpoint and only replays its journal tail', async () => {
    registerSession(database, 'checkpoint-replay')
    const journal = await SegmentJournal.open(root, 'checkpoint-replay')
    await journal.appendOutput(1, Uint8Array.from([65]))
    await journal.appendDomainCursor(2, 1)
    await journal.appendOutput(3, Uint8Array.from([66]))
    await journal.close()
    await new CheckpointManager(root, database).create({
      sessionId: 'checkpoint-replay', terminalSequence: 2, domainEventSequence: 1,
      screenEpoch: 4, snapshot: Uint8Array.from([9, 8, 7])
    })

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'checkpoint-replay', fromSequence: 0
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      checkpointSequence: 2,
      checkpoint: {
        terminalSequence: 2, domainEventSequence: 1, screenEpoch: 4,
        snapshot: Uint8Array.from([9, 8, 7])
      }
    })
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toEqual([
      expect.objectContaining({ sequence: 3, data: Uint8Array.from([66]) })
    ])
  })

  it('returns projection responses with runtime generation protection', async () => {
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'request-1',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {}
    })
    await settle()

    expect(port.last('rpc.response')).toMatchObject({
      requestId: 'request-1', runtimeGeneration: database.runtimeGeneration,
      result: {
        eventSequence: 0,
        hierarchy: {
          windowId: 'window-1',
          workspaces: [expect.objectContaining({ id: 'replay-workspace' })],
          tasks: [expect.objectContaining({ id: 'replay-task' })]
        }
      }
    })
  })

  it('rejects expired and pre-cancelled requests with structured errors', async () => {
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'expired',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: 1, payload: {}
    })
    port.receive({ type: 'rpc.cancel', protocolVersion: PROTOCOL_VERSION, requestId: 'cancelled' })
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'cancelled',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {}
    })
    await settle()

    expect(port.findRpcError('expired')).toMatchObject({ code: 'TIMEOUT', retryable: true })
    expect(port.findRpcError('cancelled')).toMatchObject({ code: 'CANCELLED', retryable: false })
  })

  it('drops terminal callbacks already queued when the Renderer port disconnects', async () => {
    const errorsBefore = port.sent.filter(({ type }) => type === 'protocol.error').length
    port.disconnect()
    port.receive({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'late-renderer-frame', throughSequence: 1
    })
    await settle()

    expect(port.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(errorsBefore)
  })

  it('treats an ACK for a just-disposed attached PTY as a harmless shutdown callback', async () => {
    const sessions = new RuntimeSessionRegistry()
    const shutdownPort = new MockPort()
    new RuntimeServer(shutdownPort, root, database, undefined, undefined, sessions)
    shutdownPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'shutdown-renderer'
    })
    shutdownPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'shutdown-session', executionContextId: 'local-default',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('shutdown-session'))
    const errorsBefore = shutdownPort.sent.filter(({ type }) => type === 'protocol.error').length

    sessions.disposeAll()
    shutdownPort.receive({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'shutdown-session', throughSequence: 1
    })
    shutdownPort.receive({
      type: 'terminal.resize', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'shutdown-session', cols: 100, rows: 30
    })
    await settle()

    expect(shutdownPort.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(errorsBefore)
    shutdownPort.disconnect()
  })

  it('pushes replayable domain batches directly to a subscribed Renderer', async () => {
    port.receive({
      type: 'events.subscribe', protocolVersion: PROTOCOL_VERSION,
      consumerId: 'renderer-1', afterSequence: 0, batchSize: 100
    })
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'create-workspace',
      method: 'workspace.create', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {
        command: { commandId: 'cmd-workspace', commandType: 'workspace.create', requestHash: 'hash' },
        input: { id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1 }
      }
    })
    await settle()

    expect(port.last('events.batch')).toMatchObject({
      consumerId: 'renderer-1', throughSequence: 1,
      events: [{ eventType: 'workspace.created' }]
    })
  })

  it('flushes a provider notification to an already subscribed Renderer without waiting for another RPC', async () => {
    port.receive({
      type: 'events.subscribe', protocolVersion: PROTOCOL_VERSION,
      consumerId: 'notification-renderer', afterSequence: 0, batchSize: 100
    })
    const transactions = new DomainTransactionManager(database)
    new AgentNotificationRepository(database, transactions).publish(
      { commandId: 'publish-notification', commandType: 'agent.notification.publish', requestHash: 'notification' },
      {
        eventId: 'provider-event', runId: 'run-1', sessionId: 'persisted-session', provider: 'claude-code',
        event: { eventType: 'completed', title: 'Claude Code', subtitle: 'Completed', body: '完成', sound: true, cooldownKey: 'Stop' },
        now: 10
      }
    )

    server.flushSemanticEvents()

    expect(port.last('events.batch')).toMatchObject({
      consumerId: 'notification-renderer', events: [{ eventType: 'agent.notification' }]
    })
  })

  it('binds a persisted Session to a Runtime generation SessionRun and closes it on PTY exit', async () => {
    registerSession(database, 'run-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'run-session',
      executionContextId: 'replay-context', profile: 'shell', cols: 80, rows: 24
    })
    await settle()
    const running = database.get<{ id: string; status: string; runtime_generation: string }>(
      'SELECT id, status, runtime_generation FROM session_runs WHERE session_id = ?',
      'run-session'
    )
    expect(running).toMatchObject({ status: 'running', runtime_generation: database.runtimeGeneration })

    port.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'run-session'
    })
    await waitUntil(() => database.get<{ status: string }>(
      'SELECT status FROM session_runs WHERE id = ?', running!.id
    )?.status === 'exited')
    expect(database.get('SELECT status FROM session_runs WHERE id = ?', running!.id)).toEqual({
      status: 'exited'
    })
  })

  it('persists an attached terminal interaction marker before its input is processed', async () => {
    registerCanvasSession(database, 'interaction-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'interaction-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('terminal.spawned') !== undefined)

    port.receive({
      type: 'terminal.user-interaction', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'interaction-session', interactionKind: 'submit'
    })
    port.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'interaction-session', data: "printf '__ORDERED_INPUT__\\n'\r"
    })

    await waitUntil(() => database.get<{ last_user_interaction_seq: number }>(
      `SELECT last_user_interaction_seq FROM session_canvas_memberships
       WHERE session_id = 'interaction-session'`
    )?.last_user_interaction_seq === 1)
    expect(database.get<{ event_type: string }>(
      `SELECT event_type FROM domain_events
       WHERE session_id = 'interaction-session' AND event_type = 'session.user-interacted'`
    )).toEqual({ event_type: 'session.user-interacted' })
    await waitUntil(() => terminalText(port).includes('__ORDERED_INPUT__'))
  })

  it('tracks real Shell commands as running, idle, error, and interrupted work', async () => {
    const previousShell = process.env.SHELL
    process.env.SHELL = '/bin/zsh'
    try {
      registerCanvasSession(database, 'work-status-session')
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => port.last('terminal.spawned') !== undefined)

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'sleep 0.35\r'
      })
      await waitUntil(() => workStatus('work-status-session') === 'running')
      await waitUntil(() => workStatus('work-status-session') === 'idle', 5_000)

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'false\r'
      })
      await waitUntil(() => workStatus('work-status-session') === 'error')

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'sleep 5\r'
      })
      await waitUntil(() => workStatus('work-status-session') === 'running')
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: '\u0003'
      })
      await waitUntil(() => workStatus('work-status-session') === 'interrupted', 5_000)
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })

  it('keeps a live PTY in the Runtime registry across Renderer disconnect and reattach', async () => {
    const priorRun = await SegmentJournal.open(root, 'reload-session')
    await priorRun.appendOutput(1, new TextEncoder().encode('output from an earlier app run'))
    await priorRun.close()
    const sessions = new RuntimeSessionRegistry()
    const firstPort = new MockPort()
    new RuntimeServer(firstPort, root, database, undefined, undefined, sessions)
    firstPort.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'reload-1' })
    firstPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'reload-session',
      executionContextId: 'local-default', profile: 'shell', cols: 80, rows: 24
    })
    await settle()
    const firstPid = firstPort.last('terminal.spawned')?.pid
    expect(firstPid).toBeTypeOf('number')
    firstPort.disconnect()

    const secondPort = new MockPort()
    new RuntimeServer(secondPort, root, database, undefined, undefined, sessions)
    secondPort.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'reload-2' })
    secondPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'reload-session',
      executionContextId: 'local-default', profile: 'shell', cols: 80, rows: 24
    })
    await settle()

    expect(secondPort.last('terminal.spawned')).toMatchObject({
      pid: firstPid, reattached: true, replayFromSequence: 2
    })
    secondPort.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'reload-session', fromSequence: 2
    })
    await waitUntil(() => secondPort.last('terminal.replay-complete') !== undefined)
    secondPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'reload-session', data: "printf '__LIVE_AFTER_REPLAY__\\n'\r"
    })
    await waitUntil(() => secondPort.sent.some((message) =>
      message.type === 'terminal.data' &&
      new TextDecoder().decode(message.data).includes('__LIVE_AFTER_REPLAY__')
    ))
    secondPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'reload-session'
    })
    await settle()
  })

  it('serializes duplicate attach requests so one persisted Session owns one live PTY', async () => {
    registerSession(database, 'duplicate-spawn-session')
    const message = {
      type: 'terminal.spawn' as const,
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'duplicate-spawn-session',
      executionContextId: 'replay-context',
      profile: 'shell' as const,
      cols: 80,
      rows: 24
    }

    port.receive(message)
    port.receive(message)
    await waitUntil(() => port.sent.filter(({ type }) => type === 'terminal.spawned').length >= 2)

    const pids = port.sent
      .filter((candidate) => candidate.type === 'terminal.spawned')
      .map((candidate) => candidate.pid)
    expect(new Set(pids).size).toBe(1)
    expect(database.all(
      'SELECT id FROM session_runs WHERE session_id = ?',
      'duplicate-spawn-session'
    )).toHaveLength(1)
    port.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'duplicate-spawn-session'
    })
    await settle()
  })

  it('rejects input for an invalid Workspace while keeping the PTY alive', async () => {
    const sessions = new RuntimeSessionRegistry()
    const guardedPort = new MockPort()
    new RuntimeServer(guardedPort, root, database, undefined, undefined, sessions)
    guardedPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'guarded-renderer'
    })
    registerSession(database, 'guarded-session')
    guardedPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'guarded-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('guarded-session'))
    const pid = sessions.get('guarded-session')!.pid
    database.run(
      `INSERT INTO workspace_path_state (
         workspace_id, status, reason, checked_at, validation_generation
       ) VALUES ('replay-workspace', 'invalid', 'missing', ?, 1)
       ON CONFLICT(workspace_id) DO UPDATE SET
         status = 'invalid', reason = 'missing', checked_at = excluded.checked_at`,
      Date.now()
    )

    guardedPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'guarded-session', data: 'pwd\r'
    })
    await settle()

    expect(guardedPort.last('protocol.error')).toMatchObject({
      code: 'WORKSPACE_PATH_INVALID',
      message: '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
    })
    expect(sessions.get('guarded-session')?.pid).toBe(pid)
    guardedPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'guarded-session'
    })
    await settle()
  })

  it('records each Shell working directory after a completed command', async () => {
    const sessions = new RuntimeSessionRegistry()
    const cwdPort = new MockPort()
    new RuntimeServer(cwdPort, root, database, undefined, undefined, sessions)
    cwdPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'cwd-renderer'
    })
    registerSession(database, 'cwd-session')
    const target = join(root, 'nested-working-directory')
    await mkdir(target)
    cwdPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('cwd-session'))
    await new Promise((resolve) => setTimeout(resolve, 200))

    cwdPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-session', data: `cd ${JSON.stringify(target)}\r`
    })

    await waitUntil(() => database.get<{ cwd: string }>(
      'SELECT cwd FROM sessions WHERE id = ?', 'cwd-session'
    )?.cwd.endsWith('/nested-working-directory') === true, 5_000)
    expect(database.get<{ cwd: string }>('SELECT cwd FROM sessions WHERE id = ?', 'cwd-session')?.cwd)
      .toMatch(/\/nested-working-directory$/)
    cwdPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'cwd-session'
    })
    await settle()
  }, 10_000)

  it('changes Workspace and Task order only after submitted terminal input', async () => {
    const sessions = new RuntimeSessionRegistry()
    const interactionPort = new MockPort()
    new RuntimeServer(interactionPort, root, database, undefined, undefined, sessions)
    interactionPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'recency-renderer'
    })
    registerCanvasSession(database, 'recency-session')
    interactionPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('recency-session'))
    const priorLastOpenedAt = database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at

    interactionPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', data: 'echo recency'
    })
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at).toBe(priorLastOpenedAt)
    interactionPort.receive({
      type: 'terminal.user-interaction', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', interactionKind: 'submit'
    })
    interactionPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', data: '\r'
    })

    await waitUntil(() => (database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at ?? 0) > 1)
    const taskLastOpenedAt = database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM workspaces WHERE id = ?', 'replay-workspace'
    )?.last_opened_at).toBe(taskLastOpenedAt)
  })

  it('publishes the Shell HUD after a chained relative cd command', async () => {
    const sessions = new RuntimeSessionRegistry()
    const cwdPort = new MockPort()
    new RuntimeServer(cwdPort, root, database, undefined, undefined, sessions)
    cwdPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'cwd-hud-renderer'
    })
    registerSession(database, 'cwd-hud-session')
    cwdPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-hud-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('cwd-hud-session'))
    await new Promise((resolve) => setTimeout(resolve, 200))

    cwdPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-hud-session', data: 'mkdir -p outside && cd outside\r'
    })

    await waitUntil(() => cwdPort.last('terminal.hud')?.hud?.cwd?.endsWith('/outside') === true, 5_000)
    expect(cwdPort.last('terminal.hud')).toMatchObject({
      sessionId: 'cwd-hud-session', hud: { mode: 'shell', cwd: join(root, 'outside') }
    })
    cwdPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'cwd-hud-session'
    })
    await settle()
  }, 10_000)

  it('starts a restored Shell in that Session own last working directory', async () => {
    const sessions = new RuntimeSessionRegistry()
    const restorePort = new MockPort()
    new RuntimeServer(restorePort, root, database, undefined, undefined, sessions)
    restorePort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'cwd-restore-renderer'
    })
    registerSession(database, 'cwd-restore-session')
    const target = join(root, 'restored-working-directory')
    await mkdir(target)
    database.run('UPDATE sessions SET cwd = ? WHERE id = ?', target, 'cwd-restore-session')

    restorePort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-restore-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('cwd-restore-session'))

    expect(await childProcessCwd(sessions.get('cwd-restore-session')!.pid))
      .toMatch(/\/restored-working-directory$/)
    restorePort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-restore-session'
    })
    await settle()
  })

  it('launches an AI panel with its validated resume identity and permission mode', async () => {
    const executable = join(root, 'provider-fixture.sh')
    const argumentFile = join(root, 'provider-arguments.txt')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    try {
      registerSession(database, 'provider-resume-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
        ) VALUES (?, ?, 'claude-code', ?, 'available', ?, 1, 1, 1)`,
        'binding-resume', 'provider-resume-session', 'provider-session-42',
        JSON.stringify({ permissionMode: 'default' })
      )
      const sessions = new RuntimeSessionRegistry()
      const providerPort = new MockPort()
      new RuntimeServer(providerPort, root, database, undefined, undefined, sessions)
      providerPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-resume-renderer'
      })
      providerPort.receive({
        type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
        requestId: 'permission-bypass', method: 'session.set-permission-mode',
        capability: 'renderer', deadlineAt: Date.now() + 1000,
        payload: {
          command: {
            commandId: 'permission-bypass', commandType: 'session.set-permission-mode',
            requestHash: 'hash-permission-bypass'
          },
          input: {
            sessionId: 'provider-resume-session', provider: 'claude-code',
            permissionMode: 'bypassPermissions', now: 2
          }
        }
      })
      await waitUntil(() => providerPort.last('rpc.response')?.requestId === 'permission-bypass')
      providerPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-resume-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-session-42', '--dangerously-skip-permissions'
      ])
      providerPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-resume-session'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
      if (previousArgumentFile === undefined) delete process.env.MATOU_TEST_ARGUMENT_FILE
      else process.env.MATOU_TEST_ARGUMENT_FILE = previousArgumentFile
    }
  })

  it('keeps a quiet restored Claude conversation running when statusline confirms its launch', async () => {
    const executable = join(root, 'provider-quiet-resume-fixture.sh')
    const argumentFile = join(root, 'provider-quiet-resume-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    let resumeServer: RuntimeServer | undefined
    const providerHooks = new ProviderHookServer(root, repository, {
      onIdentityRecorded: ({ sessionId, runId }) => {
        resumeServer?.providerIdentityRecorded(sessionId, runId)
        void resumeServer?.refreshSessionHud(sessionId)
      }
    })
    await providerHooks.start()
    try {
      registerSession(database, 'quiet-resume-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-quiet-resume', 'quiet-resume-session', 'provider-quiet-resume'
      )
      const sessions = new RuntimeSessionRegistry()
      const resumePort = new MockPort()
      resumeServer = new RuntimeServer(
        resumePort, root, database, undefined, undefined, sessions,
        providerHooks, undefined, { providerResumeTimeoutMs: 500 }
      )
      resumePort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'quiet-resume-renderer'
      })
      resumePort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'quiet-resume-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      const settingsIndex = arguments_.indexOf('--settings')
      const settings = JSON.parse(await readFile(arguments_[settingsIndex + 1]!, 'utf8')) as {
        hooks: { Stop: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.Stop[0]!.hooks[0]!.url
      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ session_id: 'provider-quiet-resume', cwd: root })
      })).status).toBe(200)

      await new Promise((resolve) => setTimeout(resolve, 600))
      expect(sessions.get('quiet-resume-session')?.profile).toBe('claude-code')
      expect(repository.getResumeBinding('quiet-resume-session', 'claude-code')).toMatchObject({
        providerSessionId: 'provider-quiet-resume', resumeState: 'available'
      })

      resumePort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'quiet-resume-session'
      })
      await settle()
    } finally {
      resumeServer?.close()
      await providerHooks.stop()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('consumes a fork launch exactly once and passes Claude the Kooky fork arguments', async () => {
    const executable = join(root, 'provider-fork-fixture.sh')
    const argumentFile = join(root, 'provider-fork-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    try {
      registerSession(database, 'fork-source', 'claude-code')
      registerSession(database, 'fork-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-derived', 'fork-source', 'provider-source-42'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      const forkServer = new RuntimeServer(
        forkPort, root, database, undefined, undefined, sessions,
        undefined, undefined, { providerResumeTimeoutMs: 1_000 }
      )
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-source-42', '--fork-session'
      ])
      expect(database.get(
        'SELECT state, started_at FROM session_fork_intents WHERE session_id = ?', 'fork-derived'
      )).toMatchObject({ state: 'starting', started_at: expect.any(Number) })

      new SessionRepository(database, new DomainTransactionManager(database))
        .recordResumableProviderIdentity({
          commandId: 'fork-derived-binding', commandType: 'provider-hook',
          requestHash: 'fork-derived-binding'
        }, {
          id: 'fork-derived-binding', sessionId: 'fork-derived', provider: 'claude-code',
          providerSessionId: 'provider-derived-43', metadata: {}, now: 2
        })
      await forkServer.refreshSessionHud('fork-derived')
      await new Promise((resolve) => setTimeout(resolve, 1_100))
      expect(sessions.has('fork-derived')).toBe(true)

      forkPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'fork-derived'
      })
      await settle()
      forkServer.close()

      await writeFile(argumentFile, '')
      const restoredRegistry = new RuntimeSessionRegistry()
      const restoredPort = new MockPort()
      const restoredServer = new RuntimeServer(
        restoredPort, root, database, undefined, undefined, restoredRegistry
      )
      restoredPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-restored-renderer'
      })
      restoredPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-derived-43'
      ])
      restoredPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'fork-derived'
      })
      await settle()
      restoredServer.close()
    } finally {
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('keeps a quiet real-style Fork running while its statusline identity remains provisional', async () => {
    const executable = join(root, 'provider-quiet-fork-fixture.sh')
    const argumentFile = join(root, 'provider-quiet-fork-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    let forkServer: RuntimeServer | undefined
    const providerHooks = new ProviderHookServer(root, repository, {
      onIdentityRecorded: ({ sessionId, runId }) => {
        forkServer?.providerIdentityRecorded(sessionId, runId)
        void forkServer?.refreshSessionHud(sessionId)
      }
    })
    await providerHooks.start()
    try {
      registerSession(database, 'fork-quiet-source', 'claude-code')
      registerSession(database, 'fork-quiet-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-quiet-derived', 'fork-quiet-source', 'provider-source-quiet'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      forkServer = new RuntimeServer(
        forkPort, root, database, undefined, undefined, sessions,
        providerHooks, undefined, { providerResumeTimeoutMs: 500 }
      )
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-quiet-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-quiet-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      const settingsIndex = arguments_.indexOf('--settings')
      expect(settingsIndex).toBeGreaterThanOrEqual(0)
      const settings = JSON.parse(await readFile(arguments_[settingsIndex + 1]!, 'utf8')) as {
        hooks: { Stop: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.Stop[0]!.hooks[0]!.url
      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          session_id: 'provider-derived-quiet', cwd: root,
          model: { display_name: 'Claude Opus 4.6' }
        })
      })).status).toBe(200)

      await new Promise((resolve) => setTimeout(resolve, 600))
      expect(sessions.has('fork-quiet-derived')).toBe(true)
      expect(database.get<{ state: string }>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', 'fork-quiet-derived'
      )).toEqual({ state: 'starting' })
      expect(repository.getResumeBinding('fork-quiet-derived', 'claude-code')).toBeUndefined()

      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          hook_event_name: 'UserPromptSubmit',
          session_id: 'provider-derived-quiet', cwd: root
        })
      })).status).toBe(200)
      expect(database.get<{ state: string }>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', 'fork-quiet-derived'
      )).toEqual({ state: 'succeeded' })

      forkPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'fork-quiet-derived'
      })
      await settle()
    } finally {
      forkServer?.close()
      await providerHooks.stop()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('keeps a failed fork panel inert and never falls back to Shell or repeats the fork', async () => {
    const executable = join(root, 'provider-fork-failure.sh')
    await writeFile(executable, '#!/bin/sh\nprintf "No session found for requested id\\n"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    try {
      registerSession(database, 'fork-failure-source', 'claude-code')
      registerSession(database, 'fork-failure-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-failure-derived', 'fork-failure-source', 'missing-provider-42'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      new RuntimeServer(forkPort, root, database, undefined, undefined, sessions)
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-failure-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-failure-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => database.get<{ state: string }>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', 'fork-failure-derived'
      )?.state === 'failed')
      await waitUntil(() => terminalText(forkPort).includes('[Fork 未完成，请检查上方原因后重试]'))
      expect(database.get(
        'SELECT kind FROM sessions WHERE id = ?', 'fork-failure-derived'
      )).toEqual({ kind: 'claude-code' })
      expect(sessions.has('fork-failure-derived')).toBe(false)
      expect(forkPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)

      await settle()
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-failure-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => forkPort.last('terminal.spawned')?.pid === 0)
      const durableText = new TextDecoder().decode(Uint8Array.from(
        (await readSessionFrames(root, 'fork-failure-derived'))
          .filter((frame) => frame.kind === 'output')
          .flatMap((frame) => [...frame.data])
      ))
      expect(durableText.match(/\[Fork 未完成，请检查上方原因后重试\]/g)).toHaveLength(1)
    } finally {
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('shows the Kooky fork failure banner when the fork process exits before producing output', async () => {
    const executable = join(root, 'provider-fork-exit.sh')
    await writeFile(executable, '#!/bin/sh\nexit 7\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    try {
      registerSession(database, 'fork-exit-source', 'claude-code')
      registerSession(database, 'fork-exit-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-exit-derived', 'fork-exit-source', 'provider-source-exit'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      new RuntimeServer(forkPort, root, database, undefined, undefined, sessions)
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-exit-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-exit-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => database.get<{ state: string }>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', 'fork-exit-derived'
      )?.state === 'failed')
      await waitUntil(() => terminalText(forkPort).includes('[Fork 未完成，请检查上方原因后重试]'))
      expect(database.get('SELECT kind FROM sessions WHERE id = ?', 'fork-exit-derived'))
        .toEqual({ kind: 'claude-code' })
      expect(forkPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)
    } finally {
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('switches live permission modes in place and respawns across the Bypass boundary', async () => {
    const executable = join(root, 'provider-live-permission.sh')
    const argumentFile = join(root, 'provider-live-permission-arguments.txt')
    const inputFile = join(root, 'provider-live-permission-input.txt')
    await writeFile(executable, [
      '#!/bin/sh',
      'printf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"',
      'head -c 2100 /dev/zero | tr "\\0" x',
      'stty raw -echo',
      'cat >> "$MATOU_TEST_INPUT_FILE"'
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    const previousInputFile = process.env.MATOU_TEST_INPUT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    process.env.MATOU_TEST_INPUT_FILE = inputFile
    const sessions = new RuntimeSessionRegistry()
    const livePort = new MockPort()
    try {
      registerSession(database, 'provider-live-permission', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', ?, 1, 1, 1)`,
        'binding-live-permission', 'provider-live-permission', 'provider-live-42',
        JSON.stringify({ permissionMode: 'default' })
      )
      new RuntimeServer(livePort, root, database, undefined, undefined, sessions)
      livePort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'live-permission-renderer'
      })
      livePort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-live-permission', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.has('provider-live-permission'))
      const firstPid = sessions.get('provider-live-permission')!.pid

      livePort.receive(rpc('permission-plan', 'session.set-permission-mode', {
        sessionId: 'provider-live-permission', provider: 'claude-code',
        permissionMode: 'plan', respawn: false, now: 2
      }))
      await waitUntil(() => livePort.findRpcResponse('permission-plan') !== undefined)
      await waitUntilAsync(async () => (await readFile(inputFile, 'utf8').catch(() => '')).includes('\u001b[Z\u001b[Z'))
      expect(sessions.get('provider-live-permission')?.pid).toBe(firstPid)

      livePort.receive(rpc('model-sonnet', 'session.set-model', {
        sessionId: 'provider-live-permission', modelStrategy: 'claude-sonnet-4-6'
      }))
      await waitUntil(() => livePort.findRpcResponse('model-sonnet') !== undefined)
      await waitUntilAsync(async () => (await readFile(inputFile, 'utf8').catch(() => ''))
        .includes('/model claude-sonnet-4-6\r'))
      expect(sessions.get('provider-live-permission')?.pid).toBe(firstPid)
      expect(livePort.last('terminal.hud')).toMatchObject({
        hud: { modelStrategy: 'claude-sonnet-4-6', mode: 'agent' }
      })

      livePort.receive(rpc('permission-bypass-live', 'session.set-permission-mode', {
        sessionId: 'provider-live-permission', provider: 'claude-code',
        permissionMode: 'bypassPermissions', respawn: true, now: 3
      }))
      await waitUntil(() => sessions.get('provider-live-permission')?.pid !== firstPid)
      await waitUntil(() => livePort.findRpcResponse('permission-bypass-live') !== undefined)
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')).includes('--dangerously-skip-permissions'))

      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-live-42', '--dangerously-skip-permissions'
      ])
      expect(terminalText(livePort)).toContain('\u001b[2J\u001b[3J\u001b[H')
      expect(livePort.last('terminal.hud')).toMatchObject({
        hud: { permissionMode: 'bypassPermissions', mode: 'agent' }
      })
      expect(livePort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)
    } finally {
      livePort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-live-permission'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
      restoreEnv('MATOU_TEST_INPUT_FILE', previousInputFile)
    }
  })

  it('starts a fresh live AI process when Bypass is confirmed before a resumable identity exists', async () => {
    const executable = join(root, 'provider-fresh-permission.sh')
    const argumentFile = join(root, 'provider-fresh-permission-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const sessions = new RuntimeSessionRegistry()
    const freshPort = new MockPort()
    try {
      registerSession(database, 'provider-fresh-permission', 'claude-code')
      new RuntimeServer(freshPort, root, database, undefined, undefined, sessions)
      freshPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fresh-permission-renderer'
      })
      freshPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fresh-permission', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.has('provider-fresh-permission'))
      const firstPid = sessions.get('provider-fresh-permission')!.pid

      freshPort.receive(rpc('fresh-permission-bypass', 'session.set-permission-mode', {
        sessionId: 'provider-fresh-permission', provider: 'claude-code',
        permissionMode: 'bypassPermissions', respawn: true, now: 2
      }))

      await waitUntil(() => freshPort.findRpcResponse('fresh-permission-bypass') !== undefined)
      await waitUntil(() => sessions.get('provider-fresh-permission')?.pid !== firstPid)
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')).length > 0)
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--dangerously-skip-permissions'
      ])
      expect(freshPort.findRpcError('fresh-permission-bypass')).toBeUndefined()
    } finally {
      freshPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fresh-permission'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('reports a failed Bypass respawn instead of presenting the Shell fallback as success', async () => {
    const executable = join(root, 'provider-failed-permission.sh')
    await writeFile(executable, '#!/bin/sh\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const failedPort = new MockPort()
    try {
      registerSession(database, 'provider-failed-permission', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', ?, 1, 1, 1)`,
        'binding-failed-permission', 'provider-failed-permission', 'provider-failed-42',
        JSON.stringify({ permissionMode: 'default' })
      )
      new RuntimeServer(failedPort, root, database, undefined, undefined, sessions)
      failedPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'failed-permission-renderer'
      })
      failedPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-failed-permission', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.get('provider-failed-permission')?.profile === 'claude-code')
      await rm(executable)

      failedPort.receive(rpc('failed-permission-bypass', 'session.set-permission-mode', {
        sessionId: 'provider-failed-permission', provider: 'claude-code',
        permissionMode: 'bypassPermissions', respawn: true, now: 2
      }))

      await waitUntil(() => failedPort.findRpcError('failed-permission-bypass') !== undefined)
      expect(failedPort.findRpcResponse('failed-permission-bypass')).toBeUndefined()
      expect(failedPort.last('terminal.hud')).toMatchObject({ hud: { mode: 'shell' } })
    } finally {
      failedPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-failed-permission'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('returns a naturally completed Agent panel to a usable Shell HUD without an exit flash', async () => {
    const executable = join(root, 'provider-completes.sh')
    await writeFile(executable, '#!/bin/sh\nsleep 0.1\nexit 0\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const completedPort = new MockPort()
    try {
      registerSession(database, 'provider-completed', 'claude-code')
      new RuntimeServer(completedPort, root, database, undefined, undefined, sessions)
      completedPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'completed-renderer'
      })
      completedPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-completed', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('provider-completed')?.profile === 'shell')
      expect(database.get<{ kind: string }>('SELECT kind FROM sessions WHERE id = ?', 'provider-completed'))
        .toEqual({ kind: 'shell' })
      expect(completedPort.last('terminal.hud')).toMatchObject({ hud: { mode: 'shell' } })
      expect(completedPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)
    } finally {
      completedPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'provider-completed'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('promotes a Shell panel to bypass Claude when a real-world zsh config loads slowly', async () => {
    const executable = join(root, 'claude')
    const argumentFile = join(root, 'shell-promoted-provider-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    const previousPath = process.env.PATH
    const previousShell = process.env.SHELL
    const previousZdotdir = process.env.ZDOTDIR
    await writeFile(join(root, '.zshrc'), "sleep 2.2\nalias cc='claude --dangerously-skip-permissions'\n")
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    process.env.PATH = `${root}:${previousPath ?? ''}`
    process.env.SHELL = '/bin/zsh'
    process.env.ZDOTDIR = root
    const sessions = new RuntimeSessionRegistry()
    const promotedPort = new MockPort()
    try {
      registerSession(database, 'shell-promoted-provider', 'shell')
      new RuntimeServer(promotedPort, root, database, undefined, undefined, sessions)
      promotedPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'shell-promoted-renderer'
      })
      promotedPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'shell-promoted-provider', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.get('shell-promoted-provider')?.profile === 'shell')

      promotedPort.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'shell-promoted-provider', data: 'cc\r'
      })

      await waitUntil(() => sessions.get('shell-promoted-provider')?.profile === 'claude-code', 6_000)
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')).length > 0, 6_000)
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--dangerously-skip-permissions'
      ])
      expect(database.get<{ kind: string; title: string }>(
        'SELECT kind, title FROM sessions WHERE id = ?', 'shell-promoted-provider'
      )).toEqual({ kind: 'claude-code', title: 'Claude' })
      expect(promotedPort.last('terminal.hud')).toMatchObject({
        sessionId: 'shell-promoted-provider', hud: {
          mode: 'agent', permissionMode: 'bypassPermissions', modelStrategy: 'opusplan'
        }
      })
      await waitUntil(() => terminalText(promotedPort).includes('\u001b[2J\u001b[3J\u001b[H'))
      expect(terminalText(promotedPort)).toContain('\u001b[2J\u001b[3J\u001b[H')
    } finally {
      promotedPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'shell-promoted-provider'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
      restoreEnv('PATH', previousPath)
      restoreEnv('SHELL', previousShell)
      restoreEnv('ZDOTDIR', previousZdotdir)
    }
  })

  it('replaces the live fallback Shell when the same persisted Session retries Claude restore', async () => {
    const executable = join(root, 'provider-restore-retry.sh')
    await writeFile(executable, '#!/bin/sh\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const retryPort = new MockPort()
    try {
      registerSession(database, 'provider-restore-retry', 'shell')
      new RuntimeServer(retryPort, root, database, undefined, undefined, sessions)
      retryPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'restore-retry-renderer'
      })
      retryPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-restore-retry', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.get('provider-restore-retry')?.profile === 'shell')
      database.run(
        `UPDATE sessions SET kind = 'claude-code', title = 'Claude', status = 'starting',
           work_status = 'starting' WHERE id = ?`,
        'provider-restore-retry'
      )
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, restore_state,
           metadata_json, created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', 'restoring', '{}', 1, 2, 1)`,
        'binding-restore-retry', 'provider-restore-retry', 'provider-retry-identity'
      )

      retryPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-restore-retry', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('provider-restore-retry')?.profile === 'claude-code')
      expect(retryPort.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(0)
      expect(retryPort.last('terminal.hud')).toMatchObject({
        sessionId: 'provider-restore-retry', hud: { mode: 'agent' }
      })
    } finally {
      retryPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-restore-retry'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('captures a newly started Claude conversation for the next automatic restore', async () => {
    const executable = join(root, 'new-provider-fixture.sh')
    const argumentFile = join(root, 'new-provider-arguments.txt')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    const providerHooks = new ProviderHookServer(root, repository)
    await providerHooks.start()
    try {
      registerSession(database, 'provider-new-session', 'claude-code')
      const registry = new RuntimeSessionRegistry()
      const providerPort = new MockPort()
      new RuntimeServer(
        providerPort, root, database, undefined, undefined, registry, providerHooks
      )
      providerPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-new-renderer'
      })
      providerPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-new-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      expect(arguments_.slice(0, 2)).toEqual(['--settings', expect.any(String)])
      const settings = JSON.parse(await readFile(arguments_[1]!, 'utf8')) as {
        hooks: { Stop: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.Stop[0]!.hooks[0]!.url
      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          hook_event_name: 'Stop', session_id: 'claude-new-session-42', cwd: root
        })
      })).status).toBe(200)
      expect(repository.getResumeBinding('provider-new-session', 'claude-code')).toMatchObject({
        providerSessionId: 'claude-new-session-42', metadata: {
          cwd: root, lastHookEvent: 'Stop'
        }
      })

      providerPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-new-session'
      })
      await settle()
    } finally {
      await providerHooks.stop()
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
      if (previousArgumentFile === undefined) delete process.env.MATOU_TEST_ARGUMENT_FILE
      else process.env.MATOU_TEST_ARGUMENT_FILE = previousArgumentFile
    }
  })

  it('lets Claude finish its SessionEnd hook after the provider PTY exits', async () => {
    const executable = join(root, 'provider-session-end-fixture.sh')
    const argumentFile = join(root, 'provider-session-end-arguments.txt')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 0.2\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    const providerHooks = new ProviderHookServer(root, repository)
    await providerHooks.start()
    const registry = new RuntimeSessionRegistry()
    const providerPort = new MockPort()
    const server = new RuntimeServer(
      providerPort, root, database, undefined, undefined, registry, providerHooks
    )
    try {
      registerSession(database, 'provider-session-end', 'claude-code')
      providerPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-session-end-renderer'
      })
      providerPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-session-end', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      const settingsIndex = arguments_.indexOf('--settings')
      const settings = JSON.parse(await readFile(arguments_[settingsIndex + 1]!, 'utf8')) as {
        hooks: { SessionEnd: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.SessionEnd[0]!.hooks[0]!.url
      await waitUntil(() => registry.get('provider-session-end')?.profile === 'shell')

      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          hook_event_name: 'SessionEnd', session_id: 'provider-ending-after-pty'
        })
      })).status).toBe(200)
      expect(repository.getResumeBinding('provider-session-end', 'claude-code')).toBeUndefined()
    } finally {
      providerPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-session-end'
      })
      await settle()
      server.close()
      await providerHooks.stop()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('clears a missing AI resume identity and keeps the same panel usable as Shell', async () => {
    const executable = join(root, 'missing-provider-session.sh')
    await writeFile(executable, '#!/bin/sh\nprintf "No session found for requested id\\n"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    try {
      registerSession(database, 'provider-fallback-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-fallback', 'provider-fallback-session', 'missing-provider-42'
      )
      const sessions = new RuntimeSessionRegistry()
      const fallbackPort = new MockPort()
      new RuntimeServer(fallbackPort, root, database, undefined, undefined, sessions)
      fallbackPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-fallback-renderer'
      })
      fallbackPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fallback-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('provider-fallback-session')?.profile === 'shell')

      expect(database.get<{ kind: string }>(
        'SELECT kind FROM sessions WHERE id = ?', 'provider-fallback-session'
      )).toEqual({ kind: 'shell' })
      expect(database.get<{ resume_state: string; invalidated_at: number | null }>(
        'SELECT resume_state, invalidated_at FROM provider_bindings WHERE id = ?',
        'binding-fallback'
      )).toMatchObject({ resume_state: 'failed', invalidated_at: expect.any(Number) })
      await waitUntil(() => terminalText(fallbackPort).includes('[上次会话无法续接，已回到普通终端]'))
      expect(terminalText(fallbackPort)).toContain('[上次会话无法续接，已回到普通终端]')
      expect(fallbackPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)

      fallbackPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fallback-session'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
    }
  })

  it('degrades an unresponsive AI resume to a usable Shell at the product deadline', async () => {
    const executable = join(root, 'unresponsive-provider-session.sh')
    await writeFile(executable, '#!/bin/sh\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const timeoutPort = new MockPort()
    try {
      registerSession(database, 'provider-timeout-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-timeout', 'provider-timeout-session', 'unresponsive-provider-42'
      )
      new RuntimeServer(
        timeoutPort, root, database, undefined, undefined, sessions, undefined, undefined,
        { providerResumeTimeoutMs: 25 }
      )
      timeoutPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-timeout-renderer'
      })
      timeoutPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-timeout-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('provider-timeout-session')?.profile === 'shell')
      await waitUntil(() => terminalText(timeoutPort).includes(
        '[上次会话无法续接，已回到普通终端]'
      ))
      expect(database.get<{ kind: string }>(
        'SELECT kind FROM sessions WHERE id = ?', 'provider-timeout-session'
      )).toEqual({ kind: 'shell' })
      expect(database.get<{ resume_state: string }>(
        'SELECT resume_state FROM provider_bindings WHERE id = ?', 'binding-timeout'
      )).toEqual({ resume_state: 'failed' })
    } finally {
      timeoutPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-timeout-session'
      })
      await settle()
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
    }
  })

  it('degrades to Shell when the AI resume process cannot start', async () => {
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = join(root, 'provider-command-does-not-exist')
    try {
      registerSession(database, 'provider-launch-fallback', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-launch-fallback', 'provider-launch-fallback', 'provider-session-9'
      )
      const sessions = new RuntimeSessionRegistry()
      const launchPort = new MockPort()
      new RuntimeServer(launchPort, root, database, undefined, undefined, sessions)
      launchPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-launch-fallback-renderer'
      })
      launchPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-launch-fallback', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('provider-launch-fallback')?.profile === 'shell')
      await waitUntil(() => terminalText(launchPort).includes('[上次会话无法续接，已回到普通终端]'))

      expect(database.get<{ kind: string }>(
        'SELECT kind FROM sessions WHERE id = ?', 'provider-launch-fallback'
      )).toEqual({ kind: 'shell' })
      expect(launchPort.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(0)
      launchPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-launch-fallback'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
    }
  })

  it('does not retry or repeat the fallback hint after a failed identity was cleared', async () => {
    const executable = join(root, 'provider-must-not-run.sh')
    const launchMarker = join(root, 'provider-launch-marker')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf launched > "$MATOU_TEST_LAUNCH_MARKER"\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousMarker = process.env.MATOU_TEST_LAUNCH_MARKER
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_LAUNCH_MARKER = launchMarker
    try {
      registerSession(database, 'cleared-provider-session', 'shell')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at, invalidated_at
         ) VALUES (?, ?, 'claude-code', ?, 'failed', ?, 1, 2, 1, 2)`,
        'binding-already-cleared', 'cleared-provider-session', 'provider-old',
        JSON.stringify({ invalidationReason: 'provider session not found' })
      )
      const sessions = new RuntimeSessionRegistry()
      const secondStartPort = new MockPort()
      new RuntimeServer(secondStartPort, root, database, undefined, undefined, sessions)
      secondStartPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'second-start-renderer'
      })
      secondStartPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'cleared-provider-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('cleared-provider-session')?.profile === 'shell')
      await new Promise((resolve) => setTimeout(resolve, 50))
      expect(await readFile(launchMarker, 'utf8').catch(() => '')).toBe('')
      expect(terminalText(secondStartPort)).not.toContain('[上次会话无法续接，已回到普通终端]')
      expect(database.get<{ resume_state: string }>(
        'SELECT resume_state FROM provider_bindings WHERE id = ?', 'binding-already-cleared'
      )).toEqual({ resume_state: 'failed' })
      secondStartPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'cleared-provider-session'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
      if (previousMarker === undefined) delete process.env.MATOU_TEST_LAUNCH_MARKER
      else process.env.MATOU_TEST_LAUNCH_MARKER = previousMarker
    }
  })
})

class MockPort extends EventEmitter implements RuntimePort {
  readonly sent: RuntimeMessage[] = []

  postMessage(message: RuntimeMessage): void {
    this.sent.push(message)
  }
  start(): void {}
  close(): void {}
  receive(data: unknown): void {
    this.emit('message', { data } satisfies PortMessageEvent)
  }
  disconnect(): void { this.emit('close') }
  last<T extends RuntimeMessage['type']>(type: T): Extract<RuntimeMessage, { type: T }> | undefined {
    return this.sent.filter((message) => message.type === type).at(-1) as Extract<RuntimeMessage, { type: T }> | undefined
  }
  findRpcError(requestId: string): Extract<RuntimeMessage, { type: 'rpc.error' }> | undefined {
    return this.sent.find(
      (message): message is Extract<RuntimeMessage, { type: 'rpc.error' }> =>
        message.type === 'rpc.error' && message.requestId === requestId
    )
  }
  findRpcResponse(requestId: string): Extract<RuntimeMessage, { type: 'rpc.response' }> | undefined {
    return this.sent.find(
      (message): message is Extract<RuntimeMessage, { type: 'rpc.response' }> =>
        message.type === 'rpc.response' && message.requestId === requestId
    )
  }
}

async function settle(): Promise<void> {
  for (let index = 0; index < 200; index += 1) {
    await new Promise((resolve) => setImmediate(resolve))
  }
}

async function waitUntil(predicate: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error('condition did not become true before timeout')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

async function waitUntilAsync(predicate: () => Promise<boolean>, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await predicate())) {
    if (Date.now() >= deadline) throw new Error('async condition did not become true before timeout')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

function terminalText(port: MockPort): string {
  const decoder = new TextDecoder()
  return port.sent
    .filter((message): message is Extract<RuntimeMessage, { type: 'terminal.data' }> =>
      message.type === 'terminal.data'
    )
    .map(({ data }) => decoder.decode(data))
    .join('')
}

function workStatus(sessionId: string): string | undefined {
  return database.get<{ work_status: string }>(
    'SELECT work_status FROM sessions WHERE id = ?', sessionId
  )?.work_status
}

function seedReplayAuthority(database: RuntimeDatabase, root: string): void {
  database.transaction((tx) => {
    tx.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'replay-workspace', 'Replay', root, 1, 1
    )
    tx.run(
      'INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)',
      'replay-context', 'replay-workspace', 'plain-directory', root, 1
    )
    tx.run(
      'INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
      'replay-task', 'replay-workspace', 'replay-context', 'Replay', 'active', 1, 1
    )
  })
}

function registerSession(
  database: RuntimeDatabase,
  sessionId: string,
  kind: 'shell' | 'claude-code' | 'codex' = 'shell'
): void {
  database.run(
    `INSERT INTO sessions (
       id, task_id, execution_context_id, kind, status, title,
       created_at, updated_at, last_activity_at
     ) VALUES (?, 'replay-task', 'replay-context', ?, 'exited', ?, 1, 1, 1)`,
    sessionId, kind, sessionId
  )
}

function registerCanvasSession(database: RuntimeDatabase, sessionId: string): void {
  registerSession(database, sessionId)
  database.run(
    `INSERT INTO scenes (
       id, task_id, name, mode, root_node_id, title_pinned, sort_key,
       layout_revision, created_at, updated_at
     ) VALUES (?, 'replay-task', 'Canvas', 'tile', ?, 0, 'a0', 1, 1, 1)`,
    `scene-${sessionId}`, `node-${sessionId}`
  )
  database.run(
    `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
     VALUES (?, ?, 'root', 0, 1)`,
    `node-${sessionId}`, `scene-${sessionId}`
  )
  database.run(
    `INSERT INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at)
     VALUES (?, ?, ?, ?, 1)`,
    `mount-${sessionId}`, `scene-${sessionId}`, `node-${sessionId}`, sessionId
  )
}

function rpc(requestId: string, method: RpcMethod, input: Record<string, unknown>) {
  return {
    type: 'rpc.request' as const, protocolVersion: PROTOCOL_VERSION, requestId,
    method, capability: 'renderer' as const,
    deadlineAt: Date.now() + 2_000,
    payload: {
      command: { commandId: requestId, commandType: method, requestHash: `hash-${requestId}` },
      input
    }
  }
}

function restoreEnv(name: string, value: string | undefined): void {
  if (value === undefined) delete process.env[name]
  else process.env[name] = value
}

async function childProcessCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') return readlink(`/proc/${pid}/cwd`)
  const { stdout } = await execFileAsync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}
