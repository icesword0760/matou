import {
  resolveTargetFromProjection,
  type HostCallerIdentity,
  type HostControlBackend,
  type HostTarget,
  type HostTargetSelector
} from './host-control-server'
import { CommandBoundaryRepository } from '../anchors/anchor-resolver'
import { TaskTelemetryRepository } from '../domain/product-foundation-repository'
import { readSessionFrames } from '../journal/segment-journal'
import type { PtySession } from '../session/pty-session'
import type { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { TaskWindowMigrationService } from '../hierarchy/task-window-migration-service'
import { NotificationProjection } from '../product/experience-foundation'

export class RuntimeControlBackend implements HostControlBackend {
  readonly #database: RuntimeDatabase
  readonly #dataRoot: string
  readonly #telemetry: TaskTelemetryRepository
  readonly #commands: CommandBoundaryRepository
  readonly #notifications: NotificationProjection | undefined
  readonly #active = new Map<string, PtySession>()
  readonly #taskMigrations: TaskWindowMigrationService

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
    this.#taskMigrations = new TaskWindowMigrationService(
      database, new DomainTransactionManager(database)
    )
  }

  register(sessionId: string, session: PtySession): void {
    this.#active.set(sessionId, session)
  }

  unregister(sessionId: string, session: PtySession): void {
    if (this.#active.get(sessionId) === session) this.#active.delete(sessionId)
  }

  identify(caller: HostCallerIdentity): unknown {
    const target = this.listTargets(caller, 'all').find(({ sessionId }) => sessionId === caller.sessionId)
    if (!target) throw new Error(`Session ${caller.sessionId} is not available`)
    return { caller, target }
  }

  resolveTarget(
    caller: HostCallerIdentity,
    selector: HostTargetSelector,
    targets: HostTarget[]
  ): string {
    if (selector.kind === 'self') return caller.sessionId
    return resolveTargetFromProjection(selector, targets)
  }

  listTargets(_caller?: HostCallerIdentity, _scope?: 'current-level' | 'all'): HostTarget[] {
    return this.#database.all<{
      workspace_id: string; task_id: string; session_id: string; mount_id: string; title: string
    }>(
      `SELECT workspaces.id AS workspace_id, tasks.id AS task_id,
              sessions.id AS session_id, session_mounts.id AS mount_id, sessions.title
       FROM session_mounts
       JOIN scenes ON scenes.id = session_mounts.scene_id
       JOIN tasks ON tasks.id = scenes.task_id
       JOIN workspaces ON workspaces.id = tasks.workspace_id
       JOIN sessions ON sessions.id = session_mounts.session_id
       WHERE scenes.archived_at IS NULL AND tasks.archived_at IS NULL
         AND workspaces.archived_at IS NULL AND sessions.archived_at IS NULL
       ORDER BY workspaces.created_at, tasks.created_at, scenes.created_at,
                session_mounts.created_at, session_mounts.id`
    ).map((row, index) => ({
      ref: `surface:${index + 1}`,
      workspaceId: row.workspace_id,
      taskId: row.task_id,
      sessionId: row.session_id,
      mountId: row.mount_id,
      title: row.title
    }))
  }

  async readCurrent(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown> {
    return { text: tailText(await this.#terminalText(sessionId), limits.maxLines, limits.maxBytes), source: 'journal-tail' }
  }

  async readHistory(sessionId: string, limits: { maxLines: number; maxBytes: number }): Promise<unknown> {
    return { text: tailText(await this.#terminalText(sessionId), limits.maxLines, limits.maxBytes), source: 'journal' }
  }

  async readCommands(sessionId: string, limits: { limit: number }): Promise<unknown> {
    return this.#commands.list(sessionId).slice(-limits.limit)
  }

  async sendText(sessionId: string, text: string, submit = false): Promise<void> {
    this.#requireActive(sessionId).write(text + (submit ? '\r' : ''))
  }

  async sendKey(sessionId: string, key: Parameters<HostControlBackend['sendKey']>[1]): Promise<void> {
    this.#requireActive(sessionId).write(KEY_SEQUENCES[key])
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

  async #terminalText(sessionId: string): Promise<string> {
    const active = this.#active.get(sessionId)
    const frames = active ? await active.readFrames() : await readSessionFrames(this.#dataRoot, sessionId)
    const decoder = new TextDecoder()
    let text = ''
    for (const frame of frames) {
      if (frame.kind === 'output') text += decoder.decode(frame.data, { stream: true })
    }
    return text + decoder.decode()
  }

  #requireActive(sessionId: string): PtySession {
    const session = this.#active.get(sessionId)
    if (!session) throw new Error(`Session ${sessionId} is not active`)
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

const KEY_SEQUENCES = {
  Enter: '\r', Tab: '\t', Escape: '\u001b', Backspace: '\u007f', Delete: '\u001b[3~',
  ArrowUp: '\u001b[A', ArrowDown: '\u001b[B', ArrowLeft: '\u001b[D', ArrowRight: '\u001b[C',
  Home: '\u001b[H', End: '\u001b[F', PageUp: '\u001b[5~', PageDown: '\u001b[6~',
  CtrlC: '\u0003', CtrlD: '\u0004', CtrlL: '\u000c', CtrlU: '\u0015', CtrlZ: '\u001a'
} as const

function tailText(text: string, maxLines: number, maxBytes: number): string {
  const lines = text.split('\n').slice(-maxLines).join('\n')
  const encoded = Buffer.from(lines)
  return encoded.byteLength <= maxBytes
    ? lines
    : encoded.subarray(encoded.byteLength - maxBytes).toString('utf8')
}
