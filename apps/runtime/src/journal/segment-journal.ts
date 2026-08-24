import { gzipSync, gunzipSync } from 'node:zlib'
import {
  chmod,
  mkdir,
  open,
  readFile,
  readdir,
  rename,
  truncate,
  unlink,
  writeFile
} from 'node:fs/promises'
import type { FileHandle } from 'node:fs/promises'
import { join } from 'node:path'

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
  /** Fault-injectable frame boundary; production callers use the default durable file writer. */
  writeFrame?: (handle: FileHandle, encoded: Buffer) => Promise<void>
}

type NormalizedSegmentJournalOptions = {
  maxSegmentBytes: number
  compressSealed: boolean
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
  readonly #compressSealed: boolean
  readonly #writeFrame: NormalizedSegmentJournalOptions['writeFrame']
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
    options: NormalizedSegmentJournalOptions
  ) {
    this.directory = directory
    this.#handle = handle
    this.#segmentIndex = segmentIndex
    this.#path = path
    this.#size = size
    this.#lastSequence = lastSequence
    this.#maxSegmentBytes = options.maxSegmentBytes
    this.#compressSealed = options.compressSealed
    this.#writeFrame = options.writeFrame
  }

  get path(): string {
    return this.#path
  }

  get lastSequence(): number {
    return this.#lastSequence
  }

  static async open(
    dataRoot: string,
    sessionId: string,
    options: SegmentJournalOptions = {}
  ): Promise<SegmentJournal> {
    const normalizedOptions: NormalizedSegmentJournalOptions = {
      maxSegmentBytes: options.maxSegmentBytes ?? 16 * 1024 * 1024,
      compressSealed: options.compressSealed ?? true,
      writeFrame: options.writeFrame ?? (async (handle, encoded) => {
        await handle.write(encoded)
      })
    }
    if (normalizedOptions.maxSegmentBytes < 128) {
      throw new Error('maxSegmentBytes must be at least 128 bytes')
    }

    const directory = join(dataRoot, 'journal', sessionId)
    await mkdir(directory, { recursive: true, mode: 0o700 })
    await chmod(directory, 0o700)
    const entries = await readdir(directory)
    const active = entries
      .map(parseActiveSegmentIndex)
      .filter((value): value is number => value !== undefined)
      .sort((left, right) => left - right)
      .at(-1)
    const sealedMaximum = entries
      .map(parseSealedSegmentIndex)
      .filter((value): value is number => value !== undefined)
      .sort((left, right) => left - right)
      .at(-1)
    const segmentIndex = active ?? (sealedMaximum === undefined ? 1 : sealedMaximum + 1)
    const path = segmentPath(directory, segmentIndex)

    let size = 0
    let lastSequence = 0
    if (active !== undefined) {
      const repair = await repairSegmentTail(path)
      size = (await readFile(path)).byteLength
      lastSequence = repair.lastSequence
    }

    const handle = await open(path, 'a+', 0o600)
    await chmod(path, 0o600)
    if (size === 0) {
      await handle.write(MAGIC)
      size = MAGIC.byteLength
    }
    return new SegmentJournal(
      directory,
      handle,
      segmentIndex,
      path,
      size,
      Math.max(lastSequence, await lastSequenceAcrossSealed(directory)),
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
    await this.#handle.sync()
    await this.#handle.close()
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
    await this.#writeFrame(this.#handle, encoded)
    this.#size += encoded.byteLength
    this.#lastSequence = header.sequence
  }

  async #rotate(): Promise<void> {
    await this.#handle.sync()
    await this.#handle.close()
    if (this.#compressSealed) {
      const compressedPath = `${this.#path}.gz`
      const temporaryPath = `${compressedPath}.tmp`
      await writeFile(temporaryPath, gzipSync(await readFile(this.#path)), { mode: 0o600 })
      const temporaryHandle = await open(temporaryPath, 'r')
      await temporaryHandle.sync()
      await temporaryHandle.close()
      await rename(temporaryPath, compressedPath)
      await unlink(this.#path)
    }

    this.#segmentIndex += 1
    this.#path = segmentPath(this.directory, this.#segmentIndex)
    this.#handle = await open(this.#path, 'a+', 0o600)
    await this.#handle.write(MAGIC)
    await chmod(this.#path, 0o600)
    this.#size = MAGIC.byteLength
  }

  #assertOpen(): void {
    if (this.#closed) throw new Error('journal is closed')
  }
}

export async function readSegmentFrames(path: string): Promise<DecodedJournalFrame[]> {
  const stored = await readFile(path)
  const contents = path.endsWith('.gz') ? gunzipSync(stored) : stored
  return parseSegment(contents).frames
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
  const paths = (await readdir(directory))
    .filter((entry) => /^segment-\d{6}\.bin(?:\.gz)?$/.test(entry))
    .sort()
    .map((entry) => join(directory, entry))
  const frames: DecodedJournalFrame[] = []
  let previousSequence = 0
  for (const path of paths) {
    for (const frame of await readSegmentFrames(path)) {
      if (frame.sequence <= previousSequence) {
        throw new JournalCorruptionError('non-monotonic sequence across journal segments', 0)
      }
      frames.push(frame)
      previousSequence = frame.sequence
    }
  }
  return frames
}

async function lastSequenceAcrossSealed(directory: string): Promise<number> {
  const frames = await readSessionFramesFromDirectory(directory)
  return frames.at(-1)?.sequence ?? 0
}

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
  return join(directory, `segment-${String(index).padStart(6, '0')}.bin`)
}

function parseActiveSegmentIndex(name: string): number | undefined {
  const match = /^segment-(\d{6})\.bin$/.exec(name)
  return match ? Number(match[1]) : undefined
}

function parseSealedSegmentIndex(name: string): number | undefined {
  const match = /^segment-(\d{6})\.bin\.gz$/.exec(name)
  return match ? Number(match[1]) : undefined
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
