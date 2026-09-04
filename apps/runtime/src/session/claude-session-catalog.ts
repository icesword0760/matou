import { createReadStream } from 'node:fs'
import { open, readdir, realpath, stat } from 'node:fs/promises'
import { resolve } from 'node:path'

import type {
  ClaudeSessionDetail,
  ClaudeSessionEventKind,
  ClaudeSessionListResult,
  ClaudeSessionPermissionMode,
  ClaudeSessionPreviewEvent,
  ClaudeSessionSearchHit,
  ClaudeSessionSearchResult,
  ClaudeSessionSummary
} from '@matou/contracts'

interface CatalogQuery {
  cwd: string
  query: string
}

type CatalogSearchScope = 'metadata' | 'all'

interface IndexedEvent {
  index: number
  kind: ClaudeSessionEventKind
  role?: 'user' | 'assistant'
  timestamp?: number
  toolName?: string
  offset: number
  length: number
}

interface IndexedTranscript {
  providerSessionId: string
  title: string
  autoTitle?: string
  cwd: string
  updatedAt: number
  model?: string
  permissionMode: ClaudeSessionPermissionMode
  path: string
  mtimeMs: number
  size: number
  events: IndexedEvent[]
}

interface JsonLine {
  offset: number
  length: number
  source: Buffer
}

const DEFAULT_EVENT_PAGE_LIMIT = 200
const MAX_EVENT_PAGE_LIMIT = 500
const MAX_SESSION_LIST_PAGE_LIMIT = 200
const MAX_SEARCH_PAGE_LIMIT = 200
const SEARCH_READ_BATCH_SIZE = 64
const SEARCH_READ_BATCH_BYTES = 4 * 1024 * 1024
const SEARCH_CACHE_LIMIT = 16

export class ClaudeSessionCatalog {
  readonly #projectsRoot: string
  readonly #fileCache = new Map<string, {
    mtimeMs: number
    size: number
    transcript: IndexedTranscript | undefined
  }>()
  readonly #searchCache = new Map<string, {
    mtimeMs: number
    size: number
    hits: ClaudeSessionSearchHit[]
  }>()

  constructor(projectsRoot: string) {
    this.#projectsRoot = resolve(projectsRoot)
  }

  async list(input: CatalogQuery & {
    offset?: number
    limit?: number
    searchScope?: CatalogSearchScope
  }): Promise<ClaudeSessionListResult> {
    const transcripts = await this.#readWorkspace(input.cwd)
    const query = normalizeQuery(input.query)
    const searchScope = input.searchScope ?? 'all'
    const sessions = (await Promise.all(transcripts.map(async (transcript) => {
      const contentHits = query && searchScope === 'all'
        ? await this.#searchTranscript(transcript, query)
        : []
      return summarize(transcript, query, searchScope, contentHits)
    })))
      .filter((session) => !query || session.matchCount > 0)
      .sort((left, right) => right.updatedAt - left.updatedAt || left.title.localeCompare(right.title))
    const offset = clampInteger(input.offset ?? 0, 0, sessions.length)
    const limit = clampInteger(input.limit ?? 50, 1, MAX_SESSION_LIST_PAGE_LIMIT)
    const page = sessions.slice(offset, offset + limit)
    const nextOffset = offset + page.length
    return {
      sessions: page,
      total: sessions.length,
      offset,
      limit,
      nextOffset,
      hasMore: nextOffset < sessions.length
    }
  }

  async detail(input: CatalogQuery & {
    providerSessionId: string
    beforeEventIndex?: number
    aroundEventIndex?: number
    limit?: number
  }): Promise<ClaudeSessionDetail> {
    requireProviderSessionId(input.providerSessionId)
    const transcript = (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
    if (!transcript) throw new Error('Claude Code 会话不存在或不属于当前工作空间')
    const query = normalizeQuery(input.query)
    const limit = clampInteger(
      input.limit ?? DEFAULT_EVENT_PAGE_LIMIT,
      1,
      MAX_EVENT_PAGE_LIMIT
    )
    let selected: IndexedEvent[]
    let hits: ClaudeSessionSearchHit[] = []
    if (query) {
      hits = await this.#searchTranscript(transcript, query)
      selected = selectEventWindow(transcript.events, {
        limit,
        ...(input.beforeEventIndex === undefined ? {} : { beforeEventIndex: input.beforeEventIndex }),
        ...(input.aroundEventIndex === undefined ? {} : { aroundEventIndex: input.aroundEventIndex })
      })
    } else {
      selected = selectEventWindow(transcript.events, {
        limit,
        ...(input.beforeEventIndex === undefined ? {} : { beforeEventIndex: input.beforeEventIndex }),
        ...(input.aroundEventIndex === undefined ? {} : { aroundEventIndex: input.aroundEventIndex })
      })
    }
    const events = await readIndexedEvents(transcript, selected, query)
    const firstIndex = selected[0]?.index ?? 0
    const lastIndex = selected.at(-1)?.index ?? 0
    return {
      ...summarize(transcript, '', 'metadata', []),
      matchCount: hits.length,
      hits: hits.slice(0, 4),
      events,
      page: {
        startEventIndex: firstIndex,
        endEventIndex: lastIndex,
        total: transcript.events.length,
        hasEarlier: firstIndex > 1,
        hasLater: lastIndex > 0 && lastIndex < transcript.events.length
      }
    }
  }

  async search(input: CatalogQuery & {
    providerSessionId: string
    offset?: number
    limit?: number
  }): Promise<ClaudeSessionSearchResult> {
    requireProviderSessionId(input.providerSessionId)
    const transcript = (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
    if (!transcript) throw new Error('Claude Code 会话不存在或不属于当前工作空间')
    const query = normalizeQuery(input.query)
    const allHits = query ? await this.#searchTranscript(transcript, query) : []
    const offset = clampInteger(input.offset ?? 0, 0, allHits.length)
    const limit = clampInteger(input.limit ?? 100, 1, MAX_SEARCH_PAGE_LIMIT)
    const hits = allHits.slice(offset, offset + limit)
    const nextOffset = offset + hits.length
    return {
      query,
      hits,
      total: allHits.length,
      offset,
      limit,
      nextOffset,
      hasMore: nextOffset < allHits.length
    }
  }

  async autoTitle(input: { cwd: string; providerSessionId: string }): Promise<string | undefined> {
    requireProviderSessionId(input.providerSessionId)
    return (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
      ?.autoTitle
  }

  async #searchTranscript(
    transcript: IndexedTranscript,
    query: string
  ): Promise<ClaudeSessionSearchHit[]> {
    const cacheKey = `${transcript.path}\0${query}`
    const cached = this.#searchCache.get(cacheKey)
    if (cached?.mtimeMs === transcript.mtimeMs && cached.size === transcript.size) {
      this.#searchCache.delete(cacheKey)
      this.#searchCache.set(cacheKey, cached)
      return cached.hits
    }
    const hits: ClaudeSessionSearchHit[] = []
    for (const refs of indexedEventBatches(transcript.events)) {
      const events = await readIndexedEvents(transcript, refs, query)
      for (const event of events) {
        if (!event.matched) continue
        hits.push({ eventIndex: event.index, kind: event.kind, excerpt: excerpt(event.text, query) })
      }
    }
    this.#searchCache.set(cacheKey, {
      mtimeMs: transcript.mtimeMs,
      size: transcript.size,
      hits
    })
    while (this.#searchCache.size > SEARCH_CACHE_LIMIT) {
      const oldest = this.#searchCache.keys().next().value as string | undefined
      if (oldest === undefined) break
      this.#searchCache.delete(oldest)
    }
    return hits
  }

  async #readWorkspace(cwd: string): Promise<IndexedTranscript[]> {
    const normalizedCwd = await canonicalPath(cwd)
    const directoryNames = [...new Set([
      encodeClaudeProjectPath(cwd), encodeClaudeProjectPath(normalizedCwd)
    ])]
    const candidates: Array<{ directory: string; name: string }> = []
    for (const directoryName of directoryNames) {
      const directory = resolve(this.#projectsRoot, directoryName)
      if (!directory.startsWith(`${this.#projectsRoot}/`) && directory !== this.#projectsRoot) continue
      try {
        for (const name of await readdir(directory)) {
          if (name.endsWith('.jsonl')) candidates.push({ directory, name })
        }
      } catch (error) {
        if (!isMissing(error)) throw error
      }
    }
    const transcripts = await mapWithConcurrency(candidates, 8, async ({ directory, name }) => {
      const providerSessionId = name.slice(0, -'.jsonl'.length)
      if (!isProviderSessionId(providerSessionId)) return undefined
      const path = resolve(directory, name)
      try {
        const metadata = await stat(path)
        const cached = this.#fileCache.get(path)
        let transcript = cached?.mtimeMs === metadata.mtimeMs && cached.size === metadata.size
          ? cached.transcript
          : undefined
        if (!cached || cached.mtimeMs !== metadata.mtimeMs || cached.size !== metadata.size) {
          transcript = await indexTranscript(providerSessionId, path, metadata.mtimeMs, metadata.size)
          this.#fileCache.set(path, { mtimeMs: metadata.mtimeMs, size: metadata.size, transcript })
        }
        if (!transcript) return undefined
        const transcriptCwd = await canonicalPath(transcript.cwd)
        return transcriptCwd === normalizedCwd
          ? { ...transcript, cwd: normalizedCwd }
          : undefined
      } catch (error) {
        if (isMissing(error)) return undefined
        throw error
      }
    })
    return [...new Map(transcripts
      .filter((value): value is IndexedTranscript => value !== undefined)
      .map((value) => [value.providerSessionId, value])).values()]
  }
}

export function encodeClaudeProjectPath(cwd: string): string {
  return resolve(cwd).replace(/[^A-Za-z0-9]/g, '-')
}

async function indexTranscript(
  providerSessionId: string,
  path: string,
  mtimeMs: number,
  size: number
): Promise<IndexedTranscript | undefined> {
  let cwd = ''
  let title = ''
  let autoTitle = ''
  let updatedAt = 0
  let model: string | undefined
  let permissionMode: ClaudeSessionPermissionMode = 'default'
  const events: IndexedEvent[] = []
  for await (const line of scanJsonLines(path)) {
    if (line.length === 0) continue
    let row: Record<string, unknown>
    try {
      const parsed = JSON.parse(line.source.toString('utf8')) as unknown
      if (!isRecord(parsed)) continue
      row = parsed
    } catch {
      continue
    }
    if (typeof row.cwd === 'string' && row.cwd.trim()) cwd = row.cwd
    if (row.type === 'ai-title' && typeof row.aiTitle === 'string' && row.aiTitle.trim()) {
      autoTitle = row.aiTitle.trim()
    }
    const timestamp = parseTimestamp(row.timestamp)
    if (timestamp !== undefined) updatedAt = Math.max(updatedAt, timestamp)
    if (isPermissionMode(row.permissionMode)) permissionMode = row.permissionMode
    const message = isRecord(row.message) ? row.message : undefined
    if (message && isPermissionMode(message.permissionMode)) permissionMode = message.permissionMode
    if (message && typeof message.model === 'string' && message.model.trim()) model = message.model
    const event = message ? eventFromMessage(events.length + 1, message, timestamp) : undefined
    if (!event) continue
    events.push({
      index: event.index,
      kind: event.kind,
      ...(event.role ? { role: event.role } : {}),
      ...(event.timestamp === undefined ? {} : { timestamp: event.timestamp }),
      ...(event.toolName ? { toolName: event.toolName } : {}),
      offset: line.offset,
      length: line.length
    })
    if (!title && event.role === 'user' && event.kind === 'user') title = compactTitle(event.text)
  }
  if (!cwd || events.length === 0) return undefined
  return {
    providerSessionId,
    title: autoTitle || title || '未命名 Claude 会话',
    ...(autoTitle ? { autoTitle } : {}),
    cwd,
    updatedAt,
    ...(model ? { model } : {}),
    permissionMode,
    path,
    mtimeMs,
    size,
    events
  }
}

async function* scanJsonLines(path: string): AsyncGenerator<JsonLine> {
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

function selectEventWindow(
  events: IndexedEvent[],
  input: { limit: number; beforeEventIndex?: number; aroundEventIndex?: number }
): IndexedEvent[] {
  if (events.length === 0) return []
  if (input.aroundEventIndex !== undefined) {
    const anchor = clampInteger(input.aroundEventIndex, 1, events.length)
    let start = Math.max(0, anchor - 1 - Math.floor(input.limit / 2))
    let end = Math.min(events.length, start + input.limit)
    start = Math.max(0, end - input.limit)
    return events.slice(start, end)
  }
  const end = input.beforeEventIndex === undefined
    ? events.length
    : clampInteger(input.beforeEventIndex - 1, 0, events.length)
  return events.slice(Math.max(0, end - input.limit), end)
}

async function readIndexedEvents(
  transcript: IndexedTranscript,
  refs: IndexedEvent[],
  query: string
): Promise<ClaudeSessionPreviewEvent[]> {
  if (refs.length === 0) return []
  const rangeStart = refs[0]!.offset
  const rangeEnd = refs.at(-1)!.offset + refs.at(-1)!.length
  const buffer = Buffer.allocUnsafe(rangeEnd - rangeStart)
  const handle = await open(transcript.path, 'r')
  try {
    let readOffset = 0
    while (readOffset < buffer.length) {
      const result = await handle.read(buffer, readOffset, buffer.length - readOffset, rangeStart + readOffset)
      if (result.bytesRead === 0) break
      readOffset += result.bytesRead
    }
    const events: ClaudeSessionPreviewEvent[] = []
    for (const ref of refs) {
      const start = ref.offset - rangeStart
      if (start + ref.length > readOffset) continue
      const source = buffer.subarray(start, start + ref.length).toString('utf8')
      try {
        const row = JSON.parse(source) as unknown
        if (!isRecord(row)) continue
        const message = isRecord(row.message) ? row.message : undefined
        const event = message ? eventFromMessage(ref.index, message, parseTimestamp(row.timestamp)) : undefined
        if (!event) continue
        events.push({ ...event, matched: Boolean(query && searchableEventText(event).includes(query)) })
      } catch {
        continue
      }
    }
    return events
  } finally {
    await handle.close()
  }
}

function* indexedEventBatches(events: IndexedEvent[]): Generator<IndexedEvent[]> {
  let batch: IndexedEvent[] = []
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

function eventFromMessage(
  index: number,
  message: Record<string, unknown>,
  timestamp: number | undefined
): Omit<ClaudeSessionPreviewEvent, 'matched'> | undefined {
  const role = message.role === 'user' || message.role === 'assistant' ? message.role : undefined
  const content = message.content
  const parts = contentParts(content)
  if (parts.length === 0) return undefined
  const tool = parts.find((part) => part.kind === 'tool')
  const kind: ClaudeSessionEventKind = tool
    ? 'tool'
    : role === 'user' ? 'user' : role === 'assistant' ? 'assistant' : 'system'
  const text = parts.map(({ text }) => text).filter(Boolean).join('\n').trim()
  if (!text) return undefined
  return {
    index,
    kind,
    ...(role ? { role } : {}),
    ...(timestamp === undefined ? {} : { timestamp }),
    text,
    ...(tool?.toolName ? { toolName: tool.toolName } : {})
  }
}

function contentParts(content: unknown): Array<{ kind: 'text' | 'tool'; text: string; toolName?: string }> {
  if (typeof content === 'string') return content.trim() ? [{ kind: 'text', text: content }] : []
  if (!Array.isArray(content)) return []
  const parts: Array<{ kind: 'text' | 'tool'; text: string; toolName?: string }> = []
  for (const item of content) {
    if (!isRecord(item)) continue
    if (item.type === 'text' && typeof item.text === 'string') {
      parts.push({ kind: 'text', text: item.text })
      continue
    }
    if (item.type === 'tool_use' && typeof item.name === 'string') {
      parts.push({
        kind: 'tool', toolName: item.name,
        text: `${item.name} ${safeJson(item.input)}`.trim()
      })
      continue
    }
    if (item.type === 'tool_result') {
      const text = typeof item.content === 'string' ? item.content : safeJson(item.content)
      parts.push({ kind: 'tool', toolName: 'tool_result', text })
    }
  }
  return parts
}

function summarize(
  transcript: IndexedTranscript,
  query: string,
  searchScope: CatalogSearchScope,
  contentHits: ClaudeSessionSearchHit[]
): ClaudeSessionSummary {
  const hits = searchScope === 'all' ? contentHits.slice(0, 4) : []
  const contentMatchCount = searchScope === 'all' ? contentHits.length : 0
  const metadataMatch = Boolean(query && normalizeQuery([
    transcript.title,
    transcript.providerSessionId,
    transcript.cwd,
    transcript.model ?? '',
    transcript.permissionMode
  ].join(' ')).includes(query))
  const matchCount = contentMatchCount + (metadataMatch ? 1 : 0)
  return {
    providerSessionId: transcript.providerSessionId,
    title: transcript.title,
    cwd: transcript.cwd,
    updatedAt: transcript.updatedAt,
    ...(transcript.model ? { model: transcript.model } : {}),
    permissionMode: transcript.permissionMode,
    eventCount: transcript.events.length,
    matchCount,
    hits,
    availability: 'available'
  }
}

function searchableEventText(event: Omit<ClaudeSessionPreviewEvent, 'matched'>): string {
  return normalizeQuery(`${event.toolName ?? ''} ${event.text}`)
}

function excerpt(text: string, query: string): string {
  const flat = text.replace(/\s+/g, ' ').trim()
  const position = flat.toLocaleLowerCase().indexOf(query)
  if (position < 0) return flat.slice(0, 120)
  const start = Math.max(0, position - 36)
  const end = Math.min(flat.length, position + query.length + 70)
  return `${start > 0 ? '…' : ''}${flat.slice(start, end)}${end < flat.length ? '…' : ''}`
}

function compactTitle(value: string): string {
  const compact = value.replace(/\s+/g, ' ').trim()
  return compact.length > 72 ? `${compact.slice(0, 71)}…` : compact
}

function normalizeQuery(value: string): string {
  return value.trim().toLocaleLowerCase()
}

function clampInteger(value: number, minimum: number, maximum: number): number {
  const normalized = Number.isFinite(value) ? Math.trunc(value) : minimum
  return Math.max(minimum, Math.min(normalized, maximum))
}

async function mapWithConcurrency<T, Result>(
  values: T[],
  concurrency: number,
  mapper: (value: T, index: number) => Promise<Result>
): Promise<Result[]> {
  if (values.length === 0) return []
  const results = new Array<Result>(values.length)
  let cursor = 0
  await Promise.all(Array.from({ length: Math.min(concurrency, values.length) }, async () => {
    for (;;) {
      const index = cursor
      cursor += 1
      if (index >= values.length) return
      results[index] = await mapper(values[index]!, index)
    }
  }))
  return results
}

export function latestClaudeAutoTitle(source: string, providerSessionId?: string): string | undefined {
  let latest: string | undefined
  for (const line of source.split(/\r?\n/)) {
    if (!line.trim()) continue
    try {
      const parsed = JSON.parse(line) as unknown
      if (!isRecord(parsed) || parsed.type !== 'ai-title') continue
      if (providerSessionId && typeof parsed.sessionId === 'string' &&
        parsed.sessionId !== providerSessionId) continue
      if (typeof parsed.aiTitle === 'string' && parsed.aiTitle.trim()) latest = parsed.aiTitle.trim()
    } catch {
      continue
    }
  }
  return latest
}

function parseTimestamp(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value !== 'string') return undefined
  const parsed = Date.parse(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

function isPermissionMode(value: unknown): value is ClaudeSessionPermissionMode {
  return value === 'default' || value === 'auto' || value === 'acceptEdits' || value === 'plan' ||
    value === 'bypassPermissions'
}

function safeJson(value: unknown): string {
  try { return JSON.stringify(value) }
  catch { return String(value ?? '') }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function isProviderSessionId(value: string): boolean {
  return /^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$/.test(value)
}

function requireProviderSessionId(value: string): void {
  if (!isProviderSessionId(value)) throw new Error('Claude Code 会话标识格式错误')
}

async function canonicalPath(value: string): Promise<string> {
  const absolute = resolve(value)
  try { return await realpath(absolute) }
  catch { return absolute }
}

function isMissing(error: unknown): boolean {
  return isRecord(error) && error.code === 'ENOENT'
}
