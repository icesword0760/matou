import { open, readFile, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { queryExternalReadOnlyRow } from '../storage/database'

import type {
  ClaudeSessionDetail,
  ClaudeSessionEventKind,
  ClaudeSessionListResult,
  ClaudeSessionPreviewEvent,
  ClaudeSessionSearchHit,
  ClaudeSessionSearchResult,
  ClaudeSessionSummary
} from '@matou/contracts'

import { EntityMissingError } from '../errors'
import { runtimeMessages } from '../i18n/messages'
import {
  DEFAULT_EVENT_PAGE_LIMIT,
  MAX_EVENT_PAGE_LIMIT,
  MAX_SEARCH_PAGE_LIMIT,
  MAX_SESSION_LIST_PAGE_LIMIT,
  SEARCH_CACHE_LIMIT,
  canonicalPath,
  clampInteger,
  compactTitle,
  excerpt,
  indexedEventBatches,
  isMissing,
  isProviderSessionId,
  isRecord,
  normalizeQuery,
  parseTimestamp,
  safeJson,
  scanJsonLines,
  searchableEventText,
  selectEventWindow,
  type CatalogQuery,
  type CatalogSearchScope,
  type IndexedCatalogEvent
} from './session-catalog-utils'

interface MetadataConversation {
  providerSessionId: string
  title: string
  autoTitle?: string
  updatedAt: number
  stepCount: number
  workspaceUris: string[]
}

interface IndexedAntigravityTranscript {
  path: string
  mtimeMs: number
  size: number
  events: IndexedCatalogEvent[]
}

export function defaultAntigravityRoot(): string {
  return process.env.MATOU_ANTIGRAVITY_HOME?.trim() || resolve(homedir(), '.gemini', 'antigravity-cli')
}

export function antigravityMetadataPath(root: string): string {
  return resolve(root, 'cache', 'conversation_metadata.json')
}

export function antigravitySummariesDbPath(root: string): string {
  return resolve(root, 'conversation_summaries.db')
}

export function antigravityTranscriptPath(root: string, providerSessionId: string): string {
  return resolve(root, 'brain', providerSessionId, '.system_generated', 'logs', 'transcript.jsonl')
}

function inspectAntigravityMetadataTitle(
  source: string,
  providerSessionId: string
): { internal: boolean; title?: string } {
  let parsed: unknown
  try {
    parsed = JSON.parse(source) as unknown
  } catch {
    return { internal: false }
  }
  const conversations = readConversationsMap(parsed)
  if (!conversations) return { internal: false }
  const rawEntry = conversations[providerSessionId]
  if (!isRecord(rawEntry)) return { internal: false }
  if (rawEntry.is_internal === true) return { internal: true }
  const summary = isRecord(rawEntry.summary) ? rawEntry.summary : undefined
  if (!summary || summary.Internal === true) return { internal: summary?.Internal === true }
  const title = typeof summary.Title === 'string' ? summary.Title.trim() : ''
  return title ? { internal: false, title } : { internal: false }
}

export function extractAntigravityAutoTitleFromJson(
  source: string,
  providerSessionId: string
): string | undefined {
  const inspected = inspectAntigravityMetadataTitle(source, providerSessionId)
  return inspected.internal ? undefined : inspected.title
}

function readTitleFromSummariesDb(
  dbPath: string,
  providerSessionId: string
): string | undefined {
  const row = queryExternalReadOnlyRow<{ title?: unknown }>(
    dbPath,
    `SELECT title FROM conversation_summaries
     WHERE conversation_id = ?
       AND (app_data_dir = '' OR app_data_dir = 'antigravity-cli')
     LIMIT 1`,
    providerSessionId
  )
  const title = typeof row?.title === 'string' ? row.title.trim() : ''
  return title || undefined
}

export async function latestAntigravityAutoTitle(
  metadataFilePath: string,
  providerSessionId: string
): Promise<string | undefined> {
  if (!isProviderSessionId(providerSessionId)) return undefined
  let metadataResult: { internal: boolean; title?: string } = { internal: false }
  try {
    const source = await readFile(metadataFilePath, 'utf8')
    metadataResult = inspectAntigravityMetadataTitle(source, providerSessionId)
  } catch (error) {
    if (!isMissing(error)) throw error
  }
  if (metadataResult.internal) return undefined
  const summariesDbPath = resolve(dirname(metadataFilePath), '..', 'conversation_summaries.db')
  return readTitleFromSummariesDb(summariesDbPath, providerSessionId) ?? metadataResult.title
}

export class AntigravitySessionCatalog {
  readonly #root: string
  readonly #metadataCache = { mtimeMs: -1, size: -1, entries: [] as MetadataConversation[] }
  readonly #transcriptCache = new Map<string, IndexedAntigravityTranscript>()
  readonly #searchCache = new Map<string, {
    mtimeMs: number
    size: number
    hits: ClaudeSessionSearchHit[]
  }>()

  constructor(root = defaultAntigravityRoot()) {
    this.#root = resolve(root)
  }

  get root(): string {
    return this.#root
  }

  async list(input: CatalogQuery & {
    offset?: number
    limit?: number
    searchScope?: CatalogSearchScope
    titleForSession?(providerSessionId: string): string | undefined
  }): Promise<ClaudeSessionListResult> {
    const conversations = await this.#readWorkspace(input.cwd)
    const query = normalizeQuery(input.query)
    const searchScope = input.searchScope ?? 'all'
    const sessions = (await Promise.all(conversations.map(async (conversation) => {
      const contentHits = query && searchScope === 'all'
        ? await this.#searchConversation(conversation.providerSessionId, query)
        : []
      const title = input.titleForSession?.(conversation.providerSessionId)
      return summarizeConversation(
        title ? { ...conversation, title } : conversation,
        query,
        searchScope,
        contentHits
      )
    })))
      .filter((session) => !query || session.matchCount > 0)
      .sort((left, right) => right.updatedAt - left.updatedAt || left.title.localeCompare(right.title))
    const offset = clampInteger(input.offset ?? 0, 0, sessions.length)
    const limit = clampInteger(input.limit ?? 50, 1, MAX_SESSION_LIST_PAGE_LIMIT)
    const page = sessions.slice(offset, offset + limit)
    const nextOffset = offset + page.length
    return {
      provider: 'antigravity',
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
    requireAntigravitySessionId(input.providerSessionId)
    const conversation = (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
    if (!conversation) {
      throw new EntityMissingError(runtimeMessages().session.antigravitySessionNotInWorkspace)
    }
    const transcript = await this.#indexTranscript(conversation.providerSessionId)
    const query = normalizeQuery(input.query)
    const limit = clampInteger(input.limit ?? DEFAULT_EVENT_PAGE_LIMIT, 1, MAX_EVENT_PAGE_LIMIT)
    const hits = query && transcript ? await this.#searchTranscript(transcript, query) : []
    const selected = transcript
      ? selectEventWindow(transcript.events, {
          limit,
          ...(input.beforeEventIndex === undefined ? {} : { beforeEventIndex: input.beforeEventIndex }),
          ...(input.aroundEventIndex === undefined ? {} : { aroundEventIndex: input.aroundEventIndex })
        })
      : []
    const events = transcript ? await readIndexedEvents(transcript.path, selected, query) : []
    const firstIndex = selected[0]?.index ?? 0
    const lastIndex = selected.at(-1)?.index ?? 0
    const total = transcript?.events.length ?? 0
    return {
      ...summarizeConversation(conversation, '', 'metadata', []),
      matchCount: hits.length,
      hits: hits.slice(0, 4),
      events,
      page: {
        startEventIndex: firstIndex,
        endEventIndex: lastIndex,
        total,
        hasEarlier: firstIndex > 1,
        hasLater: lastIndex > 0 && lastIndex < total
      }
    }
  }

  async search(input: CatalogQuery & {
    providerSessionId: string
    offset?: number
    limit?: number
  }): Promise<ClaudeSessionSearchResult> {
    requireAntigravitySessionId(input.providerSessionId)
    const conversation = (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
    if (!conversation) {
      throw new EntityMissingError(runtimeMessages().session.antigravitySessionNotInWorkspace)
    }
    const transcript = await this.#indexTranscript(conversation.providerSessionId)
    const query = normalizeQuery(input.query)
    const allHits = query && transcript ? await this.#searchTranscript(transcript, query) : []
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
    requireAntigravitySessionId(input.providerSessionId)
    return (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
      ?.autoTitle
  }

  async #readWorkspace(cwd: string): Promise<Array<MetadataConversation & { cwd: string }>> {
    const normalizedCwd = await canonicalPath(cwd)
    const all = await this.#loadMetadata()
    const matched: Array<MetadataConversation & { cwd: string }> = []
    for (const entry of all) {
      const hasWorkspaceMatch = await matchesWorkspaceUris(entry.workspaceUris, normalizedCwd)
      if (!hasWorkspaceMatch) continue
      matched.push({ ...entry, cwd: normalizedCwd })
    }
    return matched
  }

  async #loadMetadata(): Promise<MetadataConversation[]> {
    const filePath = antigravityMetadataPath(this.#root)
    let fileStat
    try {
      fileStat = await stat(filePath)
    } catch (error) {
      if (isMissing(error)) {
        this.#metadataCache.mtimeMs = -1
        this.#metadataCache.size = -1
        this.#metadataCache.entries = []
        return []
      }
      throw error
    }
    if (
      this.#metadataCache.mtimeMs === fileStat.mtimeMs &&
      this.#metadataCache.size === fileStat.size
    ) {
      return this.#metadataCache.entries
    }
    let parsed: unknown
    try {
      parsed = JSON.parse(await readFile(filePath, 'utf8')) as unknown
    } catch {
      return []
    }
    const conversations = readConversationsMap(parsed)
    if (!conversations) return []
    const entries: MetadataConversation[] = []
    for (const [providerSessionId, rawEntry] of Object.entries(conversations)) {
      if (!isProviderSessionId(providerSessionId) || !isRecord(rawEntry)) continue
      if (rawEntry.is_internal === true) continue
      const summary = isRecord(rawEntry.summary) ? rawEntry.summary : undefined
      if (!summary || summary.Internal === true) continue
      if (summary.AppDataDir !== 'antigravity-cli') continue
      const workspaceUris = Array.isArray(summary.WorkspaceURIs)
        ? summary.WorkspaceURIs.filter((value): value is string => typeof value === 'string')
        : []
      const rawTitle = typeof summary.Title === 'string' ? summary.Title.trim() : ''
      const rawPreview = typeof summary.Preview === 'string' ? summary.Preview : ''
      const updatedAt = parseTimestamp(summary.UpdatedAt) ?? parseTimestamp(rawEntry.last_modified_time) ?? 0
      const stepCount = typeof summary.NumSteps === 'number' && Number.isFinite(summary.NumSteps)
        ? Math.max(0, Math.trunc(summary.NumSteps))
        : 0
      entries.push({
        providerSessionId,
        title: (rawTitle ? compactTitle(rawTitle) : '') ||
          (rawPreview ? compactTitle(rawPreview) : '') ||
          runtimeMessages().session.untitledAntigravitySession,
        ...(rawTitle ? { autoTitle: rawTitle } : {}),
        updatedAt,
        stepCount,
        workspaceUris
      })
    }
    this.#metadataCache.mtimeMs = fileStat.mtimeMs
    this.#metadataCache.size = fileStat.size
    this.#metadataCache.entries = entries
    return entries
  }

  async #indexTranscript(providerSessionId: string): Promise<IndexedAntigravityTranscript | undefined> {
    const filePath = antigravityTranscriptPath(this.#root, providerSessionId)
    let fileStat
    try {
      fileStat = await stat(filePath)
    } catch (error) {
      if (isMissing(error)) {
        this.#transcriptCache.delete(filePath)
        return undefined
      }
      throw error
    }
    const cached = this.#transcriptCache.get(filePath)
    if (cached && cached.mtimeMs === fileStat.mtimeMs && cached.size === fileStat.size) {
      return cached
    }
    const events: IndexedCatalogEvent[] = []
    for await (const line of scanJsonLines(filePath)) {
      if (line.length === 0) continue
      let row: Record<string, unknown>
      try {
        const parsed = JSON.parse(line.source.toString('utf8')) as unknown
        if (!isRecord(parsed)) continue
        row = parsed
      } catch {
        continue
      }
      const event = eventFromAntigravityRow(events.length + 1, row)
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
    }
    const indexed: IndexedAntigravityTranscript = {
      path: filePath,
      mtimeMs: fileStat.mtimeMs,
      size: fileStat.size,
      events
    }
    this.#transcriptCache.set(filePath, indexed)
    return indexed
  }

  async #searchConversation(providerSessionId: string, query: string): Promise<ClaudeSessionSearchHit[]> {
    const transcript = await this.#indexTranscript(providerSessionId)
    if (!transcript) return []
    return this.#searchTranscript(transcript, query)
  }

  async #searchTranscript(
    transcript: IndexedAntigravityTranscript,
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
      const events = await readIndexedEvents(transcript.path, refs, query)
      for (const event of events) {
        if (!event.matched) continue
        hits.push({ eventIndex: event.index, kind: event.kind, excerpt: excerpt(event.text, query) })
      }
    }
    this.#searchCache.set(cacheKey, { mtimeMs: transcript.mtimeMs, size: transcript.size, hits })
    while (this.#searchCache.size > SEARCH_CACHE_LIMIT) {
      const oldest = this.#searchCache.keys().next().value as string | undefined
      if (oldest === undefined) break
      this.#searchCache.delete(oldest)
    }
    return hits
  }
}

function readConversationsMap(root: unknown): Record<string, unknown> | undefined {
  if (!isRecord(root) || !isRecord(root.conversations)) return undefined
  return root.conversations
}

async function matchesWorkspaceUris(workspaceUris: string[], normalizedCwd: string): Promise<boolean> {
  for (const uri of workspaceUris) {
    const rawPath = uriToPath(uri)
    if (!rawPath) continue
    const resolvedUriPath = await canonicalPath(rawPath)
    if (resolvedUriPath === normalizedCwd) return true
  }
  return false
}

function uriToPath(uri: string): string | undefined {
  const trimmed = uri.trim()
  if (!trimmed) return undefined
  if (trimmed.startsWith('file://')) {
    try { return fileURLToPath(trimmed) }
    catch { return undefined }
  }
  return trimmed
}

function summarizeConversation(
  conversation: MetadataConversation & { cwd: string },
  query: string,
  searchScope: CatalogSearchScope,
  contentHits: ClaudeSessionSearchHit[]
): ClaudeSessionSummary {
  const hits = searchScope === 'all' ? contentHits.slice(0, 4) : []
  const contentMatchCount = searchScope === 'all' ? contentHits.length : 0
  const metadataMatch = Boolean(query && normalizeQuery([
    conversation.title,
    conversation.providerSessionId,
    conversation.cwd,
    'antigravity'
  ].join(' ')).includes(query))
  const matchCount = contentMatchCount + (metadataMatch ? 1 : 0)
  return {
    provider: 'antigravity',
    providerSessionId: conversation.providerSessionId,
    title: conversation.title,
    cwd: conversation.cwd,
    updatedAt: conversation.updatedAt,
    permissionMode: 'default',
    eventCount: conversation.stepCount,
    matchCount,
    hits,
    availability: 'available'
  }
}

async function readIndexedEvents(
  filePath: string,
  refs: IndexedCatalogEvent[],
  query: string
): Promise<ClaudeSessionPreviewEvent[]> {
  if (refs.length === 0) return []
  const rangeStart = refs[0]!.offset
  const rangeEnd = refs.at(-1)!.offset + refs.at(-1)!.length
  const buffer = Buffer.allocUnsafe(rangeEnd - rangeStart)
  const handle = await open(filePath, 'r')
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
        const event = eventFromAntigravityRow(ref.index, row)
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

function eventFromAntigravityRow(
  index: number,
  row: Record<string, unknown>
): Omit<ClaudeSessionPreviewEvent, 'matched'> | undefined {
  const timestamp = parseTimestamp(row.created_at) ?? parseTimestamp(row.timestamp)
  const type = typeof row.type === 'string' ? row.type : ''
  const source = typeof row.source === 'string' ? row.source : ''
  if (type === 'USER_INPUT' || source === 'USER_EXPLICIT') {
    const text = typeof row.content === 'string' ? row.content.trim() : ''
    if (!text) return undefined
    return {
      index,
      kind: 'user',
      role: 'user',
      ...(timestamp === undefined ? {} : { timestamp }),
      text
    }
  }
  if (type === 'PLANNER_RESPONSE' || source === 'MODEL') {
    const parts: Array<{ kind: 'text' | 'tool'; text: string; toolName?: string }> = []
    if (typeof row.content === 'string' && row.content.trim()) {
      parts.push({ kind: 'text', text: row.content.trim() })
    }
    if (Array.isArray(row.tool_calls)) {
      for (const item of row.tool_calls) {
        if (!isRecord(item) || typeof item.name !== 'string' || !item.name.trim()) continue
        const argsText = typeof item.arguments === 'string'
          ? item.arguments.trim()
          : item.arguments === undefined ? '' : safeJson(item.arguments)
        parts.push({
          kind: 'tool',
          toolName: item.name,
          text: `${item.name} ${argsText}`.trim()
        })
      }
    }
    if (parts.length === 0) return undefined
    const tool = parts.find((part) => part.kind === 'tool')
    const kind: ClaudeSessionEventKind = tool ? 'tool' : 'assistant'
    return {
      index,
      kind,
      role: 'assistant',
      ...(timestamp === undefined ? {} : { timestamp }),
      text: parts.map(({ text }) => text).join('\n').trim(),
      ...(tool?.toolName ? { toolName: tool.toolName } : {})
    }
  }
  return undefined
}

function requireAntigravitySessionId(value: string): void {
  if (!isProviderSessionId(value)) throw new Error(runtimeMessages().session.antigravitySessionIdInvalid)
}
