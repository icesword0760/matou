import { randomUUID } from 'node:crypto'
import { mkdir, open, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'

import {
  JournalCorruptionError,
  iterateSegmentFrames,
  listReadableJournalSegments,
  type DecodedJournalFrame,
  type ReadableJournalSegment
} from './segment-journal'
import {
  JournalTailIndex,
  loadJournalTailIndex,
  writeJournalTailIndex
} from './journal-tail-index'

const VERSION = 1 as const
const RANGE_INDEX_FILE = 'range-index.json'

export interface JournalBoundsSegment {
  index: number
  path: string
  firstSequence: number
  lastSequence: number
}

export interface JournalBounds {
  firstSequence: number
  lastSequence: number
  segments: readonly JournalBoundsSegment[]
}

export interface JournalReplayMetadata {
  firstSequence: number
  lastSequence: number
  tailFromSequence: number
  domainEventSequence: number
}

interface StoredJournalRangeIndex {
  version: typeof VERSION
  segments: Array<{
    index: number
    firstSequence: number
    lastSequence: number
    sourcePath: string
    sourceBytes: number
  }>
}

export async function readSessionJournalBounds(
  dataRoot: string,
  sessionId: string
): Promise<JournalBounds> {
  const directory = join(dataRoot, 'journal', sessionId)
  const groups = await listReadableJournalSegments(directory)
  return readBoundsForGroups(directory, groups)
}

async function readBoundsForGroups(
  directory: string,
  groups: readonly ReadableJournalSegment[]
): Promise<JournalBounds> {
  const indexPath = join(directory, RANGE_INDEX_FILE)
  const stored = await loadRangeIndex(indexPath)
  const storedByIndex = new Map(stored?.segments.map((segment) => [segment.index, segment]))
  const segments: JournalBoundsSegment[] = []
  const indexedSegments: StoredJournalRangeIndex['segments'] = []
  const activeIndex = groups.at(-1)?.index

  for (const group of groups) {
    const cached = storedByIndex.get(group.index)
    const currentPath = group.paths[0]!
    const currentBytes = group.index === activeIndex ? (await stat(currentPath)).size : undefined
    const useCached = cached !== undefined && (
      group.index !== activeIndex || (
        cached.sourcePath === basename(currentPath) &&
        cached.sourceBytes === currentBytes
      )
    )
    const bounds = useCached ? cached : await scanSegmentBounds(group)
    if (!bounds) continue
    segments.push({
      index: group.index,
      path: currentPath,
      firstSequence: bounds.firstSequence,
      lastSequence: bounds.lastSequence
    })
    indexedSegments.push({
      index: group.index,
      firstSequence: bounds.firstSequence,
      lastSequence: bounds.lastSequence,
      sourcePath: basename(currentPath),
      sourceBytes: useCached ? cached.sourceBytes : (await stat(currentPath)).size
    })
  }
  assertMonotonicBounds(segments)
  const refreshed: StoredJournalRangeIndex = {
    version: VERSION,
    segments: indexedSegments
  }
  if (!sameRangeIndex(stored, refreshed)) {
    await writeRangeIndex(indexPath, refreshed).catch(() => undefined)
  }
  return {
    firstSequence: segments[0]?.firstSequence ?? 0,
    lastSequence: segments.at(-1)?.lastSequence ?? 0,
    segments
  }
}

export async function readSessionReplayMetadata(
  dataRoot: string,
  sessionId: string,
  tailLineLimit = 10_000,
  domainThroughSequence?: number
): Promise<JournalReplayMetadata> {
  const directory = join(dataRoot, 'journal', sessionId)
  const bounds = await readSessionJournalBounds(dataRoot, sessionId)
  if (bounds.lastSequence === 0) {
    return {
      firstSequence: 0,
      lastSequence: 0,
      tailFromSequence: 0,
      domainEventSequence: 0
    }
  }
  const tailIndexPath = join(directory, 'tail-index.json')
  let index: JournalTailIndex
  try {
    const snapshot = await loadJournalTailIndex(tailIndexPath)
    if (
      snapshot.firstSequence !== bounds.firstSequence ||
      snapshot.lastSequence !== bounds.lastSequence
    ) {
      throw new Error('tail index does not cover current Journal bounds')
    }
    index = JournalTailIndex.fromSnapshot(snapshot)
  } catch {
    index = new JournalTailIndex()
    for await (const frame of iterateSessionFrames(dataRoot, sessionId, {
      fromSequence: bounds.firstSequence,
      throughSequence: bounds.lastSequence
    })) {
      recordTailFrame(index, frame)
    }
    await writeJournalTailIndex(
      tailIndexPath,
      index.snapshot(bounds.segments.at(-1)?.index ?? 0)
    ).catch(() => undefined)
  }
  return {
    firstSequence: bounds.firstSequence,
    lastSequence: bounds.lastSequence,
    tailFromSequence: index.tailStart(tailLineLimit),
    domainEventSequence: index.domainEventSequenceAtOrBefore(
      domainThroughSequence ?? bounds.lastSequence
    )
  }
}

export async function recordJournalSegmentBounds(
  directory: string,
  segment: {
    index: number
    firstSequence: number
    lastSequence: number
    sourcePath: string
    sourceBytes: number
  }
): Promise<void> {
  if (
    !Number.isSafeInteger(segment.index) || segment.index < 1 ||
    !Number.isSafeInteger(segment.firstSequence) || segment.firstSequence < 1 ||
    !Number.isSafeInteger(segment.lastSequence) || segment.lastSequence < segment.firstSequence ||
    !segment.sourcePath || basename(segment.sourcePath) !== segment.sourcePath ||
    !Number.isSafeInteger(segment.sourceBytes) || segment.sourceBytes < 1
  ) {
    throw new Error('journal segment bounds are invalid')
  }
  const path = join(directory, RANGE_INDEX_FILE)
  const current = await loadRangeIndex(path)
  const byIndex = new Map(
    (current?.segments ?? []).map((candidate) => [candidate.index, candidate])
  )
  const existing = byIndex.get(segment.index)
  if (existing && sameStoredSegment(existing, segment)) return
  byIndex.set(segment.index, { ...segment })
  const next: StoredJournalRangeIndex = {
    version: VERSION,
    segments: [...byIndex.values()].sort((left, right) => left.index - right.index)
  }
  validateRangeIndex(next)
  await writeRangeIndex(path, next).catch(() => undefined)
}

export async function* iterateSessionFrames(
  dataRoot: string,
  sessionId: string,
  options: { fromSequence: number; throughSequence?: number }
): AsyncGenerator<DecodedJournalFrame> {
  validateSequence(options.fromSequence, 'fromSequence')
  if (options.throughSequence !== undefined) {
    validateSequence(options.throughSequence, 'throughSequence')
  }
  const directory = join(dataRoot, 'journal', sessionId)
  const groups = await listReadableJournalSegments(directory)
  const bounds = await readBoundsForGroups(directory, groups)
  const groupsByIndex = new Map(groups.map((group) => [group.index, group]))
  let previousSequence = 0
  for (const segment of bounds.segments) {
    if (segment.lastSequence < options.fromSequence) continue
    if (
      options.throughSequence !== undefined &&
      segment.firstSequence > options.throughSequence
    ) break
    const group = groupsByIndex.get(segment.index)
    if (!group) throw new JournalCorruptionError('Journal segment is unavailable', 0)
    for await (const frame of iteratePreferredSegment(group.paths)) {
      if (frame.sequence <= previousSequence) {
        throw new JournalCorruptionError('non-monotonic sequence across journal segments', 0)
      }
      previousSequence = frame.sequence
      if (frame.sequence < options.fromSequence) continue
      if (
        options.throughSequence !== undefined &&
        frame.sequence > options.throughSequence
      ) return
      yield frame
    }
  }
}

async function scanSegmentBounds(
  group: ReadableJournalSegment
): Promise<Pick<JournalBoundsSegment, 'firstSequence' | 'lastSequence'> | undefined> {
  let firstSequence = 0
  let lastSequence = 0
  for await (const frame of iteratePreferredSegment(group.paths)) {
    if (firstSequence === 0) firstSequence = frame.sequence
    lastSequence = frame.sequence
  }
  if (firstSequence === 0) return undefined
  return { firstSequence, lastSequence }
}

function recordTailFrame(index: JournalTailIndex, frame: DecodedJournalFrame): void {
  index.record(
    frame.sequence,
    frame.kind === 'output' ? frame.data : EMPTY_BYTES,
    frame.kind === 'domain-cursor' ? frame.domainEventSequence : undefined
  )
}

const EMPTY_BYTES = new Uint8Array()

async function* iteratePreferredSegment(
  paths: readonly string[]
): AsyncGenerator<DecodedJournalFrame> {
  let failure: unknown
  let emittedThrough = 0
  for (const path of paths) {
    try {
      for await (const frame of iterateSegmentFrames(path)) {
        if (frame.sequence <= emittedThrough) continue
        emittedThrough = frame.sequence
        yield frame
      }
      return
    } catch (error) {
      failure = error
    }
  }
  throw failure ?? new JournalCorruptionError('Journal segment is unavailable', 0)
}

async function loadRangeIndex(path: string): Promise<StoredJournalRangeIndex | undefined> {
  try {
    const value = JSON.parse(await readFile(path, 'utf8')) as unknown
    validateRangeIndex(value)
    return value
  } catch (error) {
    if (isMissingFile(error)) return undefined
    return undefined
  }
}

async function writeRangeIndex(path: string, index: StoredJournalRangeIndex): Promise<void> {
  validateRangeIndex(index)
  await mkdir(dirname(path), { recursive: true, mode: 0o700 })
  try {
    const temporaryPath = `${path}.tmp-${randomUUID()}`
    try {
      await writeFile(temporaryPath, `${JSON.stringify(index)}\n`, { mode: 0o600 })
      const handle = await open(temporaryPath, 'r')
      try {
        await handle.sync()
      } finally {
        await handle.close()
      }
      await rename(temporaryPath, path)
    } finally {
      await rm(temporaryPath, { force: true })
    }
  } finally {
    const directory = await open(dirname(path), 'r')
    try {
      await directory.sync()
    } finally {
      await directory.close()
    }
  }
}

function validateRangeIndex(value: unknown): asserts value is StoredJournalRangeIndex {
  if (!value || typeof value !== 'object') throw new Error('range index must be an object')
  const index = value as Partial<StoredJournalRangeIndex>
  if (index.version !== VERSION || !Array.isArray(index.segments)) {
    throw new Error('range index fields are invalid')
  }
  const normalized = index.segments as StoredJournalRangeIndex['segments']
  for (let offset = 0; offset < normalized.length; offset += 1) {
    const segment = normalized[offset]
    const previous = normalized[offset - 1]
    if (
      !segment ||
      !Number.isSafeInteger(segment.index) || segment.index < 1 ||
      !Number.isSafeInteger(segment.firstSequence) || segment.firstSequence < 1 ||
      !Number.isSafeInteger(segment.lastSequence) || segment.lastSequence < segment.firstSequence ||
      typeof segment.sourcePath !== 'string' || !segment.sourcePath ||
      basename(segment.sourcePath) !== segment.sourcePath ||
      !Number.isSafeInteger(segment.sourceBytes) || segment.sourceBytes < 1 ||
      (previous !== undefined && (
        segment.index <= previous.index || segment.firstSequence <= previous.lastSequence
      ))
    ) {
      throw new Error('range index fields are invalid')
    }
  }
}

function assertMonotonicBounds(segments: readonly JournalBoundsSegment[]): void {
  for (let index = 1; index < segments.length; index += 1) {
    const previous = segments[index - 1]!
    const current = segments[index]!
    if (
      current.index <= previous.index ||
      current.firstSequence <= previous.lastSequence
    ) {
      throw new JournalCorruptionError('non-monotonic sequence across journal segments', 0)
    }
  }
}

function sameRangeIndex(
  left: StoredJournalRangeIndex | undefined,
  right: StoredJournalRangeIndex
): boolean {
  if (!left || left.segments.length !== right.segments.length) return false
  return left.segments.every((segment, index) => {
    const candidate = right.segments[index]
    return candidate !== undefined && sameStoredSegment(segment, candidate)
  })
}

function sameStoredSegment(
  left: StoredJournalRangeIndex['segments'][number],
  right: StoredJournalRangeIndex['segments'][number]
): boolean {
  return left.index === right.index &&
    left.firstSequence === right.firstSequence &&
    left.lastSequence === right.lastSequence &&
    left.sourcePath === right.sourcePath &&
    left.sourceBytes === right.sourceBytes
}

function validateSequence(value: number, name: string): void {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`${name} must be a non-negative integer`)
  }
}

function isMissingFile(error: unknown): boolean {
  return error instanceof Error && 'code' in error && error.code === 'ENOENT'
}
