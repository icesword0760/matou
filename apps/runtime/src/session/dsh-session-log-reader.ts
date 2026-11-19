import { createReadStream } from 'node:fs'
import { open, stat, type FileHandle } from 'node:fs/promises'
import { zstdDecompress } from 'node:zlib'
import { promisify } from 'node:util'
import { runtimeMessages } from '../i18n/messages'

const decompress = promisify(zstdDecompress)
export const DSH_MAX_FRAME_BYTES = 32 * 1024 * 1024
export const DSH_MAX_LINE_BYTES = 16 * 1024 * 1024
const MAX_LOG_BYTES = 512 * 1024 * 1024
export interface DshLineReference { frameStart?: number; frameEnd?: number; offset: number; length: number }
export interface DshLogLine extends DshLineReference { row: Record<string, unknown> }
class DshReadLimitError extends Error {}
const corrupt = () => new Error(runtimeMessages().session.dshSessionCorrupt)
const tooLarge = () => new DshReadLimitError(runtimeMessages().session.dshSessionReadLimit)

async function bytes(file: FileHandle, at: number, count: number, size: number): Promise<Buffer | undefined> {
  if (at + count > size) return undefined
  const data = Buffer.allocUnsafe(count)
  let read = 0
  while (read < count) {
    const result = await file.read(data, read, count - read, at + read)
    if (!result.bytesRead) return undefined
    read += result.bytesRead
  }
  return data
}

/** Locate each independent RFC 8878 frame by its headers; no whole-file buffering.
 * An incomplete final frame belongs to a live append and is retried on the next read. */
async function frameEnd(file: FileHandle, start: number, size: number): Promise<number | undefined> {
  const header = await bytes(file, start, 5, size)
  if (!header) return undefined
  if (header.readUInt32LE() !== 0xfd2fb528 || (header[4]! & 0x18)) throw corrupt()
  const descriptor = header[4]!, single = Boolean(descriptor & 0x20), flag = descriptor >>> 6
  const dictionary = [0, 1, 2, 4][descriptor & 3]!
  const contentSize = flag === 0 ? (single ? 1 : 0) : 1 << flag
  let cursor = start + 5 + (single ? 0 : 1) + dictionary + contentSize
  if (cursor > size) return undefined
  while (true) {
    if (cursor - start > DSH_MAX_FRAME_BYTES) throw tooLarge()
    const raw = await bytes(file, cursor, 3, size)
    if (!raw) return undefined
    const block = raw.readUIntLE(0, 3), kind = (block >>> 1) & 3
    if (kind === 3) throw corrupt()
    cursor += 3 + (kind === 1 ? 1 : block >>> 3)
    if (cursor - start > DSH_MAX_FRAME_BYTES) throw tooLarge()
    if (cursor > size) return undefined
    if (block & 1) break
  }
  if (descriptor & 4) cursor += 4
  if (cursor - start > DSH_MAX_FRAME_BYTES) throw tooLarge()
  return cursor <= size ? cursor : undefined
}

function parse(source: Buffer): Record<string, unknown> {
  if (source.length > DSH_MAX_LINE_BYTES) throw tooLarge()
  try {
    const row: unknown = JSON.parse(source.toString('utf8'))
    if (!row || typeof row !== 'object' || Array.isArray(row)) throw corrupt()
    return row as Record<string, unknown>
  } catch (error) {
    if (error instanceof SyntaxError) throw corrupt()
    throw error
  }
}

export async function decodeDshFrame(file: FileHandle, start: number, end: number): Promise<Buffer> {
  if (end - start > DSH_MAX_FRAME_BYTES) throw tooLarge()
  const encoded = await bytes(file, start, end - start, end)
  if (!encoded) throw corrupt()
  try { return await decompress(encoded, { maxOutputLength: DSH_MAX_FRAME_BYTES }) }
  catch { throw corrupt() }
}

export async function* scanDshLog(path: string): AsyncGenerator<DshLogLine> {
  const size = (await stat(path)).size
  if (size > MAX_LOG_BYTES) throw tooLarge()
  if (!path.endsWith('.zstd')) {
    let pending = Buffer.alloc(0), offset = 0
    for await (const chunk of createReadStream(path, { highWaterMark: 64 * 1024, end: Math.max(0, size - 1) })) {
      const data = pending.length ? Buffer.concat([pending, chunk as Buffer]) : chunk as Buffer
      let from = 0
      for (let at = data.indexOf(10); at >= 0; at = data.indexOf(10, from)) {
        if (at > from) yield { offset: offset + from, length: at - from, row: parse(data.subarray(from, at)) }
        from = at + 1
      }
      offset += from; pending = Buffer.from(data.subarray(from))
      if (pending.length > DSH_MAX_LINE_BYTES) throw tooLarge()
    }
    if (pending.length) {
      // A valid final record need not have a newline. A partial live JSON tail is not published.
      try { yield { offset, length: pending.length, row: parse(pending) } }
      catch (error) { if (error instanceof DshReadLimitError) throw error }
    }
    return
  }
  const file = await open(path, 'r')
  let cursor = 0, decodedBytes = 0
  try {
    while (cursor < size) {
      const end = await frameEnd(file, cursor, size)
      if (end === undefined) break
      const decoded = await decodeDshFrame(file, cursor, end)
      decodedBytes += decoded.length
      if (decodedBytes > MAX_LOG_BYTES) throw tooLarge()
      let from = 0
      for (let at = decoded.indexOf(10); at >= 0; at = decoded.indexOf(10, from)) {
        if (at > from) yield { frameStart: cursor, frameEnd: end, offset: from, length: at - from, row: parse(decoded.subarray(from, at)) }
        from = at + 1
      }
      if (from < decoded.length) yield { frameStart: cursor, frameEnd: end, offset: from, length: decoded.length - from, row: parse(decoded.subarray(from)) }
      cursor = end
    }
  } finally { await file.close() }
}

/** Grouped frame reads keep response memory bounded and decode a shared frame once. */
export async function readDshLines(path: string, refs: DshLineReference[]): Promise<Record<string, unknown>[]> {
  const file = await open(path, 'r')
  const rows = new Map<DshLineReference, Record<string, unknown>>()
  try {
    const groups = new Map<number, DshLineReference[]>()
    for (const ref of refs) {
      const key = ref.frameStart ?? -1
      const group = groups.get(key) ?? []; group.push(ref); groups.set(key, group)
    }
    for (const [start, group] of groups) {
      const frame = start === -1 ? undefined : await decodeDshFrame(file, start, group[0]!.frameEnd!)
      for (const ref of group) {
        const source = frame?.subarray(ref.offset, ref.offset + ref.length) ?? await bytes(file, ref.offset, ref.length, ref.offset + ref.length)
        if (!source || source.length !== ref.length) throw corrupt()
        rows.set(ref, parse(source))
      }
    }
    return refs.map(ref => rows.get(ref)!)
  } finally { await file.close() }
}
