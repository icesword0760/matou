import { createReadStream } from 'node:fs'
import { createGunzip } from 'node:zlib'
import {
  chmod,
  mkdir,
  open,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  truncate,
} from 'node:fs/promises'
import type { FileHandle } from 'node:fs/promises'
import { join } from 'node:path'

import {
  defaultJournalCompressor,
  type JournalCompressor
} from './journal-compressor'

import {
  SEGMENT_BYTES,
  selectCompressionCandidates,
  type SegmentDescriptor
} from './journal-policy'
import {
  JournalTailIndex,
  loadJournalTailIndex,
  writeJournalTailIndex,
  type JournalTailIndexSnapshot
} from './journal-tail-index'

const MAGIC = Buffer.from('MTJRNL2\n', 'ascii')
const FRAME_PREFIX_BYTES = 8
const MAX_FRAME_BYTES = 64 * 1024 * 1024

type StoredHeader =
  | { kind: 'output'; sequence: number; timestamp: number; dataLength: number }
  | { kind: 'resize'; sequence: number; timestamp: number; dataLength: 0; cols: number; rows: number }
  | { kind: 'reset'; sequence: number; timestamp: number; dataLength: 0; screenEpoch: number }
  | { kind: 'encoding'; sequence: number; timestamp: number; dataLength: 0; encoding: string }
  | { kind: 'domain-cursor'; sequence: number; timestamp: number; dataLength: 0; domainEventSequence: number }
  | { kind: 'exit'; sequence: number; timestamp: number; dataLength: 0; exitCode: number; signal?: number }

export type DecodedJournalFrame =
  | { kind: 'output'; sequence: number; data: Uint8Array }
  | { kind: 'resize'; sequence: number; cols: number; rows: number }
  | { kind: 'reset'; sequence: number; screenEpoch: number }
  | { kind: 'encoding'; sequence: number; encoding: string }
  | { kind: 'domain-cursor'; sequence: number; domainEventSequence: number }
  | { kind: 'exit'; sequence: number; exitCode: number; signal?: number }

export interface SegmentJournalOptions {
  maxSegmentBytes?: number
  compressSealed?: boolean
  compressor?: JournalCompressor
  /** Fault-injectable frame boundary; production callers use the default durable file writer. */
  writeFrame?: (handle: FileHandle, encoded: Buffer) => Promise<void>
}

type NormalizedSegmentJournalOptions = {
  maxSegmentBytes: number
  compressSealed: boolean
  compressor: JournalCompressor
  writeFrame: (handle: FileHandle, encoded: Buffer) => Promise<void>
}

export interface TailRepairResult {
  truncatedBytes: number
  lastSequence: number
}

export class JournalCorruptionError extends Error {
  readonly offset: number
  readonly quarantinePath: string | undefined

  constructor(message: string, offset: number, quarantinePath?: string) {
    super(message)
    this.name = 'JournalCorruptionError'
    this.offset = offset
    this.quarantinePath = quarantinePath
  }
}

export class SegmentJournal {
  readonly directory: string
  readonly #maxSegmentBytes: number
  readonly #writeFrame: NormalizedSegmentJournalOptions['writeFrame']
  readonly #compressSealed: boolean
  readonly #compressor: JournalCompressor
  readonly #sealedSegments: SegmentDescriptor[]
  readonly #tailIndex: JournalTailIndex
  readonly #tailIndexPath: string
  #tailIndexWrite = Promise.resolve()
  #handle: FileHandle
  #segmentIndex: number
  #path: string
  #size: number
  #lastSequence: number
  #closed = false

  private constructor(
    directory: string,
    handle: FileHandle,
    segmentIndex: number,
    path: string,
    size: number,
    lastSequence: number,
    sealedSegments: SegmentDescriptor[],
    tailIndex: JournalTailIndex,
    tailIndexPath: string,
    options: NormalizedSegmentJournalOptions
  ) {
    this.directory = directory
    this.#handle = handle
    this.#segmentIndex = segmentIndex
    this.#path = path
    this.#size = size
    this.#lastSequence = lastSequence
    this.#maxSegmentBytes = options.maxSegmentBytes
    this.#writeFrame = options.writeFrame
    this.#compressSealed = options.compressSealed
    this.#compressor = options.compressor
    this.#sealedSegments = sealedSegments
    this.#tailIndex = tailIndex
    this.#tailIndexPath = tailIndexPath
    this.#scheduleSealedCompression()
  }

  get path(): string {
    return this.#path
  }

  get lastSequence(): number {
    return this.#lastSequence
  }

  tailStart(maxLines = 10_000): number {
    return this.#tailIndex.tailStart(maxLines)
  }

  tailIndexSnapshot(): JournalTailIndexSnapshot {
    return this.#tailIndex.snapshot(this.#segmentIndex)
  }

  domainEventSequenceAtOrBefore(sequence: number): number {
    return this.#tailIndex.domainEventSequenceAtOrBefore(sequence)
  }

  static async open(
    dataRoot: string,
    sessionId: string,
    options: SegmentJournalOptions = {}
  ): Promise<SegmentJournal> {
    const normalizedOptions: NormalizedSegmentJournalOptions = {
      maxSegmentBytes: options.maxSegmentBytes ?? SEGMENT_BYTES,
      compressSealed: options.compressSealed ?? true,
      compressor: options.compressor ?? defaultJournalCompressor,
      writeFrame: options.writeFrame ?? writeEntireFrame
    }
    if (normalizedOptions.maxSegmentBytes < 128) {
      throw new Error('maxSegmentBytes must be at least 128 bytes')
    }

    const directory = join(dataRoot, 'journal', sessionId)
    await mkdir(directory, { recursive: true, mode: 0o700 })
    await chmod(directory, 0o700)
    const entries = await readdir(directory)
    const existingSegments = await describeExistingSegments(directory, entries)
    const activeSegment = chooseActiveRawSegment(existingSegments)
    const maximumIndex = existingSegments.map(({ index }) => index).sort((left, right) => left - right).at(-1)
    const segmentIndex = activeSegment?.index ?? (maximumIndex === undefined ? 1 : maximumIndex + 1)
    const path = activeSegment?.path ?? segmentPath(directory, segmentIndex)

    let size = 0
    let lastSequence = 0
    let activeFrames: DecodedJournalFrame[] = []
    if (activeSegment !== undefined) {
      const repair = await repairSegmentTail(path)
      size = (await stat(path)).size
      lastSequence = repair.lastSequence
      activeFrames = await readSegmentFrames(path)
    }

    const handle = await open(path, 'a+', 0o600)
    await chmod(path, 0o600)
    if (size === 0) {
      await handle.write(MAGIC)
      size = MAGIC.byteLength
    }
    const tailIndexPath = join(directory, 'tail-index.json')
    const tailIndex = await restoreTailIndex(
      tailIndexPath,
      directory,
      activeFrames,
      segmentIndex
    )
    return new SegmentJournal(
      directory,
      handle,
      segmentIndex,
      path,
      size,
      Math.max(lastSequence, tailIndex.snapshot(segmentIndex).lastSequence),
      existingSegments
        .filter((segment) => segment !== activeSegment)
        .map((segment) => segment.state === 'active'
          ? { ...segment, state: 'sealed-raw' as const }
          : segment),
      tailIndex,
      tailIndexPath,
      normalizedOptions
    )
  }

  appendOutput(sequence: number, data: Uint8Array): Promise<void> {
    return this.#append({ kind: 'output', sequence, timestamp: Date.now(), dataLength: data.byteLength }, data)
  }

  appendResize(sequence: number, cols: number, rows: number): Promise<void> {
    return this.#append({ kind: 'resize', sequence, timestamp: Date.now(), dataLength: 0, cols, rows })
  }

  appendReset(sequence: number, screenEpoch: number): Promise<void> {
    return this.#append({ kind: 'reset', sequence, timestamp: Date.now(), dataLength: 0, screenEpoch })
  }

  appendEncoding(sequence: number, encoding: string): Promise<void> {
    return this.#append({ kind: 'encoding', sequence, timestamp: Date.now(), dataLength: 0, encoding })
  }

  appendDomainCursor(sequence: number, domainEventSequence: number): Promise<void> {
    return this.#append({ kind: 'domain-cursor', sequence, timestamp: Date.now(), dataLength: 0, domainEventSequence })
  }

  appendExit(sequence: number, exitCode: number, signal?: number): Promise<void> {
    return this.#append({
      kind: 'exit',
      sequence,
      timestamp: Date.now(),
      dataLength: 0,
      exitCode,
      ...(signal === undefined ? {} : { signal })
    })
  }

  async flush(): Promise<void> {
    this.#assertOpen()
    await this.#handle.sync()
  }

  async readFrames(): Promise<DecodedJournalFrame[]> {
    return readSessionFramesFromDirectory(this.directory)
  }

  async close(): Promise<void> {
    if (this.#closed) return
    this.#closed = true
    let storageError: unknown
    try {
      await this.#handle.sync()
    } catch (error) {
      storageError = error
    }
    try {
      await this.#handle.close()
    } catch (error) {
      storageError ??= error
    }
    this.#scheduleTailIndexWrite()
    await this.#tailIndexWrite
    if (storageError !== undefined) throw storageError
  }

  compressionCandidates(
    checkpointProtectedIndexes: ReadonlySet<number> = new Set()
  ): SegmentDescriptor[] {
    const descriptors: SegmentDescriptor[] = [
      ...this.#sealedSegments.map((segment) => ({
        ...segment,
        ...(checkpointProtectedIndexes.has(segment.index) ? { checkpointProtected: true } : {})
      })),
      {
        index: this.#segmentIndex,
        path: this.#path,
        bytes: this.#size,
        state: 'active'
      }
    ]
    return selectCompressionCandidates(descriptors)
  }

  async #append(
    header: StoredHeader,
    data: Uint8Array<ArrayBufferLike> = new Uint8Array()
  ): Promise<void> {
    this.#assertOpen()
    if (!Number.isSafeInteger(header.sequence) || header.sequence <= this.#lastSequence) {
      throw new Error('journal sequence must increase monotonically')
    }
    const encoded = encodeFrame(header, data)
    if (this.#size > MAGIC.byteLength && this.#size + encoded.byteLength > this.#maxSegmentBytes) {
      await this.#rotate()
    }
    try {
      await this.#writeFrame(this.#handle, encoded)
    } catch (error) {
      // A failed write may still have appended a prefix. Restore the exact
      // committed boundary before returning so a live durability retry can
      // reuse the same sequence without leaving a corrupt frame in front of it.
      try {
        await this.#handle.truncate(this.#size)
      } catch {
        // Preserve the original storage error; startup tail repair remains the
        // final recovery boundary if the filesystem also rejects truncation.
      }
      throw error
    }
    this.#size += encoded.byteLength
    this.#lastSequence = header.sequence
    this.#tailIndex.record(
      header.sequence,
      header.kind === 'output' ? data : EMPTY_BYTES,
      header.kind === 'domain-cursor' ? header.domainEventSequence : undefined
    )
    if (this.#tailIndex.snapshot(this.#segmentIndex).framesRecorded % 256 === 0) {
      this.#scheduleTailIndexWrite()
    }
  }

  async #rotate(): Promise<void> {
    const sealedHandle = this.#handle
    const sealedSegment = {
      index: this.#segmentIndex,
      path: this.#path,
      bytes: this.#size,
      state: 'sealed-raw' as const
    }
    await sealedHandle.sync()
    this.#scheduleTailIndexWrite()
    await this.#tailIndexWrite

    const nextSegmentIndex = this.#segmentIndex + 1
    const nextPath = segmentPath(this.directory, nextSegmentIndex)
    let nextHandle: FileHandle | undefined
    try {
      nextHandle = await open(nextPath, 'a+', 0o600)
      await nextHandle.truncate(0)
      await writeEntireFrame(nextHandle, MAGIC)
      await nextHandle.sync()
      await chmod(nextPath, 0o600)
    } catch (error) {
      await nextHandle?.close().catch(() => undefined)
      await rm(nextPath, { force: true }).catch(() => undefined)
      throw error
    }

    let closeError: unknown
    try {
      await sealedHandle.close()
    } catch (error) {
      closeError = error
    }
    this.#sealedSegments.push(sealedSegment)
    this.#segmentIndex = nextSegmentIndex
    this.#path = nextPath
    this.#handle = nextHandle
    this.#size = MAGIC.byteLength
    this.#scheduleSealedCompression()
    if (closeError !== undefined) throw closeError
  }

  #scheduleSealedCompression(): void {
    if (!this.#compressSealed) return
    for (const segment of this.#sealedSegments) {
      if (segment.state !== 'sealed-raw') continue
      const candidate = {
        sessionId: this.directory.split(/[/\\]/).at(-1)!,
        index: segment.index,
        path: segment.path
      }
      void this.#compressor.schedule(candidate).then((result) => {
        for (const current of this.#sealedSegments.filter(({ index }) => index === result.index)) {
          current.path = result.path
          current.state = 'compressed'
        }
      }).catch(() => undefined)
    }
  }

  #assertOpen(): void {
    if (this.#closed) throw new Error('journal is closed')
  }

  #scheduleTailIndexWrite(): void {
    const snapshot = this.#tailIndex.snapshot(this.#segmentIndex)
    this.#tailIndexWrite = this.#tailIndexWrite.then(
      () => writeJournalTailIndex(this.#tailIndexPath, snapshot),
      () => writeJournalTailIndex(this.#tailIndexPath, snapshot)
    ).catch(() => undefined)
  }
}

async function writeEntireFrame(handle: FileHandle, encoded: Buffer): Promise<void> {
  let offset = 0
  while (offset < encoded.byteLength) {
    const { bytesWritten } = await handle.write(encoded, offset, encoded.byteLength - offset)
    if (bytesWritten <= 0) {
      throw Object.assign(new Error('journal frame write made no progress'), { code: 'EIO' })
    }
    offset += bytesWritten
  }
}

export async function readSegmentFrames(path: string): Promise<DecodedJournalFrame[]> {
  const frames: DecodedJournalFrame[] = []
  for await (const frame of iterateSegmentFrames(path)) frames.push(frame)
  return frames
}

export async function readSessionFrames(dataRoot: string, sessionId: string): Promise<DecodedJournalFrame[]> {
  return readSessionFramesFromDirectory(join(dataRoot, 'journal', sessionId))
}

export async function repairSegmentTail(path: string): Promise<TailRepairResult> {
  const contents = await readFile(path)
  try {
    const parsed = parseSegment(contents, true)
    const truncatedBytes = contents.byteLength - parsed.lastGoodOffset
    if (truncatedBytes > 0) await truncate(path, parsed.lastGoodOffset)
    return { truncatedBytes, lastSequence: parsed.frames.at(-1)?.sequence ?? 0 }
  } catch (error) {
    if (!(error instanceof JournalCorruptionError)) throw error
    const quarantinePath = `${path}.corrupt-${Date.now()}`
    await rename(path, quarantinePath)
    throw new JournalCorruptionError(error.message, error.offset, quarantinePath)
  }
}

async function readSessionFramesFromDirectory(directory: string): Promise<DecodedJournalFrame[]> {
  const frames: DecodedJournalFrame[] = []
  for await (const frame of iterateSessionFramesFromDirectory(directory)) frames.push(frame)
  return frames
}

async function* iterateSessionFramesFromDirectory(
  directory: string
): AsyncGenerator<DecodedJournalFrame> {
  const descriptors = await describeExistingSegments(directory, await readdir(directory))
  let previousSequence = 0
  for (const group of selectReadableSegmentGroups(descriptors)) {
    const frames = await readPreferredSegmentFrames(group.paths)
    for (const frame of frames) {
      if (frame.sequence <= previousSequence) {
        throw new JournalCorruptionError('non-monotonic sequence across journal segments', 0)
      }
      previousSequence = frame.sequence
      yield frame
    }
  }
}

async function restoreTailIndex(
  path: string,
  directory: string,
  activeFrames: DecodedJournalFrame[],
  activeSegmentIndex: number
): Promise<JournalTailIndex> {
  let index: JournalTailIndex
  try {
    const snapshot = await loadJournalTailIndex(path)
    if (
      snapshot.activeSegmentIndex < 1 ||
      snapshot.activeSegmentIndex > activeSegmentIndex ||
      (
        snapshot.activeSegmentIndex === activeSegmentIndex &&
        snapshot.lastSequence > (activeFrames.at(-1)?.sequence ?? 0)
      )
    ) {
      throw new Error('tail index is ahead of the Journal')
    }
    index = JournalTailIndex.fromSnapshot(snapshot)
  } catch {
    index = new JournalTailIndex()
    for await (const frame of iterateSessionFramesFromDirectory(directory)) {
      recordTailFrame(index, frame)
    }
    await writeJournalTailIndex(path, index.snapshot(activeSegmentIndex))
    return index
  }
  const throughSequence = index.snapshot().lastSequence
  const groups = selectReadableSegmentGroups(
    await describeExistingSegments(directory, await readdir(directory))
  ).filter(({ index: segmentIndex }) =>
    segmentIndex >= index.snapshot().activeSegmentIndex &&
    segmentIndex <= activeSegmentIndex
  )
  for (const group of groups) {
    const frames = group.index === activeSegmentIndex
      ? activeFrames
      : await readPreferredSegmentFrames(group.paths)
    for (const frame of frames) {
      if (frame.sequence > throughSequence) recordTailFrame(index, frame)
    }
  }
  return index
}

function recordTailFrame(index: JournalTailIndex, frame: DecodedJournalFrame): void {
  index.record(
    frame.sequence,
    frame.kind === 'output' ? frame.data : EMPTY_BYTES,
    frame.kind === 'domain-cursor' ? frame.domainEventSequence : undefined
  )
}

const EMPTY_BYTES = new Uint8Array()

function encodeFrame(header: StoredHeader, data: Uint8Array<ArrayBufferLike>): Buffer {
  const encodedHeader = Buffer.from(JSON.stringify(header), 'utf8')
  const body = Buffer.allocUnsafe(4 + encodedHeader.byteLength + data.byteLength)
  body.writeUInt32BE(encodedHeader.byteLength, 0)
  encodedHeader.copy(body, 4)
  Buffer.from(data).copy(body, 4 + encodedHeader.byteLength)
  const prefix = Buffer.allocUnsafe(FRAME_PREFIX_BYTES)
  prefix.writeUInt32BE(body.byteLength, 0)
  prefix.writeUInt32BE(crc32(body), 4)
  return Buffer.concat([prefix, body])
}

async function* iterateSegmentFrames(path: string): AsyncGenerator<DecodedJournalFrame> {
  const raw = createReadStream(path, { highWaterMark: 64 * 1024 })
  const source = path.endsWith('.gz') ? raw.pipe(createGunzip()) : raw
  let pending = Buffer.alloc(0)
  let offset = 0
  let previousSequence = 0
  let magicRead = false

  for await (const value of source) {
    const chunk = Buffer.isBuffer(value) ? value : Buffer.from(value)
    pending = pending.byteLength === 0 ? chunk : Buffer.concat([pending, chunk])
    if (!magicRead) {
      if (pending.byteLength < MAGIC.byteLength) continue
      if (!pending.subarray(0, MAGIC.byteLength).equals(MAGIC)) {
        throw new JournalCorruptionError('invalid Journal V2 magic', 0)
      }
      pending = pending.subarray(MAGIC.byteLength)
      offset = MAGIC.byteLength
      magicRead = true
    }

    while (pending.byteLength >= FRAME_PREFIX_BYTES) {
      const bodyLength = pending.readUInt32BE(0)
      const expectedChecksum = pending.readUInt32BE(4)
      if (bodyLength < 4 || bodyLength > MAX_FRAME_BYTES) {
        throw new JournalCorruptionError('invalid journal frame length', offset)
      }
      const encodedLength = FRAME_PREFIX_BYTES + bodyLength
      if (pending.byteLength < encodedLength) break
      const body = pending.subarray(FRAME_PREFIX_BYTES, encodedLength)
      if (crc32(body) !== expectedChecksum) {
        throw new JournalCorruptionError('journal frame checksum mismatch', offset)
      }
      const headerLength = body.readUInt32BE(0)
      if (headerLength < 2 || 4 + headerLength > body.byteLength) {
        throw new JournalCorruptionError('invalid journal frame header length', offset)
      }
      let header: StoredHeader
      try {
        header = JSON.parse(body.subarray(4, 4 + headerLength).toString('utf8')) as StoredHeader
      } catch {
        throw new JournalCorruptionError('invalid journal frame header JSON', offset)
      }
      const data = body.subarray(4 + headerLength)
      validateHeader(header, data.byteLength, previousSequence, offset)
      previousSequence = header.sequence
      yield decodeFrame(header, data)
      pending = pending.subarray(encodedLength)
      offset += encodedLength
    }
  }

  if (!magicRead) throw new JournalCorruptionError('invalid Journal V2 magic', 0)
  if (pending.byteLength > 0) {
    throw new JournalCorruptionError(
      pending.byteLength < FRAME_PREFIX_BYTES
        ? 'truncated journal frame prefix'
        : 'truncated journal frame body',
      offset
    )
  }
}

function parseSegment(contents: Buffer, tolerateTornTail = false): { frames: DecodedJournalFrame[]; lastGoodOffset: number } {
  if (contents.byteLength < MAGIC.byteLength || !contents.subarray(0, MAGIC.byteLength).equals(MAGIC)) {
    throw new JournalCorruptionError('invalid Journal V2 magic', 0)
  }
  const frames: DecodedJournalFrame[] = []
  let offset = MAGIC.byteLength
  let lastGoodOffset = offset
  let previousSequence = 0

  while (offset < contents.byteLength) {
    if (offset + FRAME_PREFIX_BYTES > contents.byteLength) {
      if (tolerateTornTail) break
      throw new JournalCorruptionError('truncated journal frame prefix', offset)
    }
    const bodyLength = contents.readUInt32BE(offset)
    const expectedChecksum = contents.readUInt32BE(offset + 4)
    if (bodyLength < 4 || bodyLength > MAX_FRAME_BYTES) {
      throw new JournalCorruptionError('invalid journal frame length', offset)
    }
    const bodyStart = offset + FRAME_PREFIX_BYTES
    const bodyEnd = bodyStart + bodyLength
    if (bodyEnd > contents.byteLength) {
      if (tolerateTornTail) break
      throw new JournalCorruptionError('truncated journal frame body', offset)
    }
    const body = contents.subarray(bodyStart, bodyEnd)
    if (crc32(body) !== expectedChecksum) {
      if (tolerateTornTail && bodyEnd === contents.byteLength) break
      throw new JournalCorruptionError('journal frame checksum mismatch', offset)
    }
    const headerLength = body.readUInt32BE(0)
    if (headerLength < 2 || 4 + headerLength > body.byteLength) {
      throw new JournalCorruptionError('invalid journal frame header length', offset)
    }
    let header: StoredHeader
    try {
      header = JSON.parse(body.subarray(4, 4 + headerLength).toString('utf8')) as StoredHeader
    } catch {
      throw new JournalCorruptionError('invalid journal frame header JSON', offset)
    }
    const data = body.subarray(4 + headerLength)
    validateHeader(header, data.byteLength, previousSequence, offset)
    frames.push(decodeFrame(header, data))
    previousSequence = header.sequence
    offset = bodyEnd
    lastGoodOffset = offset
  }
  return { frames, lastGoodOffset }
}

function validateHeader(header: StoredHeader, actualDataLength: number, previousSequence: number, offset: number): void {
  if (!Number.isSafeInteger(header.sequence) || header.sequence <= previousSequence) {
    throw new JournalCorruptionError('non-monotonic journal sequence', offset)
  }
  if (header.dataLength !== actualDataLength) {
    throw new JournalCorruptionError('journal payload length mismatch', offset)
  }
  if (header.kind !== 'output' && actualDataLength !== 0) {
    throw new JournalCorruptionError('control frame contains an unexpected payload', offset)
  }
}

function decodeFrame(header: StoredHeader, data: Buffer): DecodedJournalFrame {
  switch (header.kind) {
    case 'output': return { kind: 'output', sequence: header.sequence, data: Uint8Array.from(data) }
    case 'resize': return { kind: 'resize', sequence: header.sequence, cols: header.cols, rows: header.rows }
    case 'reset': return { kind: 'reset', sequence: header.sequence, screenEpoch: header.screenEpoch }
    case 'encoding': return { kind: 'encoding', sequence: header.sequence, encoding: header.encoding }
    case 'domain-cursor': return { kind: 'domain-cursor', sequence: header.sequence, domainEventSequence: header.domainEventSequence }
    case 'exit': return { kind: 'exit', sequence: header.sequence, exitCode: header.exitCode, ...(header.signal === undefined ? {} : { signal: header.signal }) }
  }
}

function segmentPath(directory: string, index: number): string {
  return join(directory, `segment-${String(index).padStart(6, '0')}.mtj`)
}

interface StoredSegmentDescriptor extends SegmentDescriptor {
  format: 'mtj' | 'legacy-bin'
}

async function describeExistingSegments(
  directory: string,
  entries: string[],
  publicationRaceRetries = 4
): Promise<StoredSegmentDescriptor[]> {
  let publicationRace = false
  const descriptors = await Promise.all(entries.map(async (
    entry
  ): Promise<StoredSegmentDescriptor | undefined> => {
    const parsed = parseSegmentName(entry)
    if (!parsed) return undefined
    const path = join(directory, entry)
    let bytes: number
    try {
      bytes = (await stat(path)).size
    } catch (error) {
      if (!(error instanceof Error && 'code' in error && error.code === 'ENOENT')) throw error
      publicationRace = true
      return undefined
    }
    return {
      index: parsed.index,
      path,
      bytes,
      state: parsed.compressed ? 'compressed' as const : 'active' as const,
      format: parsed.format
    }
  }))
  if (publicationRace && publicationRaceRetries > 0) {
    return describeExistingSegments(
      directory,
      await readdir(directory),
      publicationRaceRetries - 1
    )
  }
  return descriptors.filter((item): item is StoredSegmentDescriptor => item !== undefined)
}

function parseSegmentName(name: string): {
  index: number
  compressed: boolean
  format: StoredSegmentDescriptor['format']
} | undefined {
  const match = /^segment-(\d{6})\.(mtj|bin)(\.gz)?$/.exec(name)
  if (!match) return undefined
  return {
    index: Number(match[1]),
    format: match[2] === 'mtj' ? 'mtj' : 'legacy-bin',
    compressed: match[3] === '.gz'
  }
}

function chooseActiveRawSegment(
  descriptors: StoredSegmentDescriptor[]
): StoredSegmentDescriptor | undefined {
  const compressedIndexes = new Set(
    descriptors.filter(({ state }) => state === 'compressed').map(({ index }) => index)
  )
  return descriptors
    .filter(({ state, index }) => state !== 'compressed' && !compressedIndexes.has(index))
    .sort((left, right) => right.index - left.index || formatPriority(left) - formatPriority(right))[0]
}

function selectReadableSegmentGroups(
  descriptors: StoredSegmentDescriptor[]
): Array<{ index: number; paths: string[] }> {
  const grouped = new Map<number, StoredSegmentDescriptor[]>()
  for (const descriptor of descriptors) {
    const values = grouped.get(descriptor.index) ?? []
    values.push(descriptor)
    grouped.set(descriptor.index, values)
  }
  return [...grouped.entries()]
    .sort(([left], [right]) => left - right)
    .map(([index, values]) => ({
      index,
      paths: values.sort((left, right) => readPriority(left) - readPriority(right))
        .map(({ path }) => path)
    }))
}

async function readPreferredSegmentFrames(paths: string[]): Promise<DecodedJournalFrame[]> {
  let failure: unknown
  for (const path of paths) {
    try {
      return await readSegmentFrames(path)
    } catch (error) {
      failure = error
    }
  }
  throw failure ?? new JournalCorruptionError('Journal segment is unavailable', 0)
}

function readPriority(segment: StoredSegmentDescriptor): number {
  if (segment.state !== 'compressed' && segment.format === 'mtj') return 0
  if (segment.state !== 'compressed') return 1
  if (segment.format === 'mtj') return 2
  return 3
}

function formatPriority(segment: StoredSegmentDescriptor): number {
  return segment.format === 'mtj' ? 0 : 1
}

function crc32(buffer: Buffer): number {
  let crc = 0xffffffff
  for (const byte of buffer) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1))
    }
  }
  return (crc ^ 0xffffffff) >>> 0
}
