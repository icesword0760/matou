import { randomUUID } from 'node:crypto'

import type { RuntimeDatabase } from '../storage/database'

export const SHELL_HISTORY_BLOCK_LIMIT = 100
export const SHELL_HISTORY_OUTPUT_LINE_LIMIT = 5_000
export const SHELL_HISTORY_OUTPUT_CHARACTER_LIMIT = 1024 * 1024
export const SHELL_HISTORY_PENDING_CHARACTER_LIMIT = 64 * 1024

export interface ShellHistoryBlock {
  id: string
  sessionId: string
  command: string
  cwd: string
  output: string
  exitCode: number
  startedAt: number
  completedAt: number
}

type CompletedBlockInput = Omit<ShellHistoryBlock, 'id'>

interface ShellHistoryBlockRow {
  id: string
  session_id: string
  command_text: string
  cwd: string
  output: string
  exit_code: number
  started_at: number
  completed_at: number
}

export class ShellHistoryRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  complete(input: CompletedBlockInput): ShellHistoryBlock {
    const block: ShellHistoryBlock = {
      id: randomUUID(),
      ...input,
      output: retainNewestOutput(input.output),
    }
    this.#database.transaction((tx) => {
      tx.run(
        `INSERT INTO shell_history_blocks (
           id, session_id, command_text, cwd, output, exit_code, started_at, completed_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
        block.id, block.sessionId, block.command, block.cwd, block.output,
        block.exitCode, block.startedAt, block.completedAt
      )
      tx.run(
        `DELETE FROM shell_history_blocks
         WHERE session_id = ? AND id NOT IN (
           SELECT id FROM shell_history_blocks
           WHERE session_id = ?
           ORDER BY completed_at DESC, id DESC
           LIMIT ?
         )`,
        block.sessionId, block.sessionId, SHELL_HISTORY_BLOCK_LIMIT
      )
    })
    return block
  }

  list(sessionId: string): ShellHistoryBlock[] {
    return this.#database.all<ShellHistoryBlockRow>(
      `SELECT * FROM shell_history_blocks
       WHERE session_id = ?
       ORDER BY completed_at ASC, id ASC`,
      sessionId
    ).map(mapBlock)
  }

  listForLaunch(sessionId: string, enabled: boolean): ShellHistoryBlock[] {
    return enabled ? this.list(sessionId) : []
  }

  purge(sessionId: string): void {
    this.#database.run('DELETE FROM shell_history_blocks WHERE session_id = ?', sessionId)
  }
}

export interface CollectedShellBlock {
  command: string
  output: string
  exitCode: number
  startedAt: number
  completedAt: number
}

interface ActiveBlock {
  command: string
  output: OutputLineTail
  startedAt: number
}

const OSC_START = '\u001b]'
const OSC_END_BEL = '\u0007'
const OSC_END_ST = '\u001b\\'

/**
 * Consumes the shell integration stream without changing the live PTY bytes.
 * Only a command closed by OSC 133 D becomes durable; interrupted commands
 * therefore never masquerade as completed history after a restart.
 */
export class ShellCommandBlockCollector {
  #pending = ''
  #pendingStartedAt: number | undefined
  #active: ActiveBlock | undefined

  ingest(data: string, now = Date.now()): CollectedShellBlock[] {
    const completed: CollectedShellBlock[] = []
    let source = this.#pending + data
    let sourceStartedAt = this.#pending ? (this.#pendingStartedAt ?? now) : now
    this.#pending = ''
    this.#pendingStartedAt = undefined

    while (source.length > 0) {
      const markerStart = source.indexOf(OSC_START)
      if (markerStart < 0) {
        this.#appendOutput(source)
        break
      }
      if (markerStart > 0) this.#appendOutput(source.slice(0, markerStart))
      const bel = source.indexOf(OSC_END_BEL, markerStart + OSC_START.length)
      const st = source.indexOf(OSC_END_ST, markerStart + OSC_START.length)
      const markerEnd = earliestTerminator(bel, st)
      if (markerEnd < 0) {
        const pending = source.slice(markerStart)
        this.#pending = pending.length <= SHELL_HISTORY_PENDING_CHARACTER_LIMIT
          ? pending
          : `${OSC_START}${pending.slice(-(SHELL_HISTORY_PENDING_CHARACTER_LIMIT - OSC_START.length))}`
        this.#pendingStartedAt = markerStart === 0 ? sourceStartedAt : now
        break
      }
      const terminatorLength = markerEnd === st ? OSC_END_ST.length : OSC_END_BEL.length
      const marker = source.slice(markerStart, markerEnd + terminatorLength)
      const command = decodeCommandMarker(marker)
      if (command !== undefined) {
        this.#active = {
          command,
          output: new OutputLineTail(
            SHELL_HISTORY_OUTPUT_LINE_LIMIT,
            SHELL_HISTORY_OUTPUT_CHARACTER_LIMIT
          ),
          startedAt: sourceStartedAt
        }
      } else {
        const exitCode = decodeCompletionMarker(marker)
        if (exitCode !== undefined && this.#active) {
          completed.push({
            command: this.#active.command,
            output: this.#active.output.toString(),
            exitCode,
            startedAt: this.#active.startedAt,
            completedAt: now
          })
          this.#active = undefined
        } else if (!isShellBoundaryMarker(marker)) {
          this.#appendOutput(marker)
        }
      }
      source = source.slice(markerEnd + terminatorLength)
      sourceStartedAt = now
    }
    return completed
  }

  #appendOutput(data: string): void {
    this.#active?.output.append(data)
  }
}

/**
 * Bounds command output while it is arriving instead of first retaining an
 * arbitrarily large string and splitting it when the command completes.
 * The final shape matches retainNewestLines, including its trailing empty line.
 */
class OutputLineTail {
  readonly #maximum: number
  readonly #maximumCharacters: number
  #lines: string[] = []
  #start = 0
  #partial = ''
  #lineCharacters = 0

  constructor(maximum: number, maximumCharacters: number) {
    this.#maximum = maximum
    this.#maximumCharacters = maximumCharacters
  }

  append(data: string): void {
    if (!data) return
    const parts = `${this.#partial}${data}`.split('\n')
    this.#partial = (parts.pop() ?? '').slice(-this.#maximumCharacters)
    for (const original of parts) {
      const line = original.length + 1 > this.#maximumCharacters
        ? original.slice(-(this.#maximumCharacters - 1))
        : original
      this.#lines.push(line)
      this.#lineCharacters += line.length + 1
    }
    while (
      this.#start < this.#lines.length &&
      (
        this.#lines.length - this.#start > this.#maximum ||
        this.#lineCharacters + this.#partial.length > this.#maximumCharacters
      )
    ) {
      this.#lineCharacters -= this.#lines[this.#start]!.length + 1
      this.#start += 1
    }
    if (this.#start >= this.#maximum) {
      this.#lines = this.#lines.slice(this.#start)
      this.#start = 0
    }
  }

  toString(): string {
    const lines = this.#lines.slice(this.#start)
    lines.push(this.#partial)
    return (lines.length <= this.#maximum ? lines : lines.slice(-this.#maximum)).join('\n')
  }
}

export function encodeShellCommandMarker(command: string): string {
  return `\u001b]633;E;${Buffer.from(command, 'utf8').toString('base64')}\u0007`
}

export function formatShellHistoryForTerminal(blocks: readonly ShellHistoryBlock[]): string {
  if (blocks.length === 0) return ''
  const rendered = blocks.map((block) => {
    const output = terminalLineEndings(block.output)
    const terminatedOutput = output && !output.endsWith('\r\n') ? `${output}\r\n` : output
    return `❯ ${block.command}\r\n${terminatedOutput}`
  }).join('\r\n')
  return `${rendered}\r\n\u001b[2m──────── 会话已恢复 ────────\u001b[0m\r\n`
}

function decodeCommandMarker(marker: string): string | undefined {
  const payload = /^\u001b\]633;E;([^\u0007\u001b]*)(?:\u0007|\u001b\\)$/.exec(marker)?.[1]
  if (payload === undefined) return undefined
  try {
    return Buffer.from(payload, 'base64').toString('utf8')
  } catch {
    return undefined
  }
}

function decodeCompletionMarker(marker: string): number | undefined {
  const value = /^\u001b\]133;D;(-?\d+)(?:\u0007|\u001b\\)$/.exec(marker)?.[1]
  if (value === undefined) return undefined
  const exitCode = Number(value)
  return Number.isSafeInteger(exitCode) ? exitCode : undefined
}

function isShellBoundaryMarker(marker: string): boolean {
  return /^\u001b\]133;[ABC](?:;[^\u0007\u001b]*)?(?:\u0007|\u001b\\)$/.test(marker)
}

function earliestTerminator(bel: number, st: number): number {
  if (bel < 0) return st
  if (st < 0) return bel
  return Math.min(bel, st)
}

function retainNewestLines(output: string, maximum: number): string {
  const lines = output.split('\n')
  return lines.length <= maximum ? output : lines.slice(-maximum).join('\n')
}

function retainNewestOutput(output: string): string {
  return retainNewestLines(
    output.slice(-SHELL_HISTORY_OUTPUT_CHARACTER_LIMIT),
    SHELL_HISTORY_OUTPUT_LINE_LIMIT
  )
}

function terminalLineEndings(value: string): string {
  return value.replace(/\r?\n/g, '\r\n')
}

function mapBlock(row: ShellHistoryBlockRow): ShellHistoryBlock {
  return {
    id: row.id,
    sessionId: row.session_id,
    command: row.command_text,
    cwd: row.cwd,
    output: row.output,
    exitCode: row.exit_code,
    startedAt: row.started_at,
    completedAt: row.completed_at
  }
}
