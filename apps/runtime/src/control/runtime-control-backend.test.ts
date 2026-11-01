import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { RuntimeControlBackend } from './runtime-control-backend'
import {
  hasHostControlPostResponseEffects,
  isHostControlCommittedResult,
  markHostControlCommittedResult,
  runHostControlPostResponseEffects,
  withHostControlPostResponseEffect
} from './host-control-post-response'
import { SegmentJournal } from '../journal/segment-journal'
import type { PtySession } from '../session/pty-session'
import { TaskTelemetryRepository } from '../domain/product-foundation-repository'
import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { NotificationProjection } from '../product/experience-foundation'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let root: string
let database: RuntimeDatabase

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-control-backend-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
})

afterEach(() => database.close())

describe('RuntimeControlBackend Task information channel', () => {
  it('fails closed until the Runtime installs the complete Host Action facade', async () => {
    const backend = new RuntimeControlBackend(
      database, root, new TaskTelemetryRepository(database, database.runtimeGeneration)
    )

    await expect(backend.executeHostAction(
      'structure.create.workspace',
      { runId: 'run-uninstalled', sessionId: 'session-uninstalled' },
      { path: root, submissionKey: 'uninstalled' }
    )).rejects.toMatchObject({ code: 'RUNTIME_NOT_READY' })
  })

  it('returns the facade result object unchanged with commit and post-response metadata intact', async () => {
    const backend = new RuntimeControlBackend(
      database, root, new TaskTelemetryRepository(database, database.runtimeGeneration)
    )
    const disposed = vi.fn(async () => undefined)
    const facadeResult = markHostControlCommittedResult(withHostControlPostResponseEffect({
      kind: 'removed' as const,
      targetRef: 'session:session-1',
      removedTasks: 0,
      removedCanvases: 0,
      removedSessions: 1,
      activePath: {
        window: { ref: 'window:window-1', title: 'Window' },
        workspace: { ref: 'workspace:workspace-1', title: 'Workspace', path: root }
      }
    }, disposed))
    backend.setHostActionExecutor(async () => facadeResult)

    const result = await backend.executeHostAction(
      'structure.remove.commit',
      { runId: 'run-1', sessionId: 'session-1' },
      { confirmationRef: 'confirmation-1' }
    )

    expect(result).toBe(facadeResult)
    expect(isHostControlCommittedResult(result)).toBe(true)
    expect(hasHostControlPostResponseEffects(result)).toBe(true)
    await runHostControlPostResponseEffects(result)
    expect(disposed).toHaveBeenCalledTimes(1)
  })

  it('separates the latest terminal screen from journal history', async () => {
    const hierarchy = new HierarchyApplicationService(database, new DomainTransactionManager(database))
    const initial = hierarchy.bootstrapWindow(command('bootstrap-screen'), {
      windowId: 'window-1', defaultRootDirectory: root, defaultName: 'Workspace', now: 1
    })
    const journal = await SegmentJournal.open(root, initial.session!.id)
    await journal.appendResize(1, 12, 2)
    await journal.appendOutput(2, new TextEncoder().encode('old line\r\nsecond\r\nlatest'))
    await journal.close()
    const backend = new RuntimeControlBackend(
      database, root, new TaskTelemetryRepository(database, database.runtimeGeneration)
    )

    await expect(backend.readCurrent(initial.session!.id, { maxLines: 10, maxBytes: 4096 }))
      .resolves.toMatchObject({
        text: 'second\nlatest', cols: 12, rows: 2, source: 'screen', truncated: false
      })
    await expect(backend.readHistory(initial.session!.id, { maxLines: 10, maxBytes: 4096 }))
      .resolves.toMatchObject({
        text: 'old line\r\nsecond\r\nlatest', firstSequence: 1, lastSequence: 2,
        source: 'journal', truncated: false
      })
  })

  it('turns an external error log into Task unread feedback without changing focus', async () => {
    const hierarchy = new HierarchyApplicationService(database, new DomainTransactionManager(database))
    const initial = hierarchy.bootstrapWindow(command('bootstrap'), {
      windowId: 'window-1', defaultRootDirectory: root, defaultName: 'Workspace', now: 1
    })
    const notifications = new NotificationProjection({ cooldownMs: 0 })
    const backend = new RuntimeControlBackend(
      database, root,
      new TaskTelemetryRepository(database, database.runtimeGeneration),
      notifications
    )

    await backend.appendTaskLog(initial.task!.id, 'error', 'claude-code', 'Build failed')

    expect(notifications.list()).toEqual([
      expect.objectContaining({
        type: 'error', taskId: initial.task!.id, sessionId: initial.session!.id,
        mountId: initial.mount!.id, subtitle: 'claude-code', body: 'Build failed', read: false
      })
    ])
  })

  it('rejects writes to a deleted Task without affecting live Task information', async () => {
    const hierarchy = new HierarchyApplicationService(database, new DomainTransactionManager(database))
    const initial = hierarchy.bootstrapWindow(command('bootstrap-delete'), {
      windowId: 'window-1', defaultRootDirectory: root, defaultName: 'Workspace', now: 1
    })
    const telemetry = new TaskTelemetryRepository(database, database.runtimeGeneration)
    const backend = new RuntimeControlBackend(database, root, telemetry, new NotificationProjection())
    await backend.writeTaskStatus(initial.task!.id, 'phase', 'running')
    await backend.writeTaskProgress(initial.task!.id, 50, 'half')
    await backend.appendTaskLog(initial.task!.id, 'info', 'agent', 'working')
    hierarchy.deleteTask(command('delete'), {
      windowId: 'window-1', taskId: initial.task!.id,
      confirmedIntent: `delete-task:${initial.task!.id}`, now: 2
    })

    await expect(backend.writeTaskStatus(initial.task!.id, 'phase', 'late')).rejects.toThrow('does not exist')
    expect(telemetry.snapshot(initial.task!.id)).toEqual({ status: {}, progress: undefined, logs: [] })
  })
})

function command(commandId: string) {
  return { commandId, commandType: commandId, requestHash: `hash-${commandId}` }
}

describe('RuntimeControlBackend submitted terminal input', () => {
  afterEach(() => vi.useRealTimers())

  function fakeSession(): { session: PtySession; writes: Array<{ data: string; at: number }>; echo: () => void } {
    const state = { lastOutputAt: 0 }
    const writes: Array<{ data: string; at: number }> = []
    const session = {
      get lastOutputAt() { return state.lastOutputAt },
      write: (data: string) => { writes.push({ data, at: Date.now() }) }
    } as unknown as PtySession
    return { session, writes, echo: () => { state.lastOutputAt = Date.now() } }
  }

  function backendWith(session: PtySession): RuntimeControlBackend {
    const backend = new RuntimeControlBackend(
      database, root, new TaskTelemetryRepository(database, database.runtimeGeneration)
    )
    backend.register('session-1', session)
    return backend
  }

  it('sends Enter separately after the target stops echoing so agent TUIs submit instead of pasting', async () => {
    vi.useFakeTimers({ now: 1_000 })
    const target = fakeSession()
    const backend = backendWith(target.session)

    const sent = backend.sendText('session-1', 'finish and report back', true)
    await vi.advanceTimersByTimeAsync(0)
    expect(target.writes.map(({ data }) => data)).toEqual(['finish and report back'])

    target.echo()
    await vi.advanceTimersByTimeAsync(200)
    target.echo()
    await vi.advanceTimersByTimeAsync(200)
    expect(target.writes).toHaveLength(1)

    await vi.advanceTimersByTimeAsync(300)
    await sent
    expect(target.writes.map(({ data }) => data)).toEqual(['finish and report back', '\r'])
    expect(target.writes[1]!.at - target.writes[0]!.at).toBeGreaterThanOrEqual(200 + 250)
  })

  it('submits after the bounded wait even when a busy target never goes quiet', async () => {
    vi.useFakeTimers({ now: 1_000 })
    const target = fakeSession()
    const backend = backendWith(target.session)
    const spinner = setInterval(target.echo, 20)

    const sent = backend.sendText('session-1', 'queued follow-up', true)
    await vi.advanceTimersByTimeAsync(5_100)
    await sent
    clearInterval(spinner)

    expect(target.writes.map(({ data }) => data)).toEqual(['queued follow-up', '\r'])
    expect(target.writes[1]!.at - target.writes[0]!.at).toBeLessThanOrEqual(5_100)
  })

  it('keeps fill-only input as a single write without Enter', async () => {
    const target = fakeSession()
    const backend = backendWith(target.session)

    await backend.sendText('session-1', 'draft only')

    expect(target.writes.map(({ data }) => data)).toEqual(['draft only'])
  })
})
