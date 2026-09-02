import { readdir } from 'node:fs/promises'
import { join } from 'node:path'

import type {
  TerminalHistoryCursor,
  TerminalHistoryGap,
  TerminalHistoryLine,
  TerminalHistoryPage,
  TerminalHistorySearchOptions,
  TerminalHistorySearchResult
} from '@matou/contracts'

import { JournalCorruptionError, readSegmentFrames, type DecodedJournalFrame } from './segment-journal'

const MAX_LINES = 1_000
const DEFAULT_SEARCH_OPTIONS: TerminalHistorySearchOptions = {
  caseSensitive: false,
  regex: false,
  wholeWord: false
}

export class JournalHistoryReader {
  readonly #dataRoot: string

  constructor(dataRoot: string) {
    this.#dataRoot = dataRoot
  }

  async page(input: {
    sessionId: string
    before?: TerminalHistoryCursor
    lineLimit?: number
  }): Promise<TerminalHistoryPage> {
    const limit = boundedLimit(input.lineLimit)
    const ring: TerminalHistoryLine[] = []
    const gaps: TerminalHistoryGap[] = []
    await this.#scan(input.sessionId, gaps, (line) => {
      if (input.before && compareCursor(line.cursor, input.before) >= 0) return
      ring.push(line)
      if (ring.length > limit + 1) ring.shift()
    })
    const hasMore = ring.length > limit
    return { lines: hasMore ? ring.slice(1) : ring, gaps, hasMore }
  }

  async search(input: {
    sessionId: string
    query: string
    before?: TerminalHistoryCursor
    limit?: number
    options?: TerminalHistorySearchOptions
  }): Promise<TerminalHistorySearchResult> {
    if (!input.query) return { matches: [], gaps: [], hasMore: false }
    const limit = boundedLimit(input.limit)
    const matchesQuery = compileSearch(input.query, input.options ?? DEFAULT_SEARCH_OPTIONS)
    const ring: TerminalHistoryLine[] = []
    const gaps: TerminalHistoryGap[] = []
    await this.#scan(input.sessionId, gaps, (line) => {
      if (input.before && compareCursor(line.cursor, input.before) >= 0) return
      if (!matchesQuery(line.text)) return
      ring.push(line)
      if (ring.length > limit + 1) ring.shift()
    })
    const hasMore = ring.length > limit
    return { matches: (hasMore ? ring.slice(1) : ring).reverse(), gaps, hasMore }
  }

  async #scan(
    sessionId: string,
    gaps: TerminalHistoryGap[],
    onLine: (line: TerminalHistoryLine) => void
  ): Promise<void> {
    const directory = join(this.#dataRoot, 'journal', sessionId)
    let entries: string[]
    try {
      entries = await readdir(directory)
    } catch (error) {
      if (isMissing(error)) return
      throw error
    }
    const groups = groupSegments(directory, entries)
    addMissingSegmentGaps(groups, gaps)
    let decoder = new TextDecoder('utf-8', { fatal: false })
    let pending = ''
    let pendingSequence = 0
    let pendingLineIndex = 0

    for (const group of groups) {
      let loaded = await readFirstValid(group.paths)
      if (!loaded.frames) {
        // Compression publishes gzip before removing raw. If the directory was
        // listed just before raw removal, refresh this one index so that normal
        // publication never appears to users as a damaged-history gap.
        const refreshed = groupSegments(directory, await readdir(directory))
          .find(({ index }) => index === group.index)
        if (refreshed) loaded = await readFirstValid(refreshed.paths)
      }
      if (!loaded.frames) {
        gaps.push({
          segmentIndex: group.index,
          code: loaded.missing ? 'MISSING_SEGMENT' : 'CORRUPT_SEGMENT',
          message: loaded.error ?? 'Journal segment is unavailable'
        })
        decoder = new TextDecoder('utf-8', { fatal: false })
        pending = ''
        pendingSequence = 0
        pendingLineIndex = 0
        continue
      }
      for (const frame of loaded.frames) {
        if (frame.kind !== 'output') continue
        pendingSequence = frame.sequence
        pendingLineIndex = 0
        pending += decoder.decode(frame.data, { stream: true })
        let lineBreak = pending.indexOf('\n')
        while (lineBreak >= 0) {
          const cursor = { sequence: frame.sequence, lineIndex: pendingLineIndex }
          onLine({
            sequence: frame.sequence,
            cursor,
            text: normalizeTerminalLine(pending.slice(0, lineBreak))
          })
          pendingLineIndex += 1
          pending = pending.slice(lineBreak + 1)
          lineBreak = pending.indexOf('\n')
        }
      }
    }
    pending += decoder.decode()
    if (pending) {
      onLine({
        sequence: pendingSequence,
        cursor: { sequence: pendingSequence, lineIndex: pendingLineIndex },
        text: normalizeTerminalLine(pending)
      })
    }
    gaps.sort((left, right) => left.segmentIndex - right.segmentIndex)
  }
}

interface SegmentGroup {
  index: number
  paths: string[]
}

function groupSegments(directory: string, entries: string[]): SegmentGroup[] {
  const groups = new Map<number, Array<{
    path: string
    compressed: boolean
    format: 'mtj' | 'legacy-bin'
  }>>()
  for (const entry of entries) {
    const match = /^segment-(\d{6})\.(mtj|bin)(\.gz)?$/.exec(entry)
    if (!match) continue
    const index = Number(match[1])
    const values = groups.get(index) ?? []
    values.push({
      path: join(directory, entry),
      format: match[2] === 'mtj' ? 'mtj' : 'legacy-bin',
      compressed: match[3] === '.gz'
    })
    groups.set(index, values)
  }
  return [...groups.entries()]
    .sort(([left], [right]) => left - right)
    .map(([index, values]) => ({
      index,
      paths: values
        .sort((left, right) => segmentReadPriority(left) - segmentReadPriority(right))
        .map(({ path }) => path)
    }))
}

function segmentReadPriority(segment: {
  compressed: boolean
  format: 'mtj' | 'legacy-bin'
}): number {
  return Number(segment.compressed) * 2 + Number(segment.format === 'legacy-bin')
}

function addMissingSegmentGaps(groups: SegmentGroup[], gaps: TerminalHistoryGap[]): void {
  if (groups.length < 2) return
  const present = new Set(groups.map(({ index }) => index))
  const first = groups[0]!.index
  const last = groups.at(-1)!.index
  for (let index = first; index <= last; index += 1) {
    if (!present.has(index)) {
      gaps.push({
        segmentIndex: index,
        code: 'MISSING_SEGMENT',
        message: 'Journal segment is missing'
      })
    }
  }
}

async function readFirstValid(paths: string[]): Promise<{
  frames?: DecodedJournalFrame[]
  missing: boolean
  error?: string
}> {
  let error: string | undefined
  let missing = true
  for (const path of paths) {
    try {
      return { frames: await readSegmentFrames(path), missing: false }
    } catch (cause) {
      if (isMissing(cause)) continue
      missing = false
      error = cause instanceof JournalCorruptionError ? cause.message : errorMessage(cause)
    }
  }
  return { missing, ...(error ? { error } : {}) }
}

function compileSearch(query: string, options: TerminalHistorySearchOptions): (text: string) => boolean {
  if (options.regex) {
    let expression: RegExp
    try {
      expression = new RegExp(query, options.caseSensitive ? '' : 'i')
    } catch (error) {
      throw new Error(`invalid terminal history search pattern: ${errorMessage(error)}`)
    }
    return (text) => expression.test(text)
  }
  const needle = options.caseSensitive ? query : query.toLocaleLowerCase()
  if (!options.wholeWord) {
    return (text) => (options.caseSensitive ? text : text.toLocaleLowerCase()).includes(needle)
  }
  const expression = new RegExp(`(?:^|[^\\p{L}\\p{N}_])${escapeRegex(needle)}(?=$|[^\\p{L}\\p{N}_])`, 'u')
  return (text) => expression.test(options.caseSensitive ? text : text.toLocaleLowerCase())
}

function compareCursor(left: TerminalHistoryCursor, right: TerminalHistoryCursor): number {
  return left.sequence - right.sequence || left.lineIndex - right.lineIndex
}

function boundedLimit(value: number | undefined): number {
  if (value === undefined || !Number.isFinite(value)) return MAX_LINES
  return Math.max(1, Math.min(MAX_LINES, Math.floor(value)))
}

function normalizeTerminalLine(value: string): string {
  const withoutCarriageReturn = value.endsWith('\r') ? value.slice(0, -1) : value
  return withoutCarriageReturn.replace(
    // CSI and OSC sequences do not represent searchable terminal text.
    /\u001b(?:\[[0-?]*[ -/]*[@-~]|\][^\u0007]*(?:\u0007|\u001b\\))/g,
    ''
  )
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function isMissing(error: unknown): boolean {
  return error instanceof Error && 'code' in error && error.code === 'ENOENT'
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
