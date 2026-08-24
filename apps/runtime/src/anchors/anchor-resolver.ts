import { createHash } from 'node:crypto'

import type {
  CommandOutputAnchor,
  ScreenCaptureAnchor,
  SemanticAnchor
} from '@matou/domain'

import type { DecodedJournalFrame } from '../journal/segment-journal'
import type { RuntimeDatabase } from '../storage/database'

export interface TerminalCommandBoundary {
  commandId: string
  sessionId: string
  startedSequence: number
  executedSequence?: number
  endedSequence?: number
  commandText?: string
  cwd?: string
  exitCode?: number
  createdAt: number
  updatedAt: number
}

interface CommandRow {
  command_id: string
  session_id: string
  started_sequence: number
  executed_sequence: number | null
  ended_sequence: number | null
  command_text: string | null
  cwd: string | null
  exit_code: number | null
  created_at: number
  updated_at: number
}

export class CommandBoundaryRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  start(input: {
    commandId: string
    sessionId: string
    sequence: number
    commandText?: string
    cwd?: string
    now: number
  }): TerminalCommandBoundary {
    this.#database.run(
      `INSERT INTO terminal_commands (
         command_id, session_id, started_sequence, command_text, cwd, created_at, updated_at
       ) VALUES (?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(command_id) DO NOTHING`,
      input.commandId,
      input.sessionId,
      input.sequence,
      input.commandText ?? null,
      input.cwd ?? null,
      input.now,
      input.now
    )
    return this.get(input.commandId)!
  }

  markExecuted(commandId: string, sequence: number, now: number): void {
    this.#database.run(
      `UPDATE terminal_commands SET executed_sequence = ?, updated_at = ?
       WHERE command_id = ? AND ended_sequence IS NULL`,
      sequence,
      now,
      commandId
    )
  }

  finish(commandId: string, sequence: number, exitCode: number, now: number): void {
    this.#database.run(
      `UPDATE terminal_commands
       SET ended_sequence = ?, exit_code = ?, updated_at = ?
       WHERE command_id = ?`,
      sequence,
      exitCode,
      now,
      commandId
    )
  }

  get(commandId: string): TerminalCommandBoundary | undefined {
    const row = this.#database.get<CommandRow>(
      'SELECT * FROM terminal_commands WHERE command_id = ?', commandId
    )
    return row ? mapCommand(row) : undefined
  }

  list(sessionId: string): TerminalCommandBoundary[] {
    return this.#database
      .all<CommandRow>(
        'SELECT * FROM terminal_commands WHERE session_id = ? ORDER BY started_sequence',
        sessionId
      )
      .map(mapCommand)
  }
}

export class Osc133Tracker {
  readonly #sessionId: string
  readonly #repository: CommandBoundaryRepository
  #currentCommandId: string | undefined

  constructor(sessionId: string, repository: CommandBoundaryRepository) {
    this.#sessionId = sessionId
    this.#repository = repository
  }

  ingest(data: string, sequence: number, now: number): void {
    const expression = /\u001b\]133;([A-D])(?:;([^\u0007\u001b]*))?(?:\u0007|\u001b\\)/g
    for (const match of data.matchAll(expression)) {
      const marker = match[1]
      const parameter = match[2]
      if (marker === 'A') {
        this.#currentCommandId = undefined
      } else if (marker === 'B') {
        this.#currentCommandId = createHash('sha256')
          .update(`${this.#sessionId}\0${sequence}`)
          .digest('hex')
        this.#repository.start({
          commandId: this.#currentCommandId,
          sessionId: this.#sessionId,
          sequence,
          now
        })
      } else if (marker === 'C' && this.#currentCommandId) {
        this.#repository.markExecuted(this.#currentCommandId, sequence, now)
      } else if (marker === 'D' && this.#currentCommandId) {
        const exitCode = parameter === undefined ? 0 : Number.parseInt(parameter, 10)
        this.#repository.finish(
          this.#currentCommandId,
          sequence,
          Number.isFinite(exitCode) ? exitCode : 0,
          now
        )
        this.#currentCommandId = undefined
      }
    }
  }
}

export class AnchorResolver {
  readonly #database: RuntimeDatabase
  readonly #readFrames: (sessionId: string) => Promise<DecodedJournalFrame[]>
  readonly #boundaries: CommandBoundaryRepository

  constructor(
    database: RuntimeDatabase,
    readFrames: (sessionId: string) => Promise<DecodedJournalFrame[]>,
    boundaries: CommandBoundaryRepository
  ) {
    this.#database = database
    this.#readFrames = readFrames
    this.#boundaries = boundaries
  }

  resolveSemantic(anchor: SemanticAnchor):
    | { status: 'resolved'; eventSequence: number; payload: unknown }
    | { status: 'degraded'; reason: 'event-missing' } {
    const event = this.#database.get<{ seq: number; payload_json: string }>(
      'SELECT seq, payload_json FROM domain_events WHERE event_id = ? AND session_id = ?',
      anchor.eventId,
      anchor.sessionId
    )
    return event
      ? { status: 'resolved', eventSequence: event.seq, payload: JSON.parse(event.payload_json) as unknown }
      : { status: 'degraded', reason: 'event-missing' }
  }

  async resolveCommandOutput(anchor: CommandOutputAnchor): Promise<
    | { status: 'resolved'; text: string; startSequence: number; endSequence: number }
    | { status: 'degraded'; reason: 'command-missing' | 'command-boundary-mismatch' | 'journal-retention'; text: string }
  > {
    const command = this.#boundaries.get(anchor.commandId)
    if (!command || command.sessionId !== anchor.sessionId) {
      return { status: 'degraded', reason: 'command-missing', text: '' }
    }
    if (
      command.startedSequence !== anchor.startSequence ||
      (command.endedSequence !== undefined && command.endedSequence !== anchor.endSequence)
    ) {
      return { status: 'degraded', reason: 'command-boundary-mismatch', text: '' }
    }
    const frames = await this.#readFrames(anchor.sessionId)
    const output = outputText(frames, anchor.startSequence, anchor.endSequence)
    const minimum = frames.at(0)?.sequence
    if (minimum === undefined || minimum > anchor.startSequence) {
      return { status: 'degraded', reason: 'journal-retention', text: output }
    }
    return {
      status: 'resolved',
      text: output,
      startSequence: anchor.startSequence,
      endSequence: anchor.endSequence
    }
  }

  async resolveScreenCapture(anchor: ScreenCaptureAnchor): Promise<
    | { status: 'resolved'; text: string }
    | { status: 'degraded'; reason: 'journal-retention' | 'screen-epoch-mismatch'; text: string }
  > {
    const frames = await this.#readFrames(anchor.sessionId)
    if (!frames.some(({ sequence }) => sequence === anchor.sequence)) {
      return { status: 'degraded', reason: 'journal-retention', text: anchor.capturedText }
    }
    let epoch = 0
    for (const frame of frames) {
      if (frame.sequence > anchor.sequence) break
      if (frame.kind === 'reset') epoch = frame.screenEpoch
    }
    if (epoch !== anchor.screenEpoch) {
      return { status: 'degraded', reason: 'screen-epoch-mismatch', text: anchor.capturedText }
    }
    return { status: 'resolved', text: anchor.capturedText }
  }
}

function outputText(frames: DecodedJournalFrame[], start: number, end: number): string {
  const decoder = new TextDecoder()
  let text = ''
  for (const frame of frames) {
    if (frame.sequence < start || frame.sequence > end || frame.kind !== 'output') continue
    text += decoder.decode(frame.data, { stream: true })
  }
  text += decoder.decode()
  return text
}

function mapCommand(row: CommandRow): TerminalCommandBoundary {
  return {
    commandId: row.command_id,
    sessionId: row.session_id,
    startedSequence: row.started_sequence,
    ...(row.executed_sequence === null ? {} : { executedSequence: row.executed_sequence }),
    ...(row.ended_sequence === null ? {} : { endedSequence: row.ended_sequence }),
    ...(row.command_text === null ? {} : { commandText: row.command_text }),
    ...(row.cwd === null ? {} : { cwd: row.cwd }),
    ...(row.exit_code === null ? {} : { exitCode: row.exit_code }),
    createdAt: row.created_at,
    updatedAt: row.updated_at
  }
}
