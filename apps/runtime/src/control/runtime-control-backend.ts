import {
  type HostCallerIdentity,
  type HostControlBackend,
  type HostTarget,
  type HostTargetSelector
} from './host-control-server'
import { HostTopologyProjector } from './host-topology-projector'
import { CommandBoundaryRepository } from '../anchors/anchor-resolver'
import { TaskTelemetryRepository } from '../domain/product-foundation-repository'
import { readSessionFrames, type DecodedJournalFrame } from '../journal/segment-journal'
import type { PtySession } from '../session/pty-session'
import type { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { TaskWindowMigrationService } from '../hierarchy/task-window-migration-service'
import { NotificationProjection } from '../product/experience-foundation'
import { TerminalScreenProjector } from './terminal-screen-projector'
import { CONTROL_KEY_SEQUENCES, TerminalInputQueue } from './terminal-input-queue'
import { HostControlTargetNotReadyError } from './host-control-types'

export class RuntimeControlBackend implements HostControlBackend {
  readonly #database: RuntimeDatabase
  readonly #dataRoot: string
  readonly #telemetry: TaskTelemetryRepository
  readonly #commands: CommandBoundaryRepository
  readonly #notifications: NotificationProjection | undefined
  readonly #active = new Map<string, PtySession>()
  readonly #taskMigrations: TaskWindowMigrationService
  readonly #topology: HostTopologyProjector
  readonly #inputQueue = new TerminalInputQueue()

  constructor(
    database: RuntimeDatabase,
    dataRoot: string,
    telemetry: TaskTelemetryRepository,
    notifications?: NotificationProjection
  ) {
    this.#database = database
    this.#dataRoot = dataRoot
    this.#telemetry = telemetry
    this.#notifications = notifications
    this.#commands = new CommandBoundaryRepository(database)
    this.#topology = new HostTopologyProjector(database)
    this.#taskMigrations = new TaskWindowMigrationService(
      database, new DomainTransactionManager(database)
    )
  }

  register(sessionId: string, session: PtySession): void {
    this.#active.set(sessionId, session)
  }

  unregister(sessionId: string, session: PtySession): void {
    if (this.#active.get(sessionId) === session) {
      this.#active.delete(sessionId)
      this.#inputQueue.clear(sessionId)
    }
  }

  identify(caller: HostCallerIdentity): unknown {
    return this.#topology.identify(caller)
  }

  resolveTarget(
    caller: HostCallerIdentity,
    selector: HostTargetSelector,
    _targets: HostTarget[]
  ): string {
    return this.#topology.resolve(caller, selector)
  }

  listTargets(caller?: HostCallerIdentity, scope: 'current-level' | 'all' = 'all'): HostTarget[] {
    if (!caller) return []
    return this.#topology.list(caller, scope)
  }

  async readCurrent(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown> {
    const active = this.#active.get(sessionId)
    const snapshot = active
      ? await active.snapshotScreen()
      : await replayTerminalScreen(await readSessionFrames(this.#dataRoot, sessionId))
    const bounded = tailText(snapshot.text, limits.maxLines, limits.maxBytes)
    return {
      text: bounded,
      cols: snapshot.cols,
      rows: snapshot.rows,
      truncated: bounded !== snapshot.text,
      source: 'screen'
    }
  }

  async readHistory(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown> {
    const frames = await this.#terminalFrames(sessionId)
    const fullText = outputText(frames)
    const bounded = tailText(fullText, limits.maxLines, limits.maxBytes)
    return {
      text: bounded,
      truncated: bounded !== fullText,
      firstSequence: frames.at(0)?.sequence ?? 0,
      lastSequence: frames.at(-1)?.sequence ?? 0,
      source: 'journal'
    }
  }

  async readCommands(sessionId: string, limits: { limit: number }): Promise<unknown> {
    return this.#commands.list(sessionId).slice(-limits.limit)
  }

  async sendText(sessionId: string, text: string, submit = false): Promise<void> {
    await this.#inputQueue.enqueue(sessionId, () => {
      this.#requireActive(sessionId).write(text + (submit ? '\r' : ''))
    })
  }

  async sendKey(sessionId: string, key: Parameters<HostControlBackend['sendKey']>[1]): Promise<void> {
    await this.#inputQueue.enqueue(sessionId, () => {
      this.#requireActive(sessionId).write(CONTROL_KEY_SEQUENCES[key])
    })
  }

  async writeTaskStatus(taskId: string, key: string, value: string | null): Promise<void> {
    this.#requireActiveTask(taskId)
    this.#telemetry.setStatus(taskId, key, value)
  }

  async writeTaskProgress(taskId: string, progress: number, label?: string): Promise<void> {
    this.#requireActiveTask(taskId)
    this.#telemetry.setProgress(taskId, progress, label)
  }

  async appendTaskLog(
    taskId: string,
    level: Parameters<HostControlBackend['appendTaskLog']>[1],
    source: string,
    message: string
  ): Promise<void> {
    this.#requireActiveTask(taskId)
    const id = this.#telemetry.appendLog(taskId, level, source, message)
    if (level !== 'error' || !this.#notifications) return
    const target = this.#database.get<{
      workspace_id: string; session_id: string; mount_id: string | null
    }>(
      `SELECT tasks.workspace_id, sessions.id AS session_id, session_mounts.id AS mount_id
       FROM tasks
       JOIN sessions ON sessions.task_id = tasks.id AND sessions.archived_at IS NULL
       LEFT JOIN session_mounts ON session_mounts.session_id = sessions.id
       WHERE tasks.id = ? AND tasks.archived_at IS NULL
       ORDER BY sessions.last_activity_at DESC, session_mounts.created_at DESC LIMIT 1`,
      taskId
    )
    if (!target) return
    this.#notifications.ingest({
      eventId: `task-log:${taskId}:${id}`,
      type: 'error',
      title: '事项出错',
      subtitle: source,
      body: message,
      workspaceId: target.workspace_id,
      taskId,
      sessionId: target.session_id,
      ...(target.mount_id === null ? {} : { mountId: target.mount_id }),
      occurredAt: Date.now()
    })
  }

  async moveTaskToWindow(input: {
    migrationId: string
    taskId: string
    sourceWindowId: string
    targetWindowId: string
  }): Promise<unknown> {
    const now = Date.now()
    const pending = this.#taskMigrations.prepare(command(`${input.migrationId}:prepare`), {
      ...input, now
    })
    try {
      return this.#taskMigrations.acknowledgeTarget(
        command(`${input.migrationId}:ack`),
        { migrationId: pending.id, now: Date.now() }
      )
    } catch (error) {
      this.#taskMigrations.fail(command(`${input.migrationId}:rollback`), {
        migrationId: pending.id, reason: errorMessage(error), now: Date.now()
      })
      throw error
    }
  }

  async #terminalFrames(sessionId: string): Promise<DecodedJournalFrame[]> {
    const active = this.#active.get(sessionId)
    return active ? active.readFrames() : readSessionFrames(this.#dataRoot, sessionId)
  }

  #requireActive(sessionId: string): PtySession {
    const session = this.#active.get(sessionId)
    if (!session) throw new HostControlTargetNotReadyError('目标会话当前没有可输入的终端进程')
    return session
  }

  #requireActiveTask(taskId: string): void {
    const task = this.#database.get<{ id: string }>(
      'SELECT id FROM tasks WHERE id = ? AND archived_at IS NULL', taskId
    )
    if (!task) throw new Error(`Task ${taskId} does not exist`)
  }
}

function command(commandId: string) {
  return { commandId, commandType: 'task.move-to-window', requestHash: commandId }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function tailText(text: string, maxLines: number, maxBytes: number): string {
  const lines = text.split('\n').slice(-maxLines).join('\n')
  if (Buffer.byteLength(lines) <= maxBytes) return lines
  const characters = [...lines]
  let bytes = 0
  let start = characters.length
  while (start > 0) {
    const nextBytes = Buffer.byteLength(characters[start - 1]!)
    if (bytes + nextBytes > maxBytes) break
    bytes += nextBytes
    start -= 1
  }
  return characters.slice(start).join('')
}

function outputText(frames: DecodedJournalFrame[]): string {
  const decoder = new TextDecoder()
  let text = ''
  for (const frame of frames) {
    if (frame.kind === 'output') text += decoder.decode(frame.data, { stream: true })
  }
  return text + decoder.decode()
}

async function replayTerminalScreen(
  frames: DecodedJournalFrame[]
): Promise<{ text: string; cols: number; rows: number }> {
  const screen = new TerminalScreenProjector()
  const decoder = new TextDecoder()
  try {
    for (const frame of frames) {
      if (frame.kind === 'output') await screen.write(decoder.decode(frame.data, { stream: true }))
      else if (frame.kind === 'resize') await screen.resize(frame.cols, frame.rows)
      else if (frame.kind === 'reset') await screen.reset()
    }
    const tail = decoder.decode()
    if (tail) await screen.write(tail)
    return await screen.snapshot()
  } finally {
    screen.dispose()
  }
}
