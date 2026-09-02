import { mkdir, open, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'

const VERSION = 1 as const
const SPARSE_FRAME_INTERVAL = 256
const DEFAULT_MAX_TRACKED_LINES = 10_000

export interface JournalTailIndexSparseEntry {
  sequence: number
  completedLineCount: number
}

export interface JournalDomainCursorEntry {
  sequence: number
  domainEventSequence: number
}

export interface JournalTailIndexSnapshot {
  version: typeof VERSION
  firstSequence: number
  lastSequence: number
  completedLineCount: number
  framesRecorded: number
  maxTrackedLines: number
  lineStartSequences: number[]
  sparse: JournalTailIndexSparseEntry[]
  pendingLineStart: boolean
  alternateScreen: boolean
  escapeBytes: number[]
  activeSegmentIndex: number
  domainCursors: JournalDomainCursorEntry[]
}

export class JournalTailIndex {
  readonly #maxTrackedLines: number
  #firstSequence = 0
  #lastSequence = 0
  #completedLineCount = 0
  #framesRecorded = 0
  #lineStartSequences: number[] = []
  #sparse: JournalTailIndexSparseEntry[] = []
  #pendingLineStart = false
  #alternateScreen = false
  #escapeBytes: number[] = []
  #domainCursors: JournalDomainCursorEntry[] = []

  constructor(maxTrackedLines = DEFAULT_MAX_TRACKED_LINES, snapshot?: JournalTailIndexSnapshot) {
    if (!Number.isSafeInteger(maxTrackedLines) || maxTrackedLines < 1) {
      throw new Error('maxTrackedLines must be a positive integer')
    }
    this.#maxTrackedLines = maxTrackedLines
    if (snapshot) this.#restore(snapshot)
  }

  record(
    sequence: number,
    bytes: Uint8Array<ArrayBufferLike>,
    domainEventSequence?: number
  ): void {
    if (!Number.isSafeInteger(sequence) || sequence <= this.#lastSequence) {
      throw new Error('tail index sequence must increase monotonically')
    }
    if (this.#firstSequence === 0) this.#firstSequence = sequence
    this.#lastSequence = sequence
    this.#framesRecorded += 1
    if (domainEventSequence !== undefined) {
      if (!Number.isSafeInteger(domainEventSequence) || domainEventSequence < 0) {
        throw new Error('domainEventSequence must be a non-negative integer')
      }
      const previous = this.#domainCursors.at(-1)
      if (previous && domainEventSequence < previous.domainEventSequence) {
        throw new Error('domain event sequence must not move backwards')
      }
      this.#domainCursors.push({ sequence, domainEventSequence })
    }

    for (const byte of bytes) this.#recordByte(sequence, byte)
    if (this.#framesRecorded % SPARSE_FRAME_INTERVAL === 0) {
      this.#sparse.push({ sequence, completedLineCount: this.#completedLineCount })
    }
  }

  tailStart(maxLines = DEFAULT_MAX_TRACKED_LINES): number {
    if (!Number.isSafeInteger(maxLines) || maxLines < 1 || maxLines > this.#maxTrackedLines) {
      throw new Error(`maxLines must be between 1 and ${this.#maxTrackedLines}`)
    }
    if (this.#firstSequence === 0) return 0
    if (this.#lineStartSequences.length <= maxLines) return this.#firstSequence
    return this.#lineStartSequences[this.#lineStartSequences.length - maxLines]!
  }

  domainEventSequenceAtOrBefore(sequence: number): number {
    let low = 0
    let high = this.#domainCursors.length - 1
    let result = 0
    while (low <= high) {
      const middle = Math.floor((low + high) / 2)
      const entry = this.#domainCursors[middle]!
      if (entry.sequence <= sequence) {
        result = entry.domainEventSequence
        low = middle + 1
      } else {
        high = middle - 1
      }
    }
    return result
  }

  snapshot(activeSegmentIndex = 0): JournalTailIndexSnapshot {
    return {
      version: VERSION,
      firstSequence: this.#firstSequence,
      lastSequence: this.#lastSequence,
      completedLineCount: this.#completedLineCount,
      framesRecorded: this.#framesRecorded,
      maxTrackedLines: this.#maxTrackedLines,
      lineStartSequences: [...this.#lineStartSequences],
      sparse: this.#sparse.map((entry) => ({ ...entry })),
      pendingLineStart: this.#pendingLineStart,
      alternateScreen: this.#alternateScreen,
      escapeBytes: [...this.#escapeBytes],
      activeSegmentIndex,
      domainCursors: this.#domainCursors.map((entry) => ({ ...entry }))
    }
  }

  static fromSnapshot(snapshot: JournalTailIndexSnapshot): JournalTailIndex {
    validateSnapshot(snapshot)
    return new JournalTailIndex(snapshot.maxTrackedLines, snapshot)
  }

  #recordByte(sequence: number, byte: number): void {
    if (this.#escapeBytes.length > 0) {
      if (isControlStringPrefix(this.#escapeBytes)) {
        const introducer = this.#escapeBytes[1]!
        if (introducer === 0x5d && byte === 0x07) {
          this.#escapeBytes = []
          return
        }
        if (this.#escapeBytes.at(-1) === 0x1b && byte === 0x5c) {
          this.#escapeBytes = []
          return
        }
        this.#escapeBytes = byte === 0x1b
          ? [0x1b, introducer, 0x1b]
          : [0x1b, introducer]
        return
      }
      this.#escapeBytes.push(byte)
      if (escapeSequenceComplete(this.#escapeBytes)) {
        this.#applyEscapeSequence()
        this.#escapeBytes = []
      } else if (this.#escapeBytes.length > 256) {
        this.#escapeBytes = []
      }
      return
    }
    if (byte === 0x1b) {
      this.#escapeBytes = [byte]
      return
    }
    if (this.#alternateScreen) return

    if (this.#pendingLineStart) {
      this.#pushLineStart(sequence)
      this.#pendingLineStart = false
    } else if (this.#lineStartSequences.length === 0) {
      this.#pushLineStart(sequence)
    }
    if (byte === 0x0a) {
      this.#completedLineCount += 1
      this.#pendingLineStart = true
    }
  }

  #applyEscapeSequence(): void {
    const value = Buffer.from(this.#escapeBytes).toString('ascii')
    if (/^\x1b\[\?(?:47|1047|1049)h$/.test(value)) this.#alternateScreen = true
    if (/^\x1b\[\?(?:47|1047|1049)l$/.test(value)) this.#alternateScreen = false
  }

  #pushLineStart(sequence: number): void {
    this.#lineStartSequences.push(sequence)
    const limit = this.#maxTrackedLines + 1
    if (this.#lineStartSequences.length > limit) {
      this.#lineStartSequences.splice(0, this.#lineStartSequences.length - limit)
    }
  }

  #restore(snapshot: JournalTailIndexSnapshot): void {
    this.#firstSequence = snapshot.firstSequence
    this.#lastSequence = snapshot.lastSequence
    this.#completedLineCount = snapshot.completedLineCount
    this.#framesRecorded = snapshot.framesRecorded
    this.#lineStartSequences = [...snapshot.lineStartSequences]
    this.#sparse = snapshot.sparse.map((entry) => ({ ...entry }))
    this.#pendingLineStart = snapshot.pendingLineStart
    this.#alternateScreen = snapshot.alternateScreen
    this.#escapeBytes = [...snapshot.escapeBytes]
    this.#domainCursors = snapshot.domainCursors.map((entry) => ({ ...entry }))
  }
}

export async function writeJournalTailIndex(
  path: string,
  snapshot: JournalTailIndexSnapshot
): Promise<void> {
  validateSnapshot(snapshot)
  await mkdir(dirname(path), { recursive: true, mode: 0o700 })
  const temporaryPath = `${path}.tmp-${process.pid}-${Date.now()}`
  await writeFile(temporaryPath, `${JSON.stringify(snapshot)}\n`, { mode: 0o600 })
  const handle = await open(temporaryPath, 'r')
  await handle.sync()
  await handle.close()
  await rename(temporaryPath, path)
  await syncDirectory(dirname(path))
}

export async function loadJournalTailIndex(path: string): Promise<JournalTailIndexSnapshot> {
  try {
    const parsed = JSON.parse(await readFile(path, 'utf8')) as unknown
    validateSnapshot(parsed)
    return parsed
  } catch (error) {
    throw new Error('invalid Journal tail index', { cause: error })
  }
}

function escapeSequenceComplete(bytes: number[]): boolean {
  if (bytes.length < 2) return false
  if (bytes[1] === 0x5b) {
    return bytes.length >= 3 && bytes.at(-1)! >= 0x40 && bytes.at(-1)! <= 0x7e
  }
  if (isControlStringPrefix(bytes)) return false
  return true
}

function isControlStringPrefix(bytes: number[]): boolean {
  return bytes.length >= 2 && (
    bytes[1] === 0x5d || // OSC
    bytes[1] === 0x50 || // DCS
    bytes[1] === 0x58 || // SOS
    bytes[1] === 0x5e || // PM
    bytes[1] === 0x5f    // APC
  )
}

function validateSnapshot(value: unknown): asserts value is JournalTailIndexSnapshot {
  if (!value || typeof value !== 'object') throw new Error('snapshot must be an object')
  const snapshot = value as Partial<JournalTailIndexSnapshot>
  const integers = [
    snapshot.firstSequence,
    snapshot.lastSequence,
    snapshot.completedLineCount,
    snapshot.framesRecorded,
    snapshot.maxTrackedLines,
    snapshot.activeSegmentIndex
  ]
  const lineStarts = snapshot.lineStartSequences
  const sparse = snapshot.sparse
  const domainCursors = snapshot.domainCursors
  const empty = snapshot.firstSequence === 0 && snapshot.lastSequence === 0
  if (
    snapshot.version !== VERSION ||
    integers.some((item) => !Number.isSafeInteger(item) || item! < 0) ||
    snapshot.maxTrackedLines! < 1 ||
    snapshot.firstSequence! > snapshot.lastSequence! ||
    (empty ? snapshot.framesRecorded !== 0 : snapshot.firstSequence === 0 || snapshot.framesRecorded === 0) ||
    !Array.isArray(lineStarts) ||
    lineStarts.some((item) => !Number.isSafeInteger(item) || item < snapshot.firstSequence! ||
      item > snapshot.lastSequence!) ||
    lineStarts.some((item, index) => index > 0 && item < lineStarts[index - 1]!) ||
    lineStarts.length > snapshot.maxTrackedLines! + 1 ||
    !Array.isArray(sparse) ||
    sparse.some((item) => !item || !Number.isSafeInteger(item.sequence) ||
      item.sequence < 1 || !Number.isSafeInteger(item.completedLineCount) || item.completedLineCount < 0) ||
    sparse.some((item, index) => item.sequence < snapshot.firstSequence! ||
      item.sequence > snapshot.lastSequence! ||
      item.completedLineCount > snapshot.completedLineCount! ||
      (index > 0 && (item.sequence <= sparse[index - 1]!.sequence ||
        item.completedLineCount < sparse[index - 1]!.completedLineCount))) ||
    !Array.isArray(domainCursors) ||
    domainCursors.some((item) => !item || !Number.isSafeInteger(item.sequence) ||
      item.sequence < snapshot.firstSequence! || item.sequence > snapshot.lastSequence! ||
      !Number.isSafeInteger(item.domainEventSequence) || item.domainEventSequence < 0) ||
    domainCursors.some((item, index) => index > 0 && (
      item.sequence <= domainCursors[index - 1]!.sequence ||
      item.domainEventSequence < domainCursors[index - 1]!.domainEventSequence
    )) ||
    typeof snapshot.pendingLineStart !== 'boolean' ||
    typeof snapshot.alternateScreen !== 'boolean' ||
    !Array.isArray(snapshot.escapeBytes) ||
    snapshot.escapeBytes.length > 256 ||
    snapshot.escapeBytes.some((item) => !Number.isInteger(item) || item < 0 || item > 255) ||
    (snapshot.escapeBytes.length > 0 && snapshot.escapeBytes[0] !== 0x1b)
  ) {
    throw new Error('snapshot fields are invalid')
  }
}

async function syncDirectory(path: string): Promise<void> {
  try {
    const handle = await open(path, 'r')
    await handle.sync()
    await handle.close()
  } catch (error) {
    if (process.platform !== 'win32') throw error
  }
}
