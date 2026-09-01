import { createHash, randomUUID } from 'node:crypto'
import {
  chmod,
  mkdir,
  open,
  readFile,
  readdir,
  rename,
  unlink,
  writeFile
} from 'node:fs/promises'
import { join } from 'node:path'

import type { RuntimeDatabase } from '../storage/database'

const MAGIC = Buffer.from('MTCPV1\n', 'ascii')
export const MAX_CHECKPOINT_SNAPSHOT_BYTES = 16 * 1024 * 1024

export type CheckpointFaultPhase =
  | 'after-file-sync'
  | 'after-file-rename'
  | 'after-index-commit'

export interface CreateCheckpointInput {
  sessionId: string
  terminalSequence: number
  domainEventSequence: number
  screenEpoch: number
  snapshot: Uint8Array<ArrayBufferLike>
}

export interface CreatedCheckpoint {
  id: string
  generation: number
  filePath: string
}

export interface LoadedCheckpoint {
  id: string
  generation: number
  terminalSequence: number
  domainEventSequence: number
  screenEpoch: number
  snapshot: Uint8Array
}

export interface CheckpointWatermark {
  terminalSequence: number
  domainEventSequence: number
}

interface StoredCheckpoint {
  id: string
  session_id: string
  generation: number
  terminal_sequence: number
  domain_event_sequence: number
  screen_epoch: number
  file_path: string
  checksum: string
}

interface CheckpointHeader {
  id: string
  sessionId: string
  generation: number
  terminalSequence: number
  domainEventSequence: number
  screenEpoch: number
  snapshotLength: number
}

export class CheckpointManager {
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase
  readonly #retainGenerations: number

  constructor(
    dataRoot: string,
    database: RuntimeDatabase,
    options: { retainGenerations?: number } = {}
  ) {
    this.#dataRoot = dataRoot
    this.#database = database
    this.#retainGenerations = options.retainGenerations ?? 2
    if (this.#retainGenerations < 2) {
      throw new Error('at least two checkpoint generations must be retained')
    }
  }

  create(
    input: CreateCheckpointInput,
    injectFault?: (phase: CheckpointFaultPhase) => void
  ): Promise<CreatedCheckpoint> {
    validateInput(input)
    return this.#database.enqueueWrite(async () => {
      const latest = this.#database.get<{
        terminal_sequence: number
        domain_event_sequence: number
      }>(
        `SELECT terminal_sequence, domain_event_sequence
         FROM journal_checkpoints WHERE session_id = ? AND valid = 1
         ORDER BY generation DESC LIMIT 1`,
        input.sessionId
      )
      if (
        latest &&
        (input.terminalSequence < latest.terminal_sequence ||
          input.domainEventSequence < latest.domain_event_sequence)
      ) {
        throw new Error('checkpoint watermark must not move backwards')
      }
      const generation =
        (this.#database.get<{ maximum: number }>(
          'SELECT COALESCE(MAX(generation), 0) AS maximum FROM journal_checkpoints WHERE session_id = ?',
          input.sessionId
        )?.maximum ?? 0) + 1
      const id = randomUUID()
      const directory = this.#directory(input.sessionId)
      await mkdir(directory, { recursive: true, mode: 0o700 })
      await chmod(directory, 0o700)
      const filePath = join(directory, `checkpoint-${String(generation).padStart(6, '0')}.bin`)
      const temporaryPath = `${filePath}.tmp-${id}`
      const encoded = encodeCheckpoint({
        id,
        sessionId: input.sessionId,
        generation,
        terminalSequence: input.terminalSequence,
        domainEventSequence: input.domainEventSequence,
        screenEpoch: input.screenEpoch,
        snapshotLength: input.snapshot.byteLength
      }, input.snapshot)
      await writeFile(temporaryPath, encoded, { mode: 0o600 })
      const temporaryHandle = await open(temporaryPath, 'r')
      await temporaryHandle.sync()
      await temporaryHandle.close()
      injectFault?.('after-file-sync')
      await rename(temporaryPath, filePath)
      await chmod(filePath, 0o600)
      await syncDirectory(directory)
      injectFault?.('after-file-rename')

      const checksum = digest(encoded)
      this.#database.transaction((transaction) => {
        transaction.run(
          `INSERT INTO journal_checkpoints (
             id, session_id, generation, terminal_sequence, domain_event_sequence,
             screen_epoch, file_path, checksum, created_at, valid
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)`,
          id,
          input.sessionId,
          generation,
          input.terminalSequence,
          input.domainEventSequence,
          input.screenEpoch,
          filePath,
          checksum,
          Date.now()
        )
      })
      injectFault?.('after-index-commit')
      await this.#prune(input.sessionId)
      return { id, generation, filePath }
    })
  }

  async loadLatest(
    sessionId: string,
    watermark: CheckpointWatermark
  ): Promise<LoadedCheckpoint | undefined> {
    const candidates = this.#database.all<StoredCheckpoint>(
      `SELECT id, session_id, generation, terminal_sequence, domain_event_sequence,
              screen_epoch, file_path, checksum
       FROM journal_checkpoints
       WHERE session_id = ? AND valid = 1
         AND terminal_sequence <= ? AND domain_event_sequence <= ?
       ORDER BY generation DESC`,
      sessionId,
      watermark.terminalSequence,
      watermark.domainEventSequence
    )
    for (const candidate of candidates) {
      try {
        return await this.#readCandidate(candidate)
      } catch {
        this.#invalidateUnreadableCandidate(candidate.id)
      }
    }
    return undefined
  }

  async #readCandidate(candidate: StoredCheckpoint): Promise<LoadedCheckpoint> {
    const encoded = await readFile(candidate.file_path)
    if (digest(encoded) !== candidate.checksum) {
      throw new Error('checkpoint checksum mismatch')
    }
    const decoded = decodeCheckpoint(encoded)
    assertHeaderMatchesRow(decoded.header, candidate)
    return {
      id: candidate.id,
      generation: candidate.generation,
      terminalSequence: candidate.terminal_sequence,
      domainEventSequence: candidate.domain_event_sequence,
      screenEpoch: candidate.screen_epoch,
      snapshot: decoded.snapshot
    }
  }

  #invalidateUnreadableCandidate(id: string): void {
    if (this.#database.readOnly) return
    this.#database.run('UPDATE journal_checkpoints SET valid = 0 WHERE id = ?', id)
  }

  async removeOrphans(sessionId: string): Promise<number> {
    const directory = this.#directory(sessionId)
    let entries: string[]
    try {
      entries = await readdir(directory)
    } catch {
      return 0
    }
    const indexed = new Set(
      this.#database
        .all<{ file_path: string }>(
          'SELECT file_path FROM journal_checkpoints WHERE session_id = ?',
          sessionId
        )
        .map(({ file_path }) => file_path)
    )
    let removed = 0
    for (const entry of entries) {
      const path = join(directory, entry)
      if ((entry.endsWith('.bin') && !indexed.has(path)) || entry.includes('.tmp-')) {
        await unlink(path)
        removed += 1
      }
    }
    if (removed > 0) await syncDirectory(directory)
    return removed
  }

  async #prune(sessionId: string): Promise<void> {
    const obsolete = this.#database.all<{ id: string; file_path: string }>(
      `SELECT id, file_path FROM journal_checkpoints
       WHERE session_id = ?
       ORDER BY generation DESC
       LIMIT -1 OFFSET ?`,
      sessionId,
      this.#retainGenerations
    )
    for (const checkpoint of obsolete) {
      try {
        await unlink(checkpoint.file_path)
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error
      }
      this.#database.run('DELETE FROM journal_checkpoints WHERE id = ?', checkpoint.id)
    }
    if (obsolete.length > 0) await syncDirectory(this.#directory(sessionId))
  }

  #directory(sessionId: string): string {
    return join(this.#dataRoot, 'checkpoints', sessionId)
  }
}

function encodeCheckpoint(
  header: CheckpointHeader,
  snapshot: Uint8Array<ArrayBufferLike>
): Buffer {
  const encodedHeader = Buffer.from(JSON.stringify(header), 'utf8')
  const prefix = Buffer.allocUnsafe(MAGIC.byteLength + 4)
  MAGIC.copy(prefix)
  prefix.writeUInt32BE(encodedHeader.byteLength, MAGIC.byteLength)
  return Buffer.concat([prefix, encodedHeader, Buffer.from(snapshot)])
}

function decodeCheckpoint(encoded: Buffer): { header: CheckpointHeader; snapshot: Uint8Array } {
  if (
    encoded.byteLength < MAGIC.byteLength + 4 ||
    !encoded.subarray(0, MAGIC.byteLength).equals(MAGIC)
  ) {
    throw new Error('invalid checkpoint magic')
  }
  const headerLength = encoded.readUInt32BE(MAGIC.byteLength)
  const headerStart = MAGIC.byteLength + 4
  const snapshotStart = headerStart + headerLength
  if (snapshotStart > encoded.byteLength) throw new Error('truncated checkpoint header')
  const header = JSON.parse(encoded.subarray(headerStart, snapshotStart).toString('utf8')) as CheckpointHeader
  const snapshot = Uint8Array.from(encoded.subarray(snapshotStart))
  if (snapshot.byteLength !== header.snapshotLength) throw new Error('checkpoint snapshot length mismatch')
  return { header, snapshot }
}

function assertHeaderMatchesRow(header: CheckpointHeader, row: StoredCheckpoint): void {
  if (
    header.id !== row.id ||
    header.sessionId !== row.session_id ||
    header.generation !== row.generation ||
    header.terminalSequence !== row.terminal_sequence ||
    header.domainEventSequence !== row.domain_event_sequence ||
    header.screenEpoch !== row.screen_epoch
  ) {
    throw new Error('checkpoint header does not match its SQLite index')
  }
}

function digest(encoded: Buffer): string {
  return createHash('sha256').update(encoded).digest('hex')
}

function validateInput(input: CreateCheckpointInput): void {
  if (
    input.sessionId.length > 160 ||
    !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(input.sessionId)
  ) {
    throw new Error('sessionId contains unsupported characters')
  }
  for (const [name, value] of [
    ['terminalSequence', input.terminalSequence],
    ['domainEventSequence', input.domainEventSequence],
    ['screenEpoch', input.screenEpoch]
  ] as const) {
    if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${name} must be non-negative`)
  }
  if (input.snapshot.byteLength > MAX_CHECKPOINT_SNAPSHOT_BYTES) {
    throw new Error('checkpoint snapshot exceeds the storage limit')
  }
}

async function syncDirectory(directory: string): Promise<void> {
  try {
    const handle = await open(directory, 'r')
    await handle.sync()
    await handle.close()
  } catch (error) {
    if (process.platform !== 'win32') throw error
  }
}
