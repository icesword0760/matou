import { open, readdir, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import type {
  ClaudeSessionDetail, ClaudeSessionListResult, ClaudeSessionPreviewEvent,
  ClaudeSessionSearchHit, ClaudeSessionSearchResult, ClaudeSessionSummary
} from '@matou/contracts'
import { EntityMissingError } from '../errors'
import { runtimeMessages } from '../i18n/messages'
import {
  canonicalPath, clampInteger, DEFAULT_EVENT_PAGE_LIMIT, excerpt, indexedEventBatches,
  isMissing, isRecord, MAX_EVENT_PAGE_LIMIT, MAX_SEARCH_PAGE_LIMIT, MAX_SESSION_LIST_PAGE_LIMIT,
  normalizeQuery, parseTimestamp, scanJsonLines, SEARCH_CACHE_LIMIT, searchableEventText,
  selectEventWindow, type CatalogQuery, type CatalogSearchScope, type IndexedCatalogEvent
} from './session-catalog-utils'

interface HistoryBase { threadId: string; endOrdinal?: number; endOffset?: number }
interface CodexMetadata { historyBase?: HistoryBase; path: string; providerSessionId: string; cwd: string; updatedAt: number; size: number; mtimeMs: number; createdAt?: number; forkedFromId?: string }
interface CodexEventRef extends IndexedCatalogEvent { path: string; ordinal?: number; warning?: string; sortAt: number; id?: string; turn?: string; structured: boolean }
interface CodexIndex { fingerprint: string; path: string; size: number; mtimeMs: number; model?: string; events: CodexEventRef[]; hasCompletedTurn: boolean }
interface IndexTitle { title: string; updatedAt: number }
export function defaultCodexRoot(): string { return resolve(process.env.CODEX_HOME || join(homedir(), '.codex')) }
export function isCodexSessionId(value: string): boolean { return /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(value) }

/** Reads provider files only. Bodies are indexed on demand; caches contain offsets, never transcripts. */
export class CodexSessionCatalog {
  readonly #root: string
  readonly #metadata = new Map<string, CodexMetadata>()
  readonly #indexes = new Map<string, CodexIndex>()
  readonly #searches = new Map<string, { fingerprint: string; hits: ClaudeSessionSearchHit[] }>()
  constructor(root = defaultCodexRoot()) { this.#root = resolve(root) }

  async list(input: CatalogQuery & { offset?: number; limit?: number; searchScope?: CatalogSearchScope;
    titleForSession?(id: string): string | undefined }): Promise<ClaudeSessionListResult> {
    const entries = await this.#workspace(input.cwd)
    const titles = await this.#titles()
    const query = normalizeQuery(input.query)
    const archive = query && input.searchScope !== 'metadata' ? await this.#workspace() : undefined
    const sessions: ClaudeSessionSummary[] = []
    // Sequential full-text reads bound memory and I/O fan-out for large archives.
    for (const entry of entries) {
      const named = titles.get(entry.providerSessionId)
      const title = input.titleForSession?.(entry.providerSessionId) ?? named?.title ?? fallbackTitle(entry)
      const metadataMatch = Boolean(query && normalizeQuery(`${title} ${entry.providerSessionId}`).includes(query))
      const hits = query && input.searchScope !== 'metadata' ? await this.#search(await this.#historyIndex(entry, archive), query) : []
      if (query && !metadataMatch && !hits.length) continue
      sessions.push({ ...summary(entry, title, named), matchCount: (metadataMatch ? 1 : 0) + hits.length, hits: hits.slice(0, 4) })
    }
    sessions.sort((a, b) => b.updatedAt - a.updatedAt || a.providerSessionId.localeCompare(b.providerSessionId))
    const offset = clampInteger(input.offset ?? 0, 0, sessions.length)
    const limit = clampInteger(input.limit ?? 50, 1, MAX_SESSION_LIST_PAGE_LIMIT)
    const page = sessions.slice(offset, offset + limit)
    return { provider: 'codex', sessions: page, total: sessions.length, offset, limit,
      nextOffset: offset + page.length, hasMore: offset + page.length < sessions.length }
  }

  async detail(input: CatalogQuery & { providerSessionId: string; beforeEventIndex?: number; aroundEventIndex?: number; limit?: number }): Promise<ClaudeSessionDetail> {
    const entry = await this.#selected(input.cwd, input.providerSessionId)
    const index = await this.#historyIndex(entry)
    const named = (await this.#titles()).get(entry.providerSessionId)
    const query = normalizeQuery(input.query)
    const hits = query ? await this.#search(index, query) : []
    const selected = selectEventWindow(index.events, { ...input,
      limit: clampInteger(input.limit ?? DEFAULT_EVENT_PAGE_LIMIT, 1, MAX_EVENT_PAGE_LIMIT) })
    const events = await this.#readEvents(selected, query)
    const start = selected[0]?.index ?? 0; const end = selected.at(-1)?.index ?? 0
    return { ...summary(entry, named?.title ?? fallbackTitle(entry), named),
      ...(index.model ? { model: index.model } : {}), eventCount: index.events.length,
      matchCount: hits.length, hits: hits.slice(0, 4), events,
      page: { startEventIndex: start, endEventIndex: end, total: index.events.length,
        hasEarlier: start > 1, hasLater: end > 0 && end < index.events.length } }
  }

  async search(input: CatalogQuery & { providerSessionId: string; offset?: number; limit?: number }): Promise<ClaudeSessionSearchResult> {
    const entry = await this.#selected(input.cwd, input.providerSessionId)
    const query = normalizeQuery(input.query)
    const hits = query ? await this.#search(await this.#historyIndex(entry), query) : []
    const offset = clampInteger(input.offset ?? 0, 0, hits.length)
    const limit = clampInteger(input.limit ?? 100, 1, MAX_SEARCH_PAGE_LIMIT)
    const page = hits.slice(offset, offset + limit)
    return { query, hits: page, total: hits.length, offset, limit, nextOffset: offset + page.length,
      hasMore: offset + page.length < hits.length }
  }

  async hasCompletedTurn(input: { cwd: string; providerSessionId: string }): Promise<boolean> {
    return (await this.#index(await this.#selected(input.cwd, input.providerSessionId))).hasCompletedTurn
  }

  /** Session creation metadata, never file recency, identifies this launch's children. */
  async forkCandidates(input: { cwd: string; parentProviderSessionId: string; startedAt: number }): Promise<{ providerSessionId: string; path: string }[]> {
    return (await this.#workspace(input.cwd))
      .filter(entry => entry.createdAt !== undefined && entry.createdAt > input.startedAt &&
        entry.forkedFromId === input.parentProviderSessionId && entry.providerSessionId !== input.parentProviderSessionId)
      .map(({ providerSessionId, path }) => ({ providerSessionId, path }))
  }

  async #selected(cwd: string, id: string): Promise<CodexMetadata> {
    if (!isCodexSessionId(id)) throw new Error(runtimeMessages().session.codexSessionIdInvalid)
    const entry = (await this.#workspace(cwd, id)).find(item => item.providerSessionId === id)
    if (!entry) throw new EntityMissingError(runtimeMessages().session.codexSessionNotInWorkspace)
    return entry
  }

  async #workspace(cwd?: string, selectedId?: string): Promise<CodexMetadata[]> {
    const canonical = cwd === undefined ? undefined : await canonicalPath(cwd)
    const entries = new Map<string, CodexMetadata>()
    try {
      for await (const path of rolloutPaths(join(this.#root, 'sessions'))) {
        const file = await stat(path)
        let entry = this.#metadata.get(path)
        if (!entry || entry.size !== file.size || entry.mtimeMs !== file.mtimeMs) {
          entry = undefined
          // session_meta is the first record in a rollout; no body scan for listing.
          for await (const line of scanJsonLines(path)) {
            if (!line.length) continue
            const row = parseRow(line.source, path)
            const payload = isRecord(row.payload) ? row.payload : {}
            if (row.type !== 'session_meta' || typeof payload.id !== 'string' || !isCodexSessionId(payload.id) || typeof payload.cwd !== 'string') {
              throw corrupt(path)
            }
            entry = { path, providerSessionId: payload.id, cwd: await canonicalPath(payload.cwd),
              updatedAt: file.mtimeMs, size: file.size, mtimeMs: file.mtimeMs,
              ...(parseTimestamp(payload.timestamp) === undefined ? {} : { createdAt: parseTimestamp(payload.timestamp)! }),
              ...(typeof payload.forked_from_id === 'string' ? { forkedFromId: payload.forked_from_id } : {}),
              ...(historyBase(payload.history_base) ? { historyBase: historyBase(payload.history_base)! } : {}) }
            break
          }
          if (!entry) throw corrupt(path)
          cachePut(this.#metadata, path, entry, 256)
        }
        if ((canonical !== undefined && entry.cwd !== canonical) || (selectedId && entry.providerSessionId !== selectedId)) continue
        const previous = entries.get(entry.providerSessionId)
        if (!previous || previous.mtimeMs < entry.mtimeMs) entries.set(entry.providerSessionId, entry)
      }
    } catch (error) { throw readableError(error) }
    return [...entries.values()]
  }

  async #titles(): Promise<Map<string, IndexTitle>> {
    const titles = new Map<string, IndexTitle>()
    try {
      for await (const line of scanJsonLines(join(this.#root, 'session_index.jsonl'))) {
        let row: unknown
        try { row = JSON.parse(line.source.toString('utf8')) } catch { continue }
        if (!isRecord(row) || typeof row.id !== 'string' || !isCodexSessionId(row.id) || typeof row.thread_name !== 'string' || !row.thread_name.trim()) continue
        const updatedAt = parseTimestamp(row.updated_at)
        if (updatedAt === undefined || (titles.get(row.id)?.updatedAt ?? -Infinity) > updatedAt) continue
        titles.set(row.id, { title: row.thread_name.trim(), updatedAt })
      }
    } catch (error) { if (!isMissing(error)) throw readableError(error) }
    return titles
  }

  async #index(entry: CodexMetadata): Promise<CodexIndex> {
    const cached = this.#indexes.get(entry.path)
    if (cached?.size === entry.size && cached.mtimeMs === entry.mtimeMs) return cached
    const candidates: CodexEventRef[] = []
    const structuredIds = new Set<string>(); const structuredUsers = new Set<string>()
    const turns = new Map<string, { user: boolean; final: boolean; failed: boolean }>()
    let currentTurn = ''; let model: string | undefined; let hasCompletedTurn = false
    try {
      for await (const line of scanJsonLines(entry.path)) {
        if (!line.length) continue
        let row: Record<string, unknown>
        try { row = parseRow(line.source, entry.path) } catch (error) {
          // A live writer may not have finished the final JSON record yet.
          if (line.offset + line.length >= entry.size) continue
          throw error
        }
        const payload = isRecord(row.payload) ? row.payload : {}
        if (row.type === 'turn_context' && typeof payload.model === 'string') model = payload.model
        if (typeof payload.turn_id === 'string') currentTurn = payload.turn_id
        const turn = turns.get(currentTurn) ?? { user: false, final: false, failed: false }
        if (row.type === 'event_msg' && ['turn_aborted', 'task_failed', 'error'].includes(String(payload.type))) turn.failed = true
        const item = isRecord(payload.item) ? payload.item : {}
        if (row.type === 'event_msg' && payload.type === 'item_completed') {
          if (item.type === 'UserMessage') turn.user = true
          if (item.type === 'AgentMessage' && item.phase === 'final_answer' && contentText(item.content)) turn.final = true
        }
        if (row.type === 'event_msg' && payload.type === 'task_complete' && turn.user && turn.final && !turn.failed && !payload.error) hasCompletedTurn = true
        if (currentTurn) turns.set(currentTurn, turn)
        const event = eventFromRow(row)
        if (!event) continue
        const structured = row.type === 'event_msg'
        const id = typeof item.id === 'string' ? item.id : typeof payload.id === 'string' ? payload.id : undefined
        if (structured && id) {
          if (structuredIds.has(id)) continue
          structuredIds.add(id)
        }
        if (structured && event.role === 'user') structuredUsers.add(currentTurn)
        candidates.push({ index: 0, path: entry.path, sortAt: event.timestamp ?? entry.createdAt ?? entry.updatedAt,
          ...(typeof row.ordinal === 'number' ? { ordinal: row.ordinal } : {}), kind: event.kind, ...(event.role ? { role: event.role } : {}),
          ...(event.timestamp === undefined ? {} : { timestamp: event.timestamp }),
          ...(event.toolName ? { toolName: event.toolName } : {}), offset: line.offset, length: line.length,
          structured, turn: currentTurn, ...(id ? { id } : {}) })
      }
    } catch (error) { throw readableError(error) }
    const seen = new Set<string>()
    const events = candidates.filter(ref => {
      if (!ref.structured && ((ref.id && structuredIds.has(ref.id)) || (ref.role === 'user' && structuredUsers.has(ref.turn ?? '')))) return false
      if (ref.id && seen.has(ref.id)) return false
      if (ref.id) seen.add(ref.id)
      return true
    }).map((ref, i) => ({ ...ref, index: i + 1 }))
    const index: CodexIndex = { fingerprint: `${entry.path}:${entry.size}:${entry.mtimeMs}`, path: entry.path, size: entry.size, mtimeMs: entry.mtimeMs, events, hasCompletedTurn,
      ...(model ? { model } : {}) }
    cachePut(this.#indexes, entry.path, index, SEARCH_CACHE_LIMIT)
    return index
  }

  /** Follow only explicit history_base references, including parents in other
   * Worktrees. No writes or durable format changes; all offsets stay file-local. */
  async #historyIndex(entry: CodexMetadata, archive?: CodexMetadata[]): Promise<CodexIndex> {
    const own = await this.#index(entry)
    if (!entry.historyBase) return own
    const byId = new Map((archive ?? await this.#workspace()).map(item => [item.providerSessionId, item]))
    const visited = new Set([entry.providerSessionId])
    let base: HistoryBase | undefined = entry.historyBase
    let events = [...own.events]
    const fingerprints = [own.fingerprint]
    let warning: string | undefined
    for (let depth = 0; base; depth++) {
      if (depth >= 5) { warning = runtimeMessages().session.codexHistoryDepthLimit; break }
      if (visited.has(base.threadId)) { warning = runtimeMessages().session.codexHistoryCycle; break }
      visited.add(base.threadId)
      const parent = byId.get(base.threadId)
      if (!parent) {
        warning = runtimeMessages().session.codexHistoryMissing
        events = [...own.events]
        break
      }
      let index: CodexIndex
      try { index = await this.#index(parent) }
      catch { warning = runtimeMessages().session.codexHistoryMissing; events = [...own.events]; break }
      fingerprints.push(`${index.fingerprint}:${base.endOrdinal ?? ''}:${base.endOffset ?? ''}`)
      const boundary = base
      const inherited = index.events.filter(ref =>
        (boundary.endOffset === undefined || ref.offset + ref.length <= boundary.endOffset) &&
        (boundary.endOrdinal === undefined || ref.ordinal === undefined || ref.ordinal < boundary.endOrdinal))
      events = [...inherited, ...events]
      base = parent.historyBase
    }
    // Copied/mirrored items can occur in both files. Keep the oldest copy of a
    // stable provider item id; messages with equal text but different ids remain.
    const seen = new Set<string>()
    events = events.filter(ref => {
      if (!ref.id) return true
      if (seen.has(ref.id)) return false
      seen.add(ref.id); return true
    }).sort((a, b) => a.sortAt - b.sortAt)
    if (warning) events.push({ index: 0, path: entry.path, offset: 0, length: 0,
      structured: true, sortAt: Infinity, kind: 'system', warning })
    return { ...own, fingerprint: `${fingerprints.join('|')}|${warning ?? ''}`,
      events: events.map((ref, i) => ({ ...ref, index: i + 1 })) }
  }

  async #readEvents(refs: IndexedCatalogEvent[], query: string): Promise<ClaudeSessionPreviewEvent[]> {
    const events: ClaudeSessionPreviewEvent[] = []
    const byPath = new Map<string, CodexEventRef[]>()
    for (const ref of refs as CodexEventRef[]) {
      if (ref.warning) {
        events.push({ index: ref.index, kind: 'system', text: ref.warning, matched: false })
      } else {
        const group = byPath.get(ref.path) ?? []; group.push(ref); byPath.set(ref.path, group)
      }
    }
    for (const [path, group] of byPath) {
      let file
      try {
        file = await open(path, 'r')
        for (const batch of indexedEventBatches(group.sort((a, b) => a.offset - b.offset))) {
          const start = batch[0]!.offset
          const end = batch.at(-1)!.offset + batch.at(-1)!.length
          const buffer = Buffer.alloc(end - start)
          const { bytesRead } = await file.read(buffer, 0, buffer.length, start)
          if (bytesRead !== buffer.length) throw corrupt(path)
          for (const ref of batch) {
            const event = eventFromRow(parseRow(buffer.subarray(ref.offset - start, ref.offset - start + ref.length), path))
            if (event) events.push({ ...event, index: ref.index, matched: Boolean(query && searchableEventText({ ...event, index: ref.index }).includes(query)) })
          }
        }
      } catch (error) { throw readableError(error) }
      finally { await file?.close() }
    }
    return events.sort((a, b) => a.index - b.index)
  }

  async #search(index: CodexIndex, query: string): Promise<ClaudeSessionSearchHit[]> {
    const key = `${index.path}\0${query}`
    const cached = this.#searches.get(key)
    if (cached?.fingerprint === index.fingerprint) return cached.hits
    const hits: ClaudeSessionSearchHit[] = []
    for (const batch of indexedEventBatches(index.events)) {
      for (const event of await this.#readEvents(batch, query)) {
        if (event.matched) hits.push({ eventIndex: event.index, kind: event.kind, excerpt: excerpt(event.text, query) })
      }
    }
    cachePut(this.#searches, key, { fingerprint: index.fingerprint, hits }, SEARCH_CACHE_LIMIT)
    return hits
  }
}

async function* rolloutPaths(directory: string): AsyncGenerator<string> {
  let entries
  try { entries = await readdir(directory, { withFileTypes: true }) }
  catch (error) { if (isMissing(error)) return; throw error }
  for (const entry of entries) {
    if (entry.isDirectory()) yield* rolloutPaths(join(directory, entry.name))
    else if (entry.isFile() && entry.name.startsWith('rollout-') && entry.name.endsWith('.jsonl')) yield join(directory, entry.name)
  }
}
function historyBase(value: unknown): HistoryBase | undefined {
  if (!isRecord(value) || typeof value.thread_id !== 'string') return undefined
  const bound = (n: unknown) => typeof n === 'number' && Number.isSafeInteger(n) && n >= 0 ? n : undefined
  const endOrdinal = bound(value.end_ordinal_exclusive), endOffset = bound(value.end_byte_offset)
  return { threadId: value.thread_id, ...(endOrdinal === undefined ? {} : { endOrdinal }),
    ...(endOffset === undefined ? {} : { endOffset }) }
}
function summary(entry: CodexMetadata, title: string, named?: IndexTitle): ClaudeSessionSummary {
  return { provider: 'codex', providerSessionId: entry.providerSessionId, title, cwd: entry.cwd,
    updatedAt: Math.max(entry.updatedAt, named?.updatedAt ?? 0), permissionMode: 'default',
    eventCount: 0, matchCount: 0, hits: [], availability: 'available' }
}
function fallbackTitle(entry: CodexMetadata): string { return `${new Date(entry.updatedAt).toISOString().slice(0, 16).replace('T', ' ')} · ${entry.providerSessionId}` }
function parseRow(source: Buffer, path: string): Record<string, unknown> {
  try { const row: unknown = JSON.parse(source.toString('utf8')); if (isRecord(row)) return row } catch { /* localized below */ }
  throw corrupt(path)
}
function corrupt(path: string): Error { return new Error(`${runtimeMessages().session.codexSessionCorrupt}: ${basename(path)}`) }
function readableError(error: unknown): Error {
  if (isRecord(error) && ['EACCES', 'EPERM'].includes(String(error.code))) return new Error(runtimeMessages().session.codexSessionPermissionDenied)
  return error instanceof Error ? error : new Error(String(error))
}
function cachePut<T>(cache: Map<string, T>, key: string, value: T, limit: number): void {
  cache.delete(key); cache.set(key, value)
  while (cache.size > limit) cache.delete(cache.keys().next().value!)
}
function contentText(content: unknown): string {
  if (typeof content === 'string') return content
  return Array.isArray(content) ? content.flatMap(part => isRecord(part) && typeof part.text === 'string' ? [part.text] : []).join('\n') : ''
}
function eventFromRow(row: Record<string, unknown>): Omit<ClaudeSessionPreviewEvent, 'index' | 'matched'> | undefined {
  const p = isRecord(row.payload) ? row.payload : {}
  const structured = row.type === 'event_msg' && p.type === 'item_completed'
  const item = structured && isRecord(p.item) ? p.item : p
  const timestamp = parseTimestamp(row.timestamp)
  const base = timestamp === undefined ? {} : { timestamp }
  const role = structured ? item.type === 'UserMessage' ? 'user' : item.type === 'AgentMessage' ? 'assistant' : undefined
    : row.type === 'response_item' && p.type === 'message' && ['user', 'assistant'].includes(String(p.role)) ? p.role as 'user' | 'assistant' : undefined
  if (role) {
    const text = contentText(item.content)
    if (!text || (role === 'user' && /^(# AGENTS\.md instructions|<environment_context>|<permissions instructions>)/.test(text.trimStart()))) return undefined
    return { ...base, role, kind: role, text }
  }
  if (structured && item.type === 'CommandExecution') {
    const command = Array.isArray(item.command) ? item.command.join(' ') : typeof item.command === 'string' ? item.command : ''
    return { ...base, kind: 'tool', toolName: 'Bash', text: [command, typeof item.aggregated_output === 'string' ? item.aggregated_output : ''].filter(Boolean).join('\n') }
  }
  return undefined
}
