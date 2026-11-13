import { createReadStream } from 'node:fs'
import { realpath } from 'node:fs/promises'
import { resolve } from 'node:path'

import type { ClaudeSessionEventKind, ClaudeSessionPreviewEvent } from '@matou/contracts'

export interface CatalogQuery {
  cwd: string
  query: string
}

export type CatalogSearchScope = 'metadata' | 'all'

export interface IndexedCatalogEvent {
  index: number
  kind: ClaudeSessionEventKind
  role?: 'user' | 'assistant'
  timestamp?: number
  toolName?: string
  offset: number
  length: number
}

export interface JsonLine {
  offset: number
  length: number
  source: Buffer
}

export const DEFAULT_EVENT_PAGE_LIMIT = 200
export const MAX_EVENT_PAGE_LIMIT = 500
export const MAX_SESSION_LIST_PAGE_LIMIT = 200
export const MAX_SEARCH_PAGE_LIMIT = 200
export const SEARCH_READ_BATCH_SIZE = 64
export const SEARCH_READ_BATCH_BYTES = 4 * 1024 * 1024
export const SEARCH_CACHE_LIMIT = 16

export async function* scanJsonLines(path: string): AsyncGenerator<JsonLine> {
  let pending = Buffer.alloc(0)
  let pendingOffset = 0
  for await (const value of createReadStream(path)) {
    const chunk = Buffer.isBuffer(value) ? value : Buffer.from(value)
    const buffer = pending.length > 0 ? Buffer.concat([pending, chunk]) : chunk
    let start = 0
    for (let index = 0; index < buffer.length; index += 1) {
      if (buffer[index] !== 0x0a) continue
      const rawEnd = index > start && buffer[index - 1] === 0x0d ? index - 1 : index
      yield {
        offset: pendingOffset + start,
        length: rawEnd - start,
        source: buffer.subarray(start, rawEnd)
      }
      start = index + 1
    }
    pendingOffset += start
    pending = start < buffer.length ? Buffer.from(buffer.subarray(start)) : Buffer.alloc(0)
  }
  if (pending.length > 0) {
    const rawEnd = pending.at(-1) === 0x0d ? pending.length - 1 : pending.length
    yield { offset: pendingOffset, length: rawEnd, source: pending.subarray(0, rawEnd) }
  }
}

export function selectEventWindow<T extends { index: number }>(
  events: T[],
  input: { limit: number; beforeEventIndex?: number; aroundEventIndex?: number }
): T[] {
  if (events.length === 0) return []
  if (input.aroundEventIndex !== undefined) {
    const anchor = clampInteger(input.aroundEventIndex, 1, events.length)
    let start = Math.max(0, anchor - 1 - Math.floor(input.limit / 2))
    const end = Math.min(events.length, start + input.limit)
    start = Math.max(0, end - input.limit)
    return events.slice(start, end)
  }
  const end = input.beforeEventIndex === undefined
    ? events.length
    : clampInteger(input.beforeEventIndex - 1, 0, events.length)
  return events.slice(Math.max(0, end - input.limit), end)
}

export function* indexedEventBatches(events: IndexedCatalogEvent[]): Generator<IndexedCatalogEvent[]> {
  let batch: IndexedCatalogEvent[] = []
  for (const event of events) {
    const first = batch[0]
    const span = first ? event.offset + event.length - first.offset : event.length
    if (batch.length > 0 && (batch.length >= SEARCH_READ_BATCH_SIZE || span > SEARCH_READ_BATCH_BYTES)) {
      yield batch
      batch = []
    }
    batch.push(event)
  }
  if (batch.length > 0) yield batch
}

export function searchableEventText(event: Omit<ClaudeSessionPreviewEvent, 'matched'>): string {
  return normalizeQuery(`${event.toolName ?? ''} ${event.text}`)
}

export function excerpt(text: string, query: string): string {
  const flat = text.replace(/\s+/g, ' ').trim()
  const position = flat.toLocaleLowerCase().indexOf(query)
  if (position < 0) return flat.slice(0, 120)
  const start = Math.max(0, position - 36)
  const end = Math.min(flat.length, position + query.length + 70)
  return `${start > 0 ? '…' : ''}${flat.slice(start, end)}${end < flat.length ? '…' : ''}`
}

export function compactTitle(value: string): string {
  const compact = value.replace(/\s+/g, ' ').trim()
  return compact.length > 72 ? `${compact.slice(0, 71)}…` : compact
}

export function normalizeQuery(value: string): string {
  return value.trim().toLocaleLowerCase()
}

export function clampInteger(value: number, minimum: number, maximum: number): number {
  const normalized = Number.isFinite(value) ? Math.trunc(value) : minimum
  return Math.max(minimum, Math.min(normalized, maximum))
}

export function parseTimestamp(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value !== 'string') return undefined
  const parsed = Date.parse(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

export function safeJson(value: unknown): string {
  try { return JSON.stringify(value) }
  catch { return String(value ?? '') }
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function isProviderSessionId(value: string): boolean {
  return /^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$/.test(value)
}

export async function canonicalPath(value: string): Promise<string> {
  const absolute = resolve(value)
  try { return await realpath(absolute) }
  catch { return absolute }
}

export function isMissing(error: unknown): boolean {
  return isRecord(error) && error.code === 'ENOENT'
}
