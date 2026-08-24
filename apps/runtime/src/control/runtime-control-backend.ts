import type { HostControlBackend, HostTarget } from './host-control-server'
import { CommandBoundaryRepository } from '../anchors/anchor-resolver'
import { TaskTelemetryRepository } from '../domain/product-foundation-repository'
import { readSessionFrames } from '../journal/segment-journal'
import type { PtySession } from '../session/pty-session'
import type { RuntimeDatabase } from '../storage/database'

export class RuntimeControlBackend implements HostControlBackend {
  readonly #database: RuntimeDatabase
  readonly #dataRoot: string
  readonly #telemetry: TaskTelemetryRepository
  readonly #commands: CommandBoundaryRepository
  readonly #active = new Map<string, PtySession>()

  constructor(database: RuntimeDatabase, dataRoot: string, telemetry: TaskTelemetryRepository) {
    this.#database = database
    this.#dataRoot = dataRoot
    this.#telemetry = telemetry
    this.#commands = new CommandBoundaryRepository(database)
  }

  register(sessionId: string, session: PtySession): void {
    this.#active.set(sessionId, session)
  }

  unregister(sessionId: string, session: PtySession): void {
    if (this.#active.get(sessionId) === session) this.#active.delete(sessionId)
  }

  listTargets(): HostTarget[] {
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

  async sendText(sessionId: string, text: string): Promise<void> {
    this.#requireActive(sessionId).write(text)
  }

  async sendKey(sessionId: string, key: Parameters<HostControlBackend['sendKey']>[1]): Promise<void> {
    this.#requireActive(sessionId).write(KEY_SEQUENCES[key])
  }

  async writeTaskStatus(taskId: string, key: string, value: string | null): Promise<void> {
    this.#telemetry.setStatus(taskId, key, value)
  }

  async writeTaskProgress(taskId: string, progress: number, label?: string): Promise<void> {
    this.#telemetry.setProgress(taskId, progress, label)
  }

  async appendTaskLog(
    taskId: string,
    level: Parameters<HostControlBackend['appendTaskLog']>[1],
    source: string,
    message: string
  ): Promise<void> {
    this.#telemetry.appendLog(taskId, level, source, message)
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
}

const KEY_SEQUENCES = {
  Enter: '\r', Tab: '\t', Escape: '\u001b', ArrowUp: '\u001b[A', ArrowDown: '\u001b[B',
  ArrowLeft: '\u001b[D', ArrowRight: '\u001b[C', CtrlC: '\u0003', CtrlD: '\u0004',
  CtrlL: '\u000c', CtrlZ: '\u001a'
} as const

function tailText(text: string, maxLines: number, maxBytes: number): string {
  const lines = text.split('\n').slice(-maxLines).join('\n')
  const encoded = Buffer.from(lines)
  return encoded.byteLength <= maxBytes
    ? lines
    : encoded.subarray(encoded.byteLength - maxBytes).toString('utf8')
}
