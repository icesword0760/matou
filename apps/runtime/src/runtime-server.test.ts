import { EventEmitter } from 'node:events'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, type RuntimeMessage } from '@matou/contracts'

import { SegmentJournal } from './journal/segment-journal'
import { CheckpointManager } from './checkpoints/checkpoint-manager'
import { RuntimeServer, type PortMessageEvent, type RuntimePort } from './runtime-server'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { RuntimeDatabase } from './storage/database'
import { MigrationRunner } from './storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from './storage/migrations'

let root: string
let database: RuntimeDatabase
let port: MockPort

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-server-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seedReplayAuthority(database, root)
  port = new MockPort()
  new RuntimeServer(port, root, database)
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

    expect(secondPort.last('terminal.spawned')).toMatchObject({ pid: firstPid, reattached: true })
    secondPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'reload-session'
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

function registerSession(database: RuntimeDatabase, sessionId: string): void {
  database.run(
    `INSERT INTO sessions (
       id, task_id, execution_context_id, kind, status, title,
       created_at, updated_at, last_activity_at
     ) VALUES (?, 'replay-task', 'replay-context', 'shell', 'exited', ?, 1, 1, 1)`,
    sessionId, sessionId
  )
}
