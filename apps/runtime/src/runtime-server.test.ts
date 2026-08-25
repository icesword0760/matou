import { EventEmitter } from 'node:events'
import { execFile } from 'node:child_process'
import { chmod, mkdir, mkdtemp, readFile, readlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, type RuntimeMessage } from '@matou/contracts'

import { SegmentJournal } from './journal/segment-journal'
import { CheckpointManager } from './checkpoints/checkpoint-manager'
import { RuntimeServer, type PortMessageEvent, type RuntimePort } from './runtime-server'
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

afterEach(() => database.close())

describe('RuntimeServer domain RPC', () => {
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

async function childProcessCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') return readlink(`/proc/${pid}/cwd`)
  const { stdout } = await execFileAsync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}
