import { readdir, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import type { ClaudeSessionDetail, ClaudeSessionListResult, ClaudeSessionPreviewEvent, ClaudeSessionSearchHit, ClaudeSessionSearchResult, ClaudeSessionSummary } from '@matou/contracts'
import { EntityMissingError } from '../errors'
import { runtimeMessages } from '../i18n/messages'
import { canonicalPath, clampInteger, DEFAULT_EVENT_PAGE_LIMIT, excerpt, isMissing, isRecord, MAX_EVENT_PAGE_LIMIT, MAX_SEARCH_PAGE_LIMIT, MAX_SESSION_LIST_PAGE_LIMIT, normalizeQuery, selectEventWindow, type CatalogQuery, type CatalogSearchScope } from './session-catalog-utils'
import { readDshLines, scanDshLog, type DshLineReference } from './dsh-session-log-reader'

export function defaultDshSessionRoot(): string { return resolve(process.env.DSH_TUI_SESSION_ROOT || join(process.env.DSH_HOME || join(homedir(), '.dsh'), 'sessions')) }
export function isDshSessionId(value: string): boolean { return /^[A-Za-z0-9][A-Za-z0-9._-]{0,159}$/.test(value) }
/** The native key uses UTF-16 code units, including both halves of non-BMP characters. */
export function encodeDshProjectPath(cwd: string): string {
  let value = '', separator = false
  for (let at = 0; at < cwd.length; at++) {
    const char = cwd[at]!
    if (/[\\/:]/.test(char)) { if (!separator) value += '-'; separator = true }
    else { value += /^[A-Za-z0-9._-]$/.test(char) ? char : `~${cwd.charCodeAt(at).toString(16).toUpperCase().padStart(4, '0')}`; separator = false }
  }
  return `--${(value.replace(/^-+/, '') || 'root').slice(0, 251)}--`
}
interface EventRef extends DshLineReference { index: number; seq: number; kind: ClaudeSessionPreviewEvent['kind']; role?: 'user' | 'assistant'; timestamp?: number; toolName?: string }
interface Index { path: string; fingerprint: string; id: string; cwd: string; updatedAt: number; createdAt: number; title?: string; model?: string; events: EventRef[] }
const SURFACE = new Set(['system/message', 'developer/message', 'user/message', 'assistant/message', 'tool/result'])
const KNOWN = new Set(('turn/start turn/end step/start step/end user/message developer/message system/message assistant/message assistant/attempt tool/call tool/result request/header request/context session/end-seed ' +
  'agent-preset/selected agent/inbox/spliced approval/asked approval/decided approval/policy command/done command/run compaction/end compaction/prune compaction/start compaction/summary deliverables/presented feedback/message-delete feedback/message-put feedback/record goal/change image/offload job/registered job/settled llm/retry llm/retry-started model/selection permission/preset plan/mode sandbox/mode schedule/change session-log-deepseek/delivery-accepted session/title session/title-llm-request subagent/catalog subagent/descriptor subagent/model-selection-policy team/member team/message/delivered team/message/queued team/task todo/write tool-workflow/agent-end tool-workflow/agent-start tool-workflow/run-end tool-workflow/run-start tool/ptc-dispatch tool/ptc-dispatch-start web/deepseek-search-llm-request working-directory/change workspace/changes').split(' '))
const object = (value: unknown): Record<string, unknown> => isRecord(value) ? value : {}
const text = (value: unknown): string | undefined => typeof value === 'string' && value.trim() ? value.trim() : undefined
class DshWorkspaceMismatch extends Error {}
const invalid = () => new Error(runtimeMessages().session.dshSessionCorrupt)

/** File references only; no persisted index or cached conversation bodies. */
export class DshSessionCatalog {
  readonly #root: string
  readonly #indexes = new Map<string, Index>()
  readonly #searches = new Map<string, { fingerprint: string; query: string; hits: ClaudeSessionSearchHit[] }>()
  constructor(root = defaultDshSessionRoot()) { this.#root = resolve(root) }

  async list(input: CatalogQuery & { offset?: number; limit?: number; searchScope?: CatalogSearchScope; titleForSession?(id: string): string | undefined }): Promise<ClaudeSessionListResult> {
    const cwd = await canonicalPath(input.cwd), query = normalizeQuery(input.query), sessions: ClaudeSessionSummary[] = []
    for (const path of await this.#paths(input.cwd, cwd)) {
      let index: Index
      try { index = await this.#index(path, cwd) } catch (error) { if (error instanceof DshWorkspaceMismatch) continue; throw error }
      const title = input.titleForSession?.(index.id) ?? titleOf(index)
      const metadataMatch = Boolean(query && normalizeQuery(`${title} ${index.id}`).includes(query))
      const hits = query && input.searchScope !== 'metadata' ? await this.#search(index, query) : []
      if (query && !metadataMatch && !hits.length) continue
      sessions.push({ ...summary(index, title), matchCount: (metadataMatch ? 1 : 0) + hits.length, hits: hits.slice(0, 4) })
    }
    sessions.sort((a, b) => b.updatedAt - a.updatedAt || a.providerSessionId.localeCompare(b.providerSessionId))
    const offset = clampInteger(input.offset ?? 0, 0, sessions.length), limit = clampInteger(input.limit ?? 50, 1, MAX_SESSION_LIST_PAGE_LIMIT)
    const page = sessions.slice(offset, offset + limit)
    return { provider: 'dsh', sessions: page, total: sessions.length, offset, limit, nextOffset: offset + page.length, hasMore: offset + page.length < sessions.length }
  }
  async detail(input: CatalogQuery & { providerSessionId: string; beforeEventIndex?: number; aroundEventIndex?: number; limit?: number }): Promise<ClaudeSessionDetail> {
    const index = await this.#selected(input.cwd, input.providerSessionId), query = normalizeQuery(input.query)
    let limit = clampInteger(input.limit ?? DEFAULT_EVENT_PAGE_LIMIT, 1, MAX_EVENT_PAGE_LIMIT)
    let selected = selectEventWindow(index.events, { ...input, limit })
    while (selected.length > 1 && selected.reduce((sum, ref) => sum + ref.length, 0) > 8 * 1024 * 1024) {
      limit = Math.max(1, Math.floor(limit / 2)); selected = selectEventWindow(index.events, { ...input, limit })
    }
    const events = await this.#events(index, selected, query), hits = query ? await this.#search(index, query) : []
    const start = selected[0]?.index ?? 0, end = selected.at(-1)?.index ?? 0
    return { ...summary(index, titleOf(index)), matchCount: hits.length, hits: hits.slice(0, 4), events,
      page: { startEventIndex: start, endEventIndex: end, total: index.events.length, hasEarlier: start > 1, hasLater: end > 0 && end < index.events.length } }
  }
  async search(input: CatalogQuery & { providerSessionId: string; offset?: number; limit?: number }): Promise<ClaudeSessionSearchResult> {
    const index = await this.#selected(input.cwd, input.providerSessionId), query = normalizeQuery(input.query)
    const hits = query ? await this.#search(index, query) : []
    const offset = clampInteger(input.offset ?? 0, 0, hits.length), limit = clampInteger(input.limit ?? 100, 1, MAX_SEARCH_PAGE_LIMIT), page = hits.slice(offset, offset + limit)
    return { query, hits: page, total: hits.length, offset, limit, nextOffset: offset + page.length, hasMore: offset + page.length < hits.length }
  }
  async #selected(cwd: string, id: string): Promise<Index> {
    if (!isDshSessionId(id)) throw new Error(runtimeMessages().session.dshSessionIdInvalid)
    const canonical = await canonicalPath(cwd)
    for (const path of await this.#paths(cwd, canonical, id)) {
      try {
        const entry = await this.#index(path, canonical)
        if (entry.id === id && entry.cwd === canonical) return entry
      } catch (error) { if (!(error instanceof DshWorkspaceMismatch)) throw error }
    }
    throw new EntityMissingError(runtimeMessages().session.dshSessionNotInWorkspace)
  }
  async #paths(original: string, canonical: string, selectedId?: string): Promise<string[]> {
    const paths: string[] = []
    for (const key of new Set([encodeDshProjectPath(resolve(original)), encodeDshProjectPath(canonical)])) {
      const base = join(this.#root, key)
      let ids: string[]
      try { ids = selectedId ? [selectedId] : (await readdir(base, { withFileTypes: true })).filter(row => row.isDirectory() && isDshSessionId(row.name)).map(row => row.name) }
      catch (error) { if (isMissing(error)) continue; throw error }
      for (const id of ids) {
        for (const suffix of ['.zstd', '']) {
          const path = join(base, id, 'session.v4.jsonl' + suffix)
          try { if ((await stat(path)).isFile()) { paths.push(path); break } }
          catch (error) { if (!isMissing(error)) throw error }
        }
      }
    }
    return [...new Set(paths)]
  }
  async #index(path: string, expectedCwd: string): Promise<Index> {
    const file = await stat(path), fingerprint = `${file.size}:${file.mtimeMs}`
    const cached = this.#indexes.get(path)
    if (cached?.fingerprint === fingerprint) { if (cached.cwd !== expectedCwd) throw new DshWorkspaceMismatch(); this.#indexes.delete(path); this.#indexes.set(path, cached); return cached }
    let index: Index | undefined, previousSeq = -1
    const surface: EventRef[] = []
    for await (const line of scanDshLog(path)) {
      const row = line.row, data = object(row.data)
      if (!index) {
        if (row.type !== 'session' || row.version !== 4 || !isDshSessionId(String(row.id)) || row.id !== basename(dirname(path)) || !text(row.cwd) || typeof row.createdAt !== 'number' || !Number.isFinite(row.createdAt) || row.createdAt < 0 || row.createdAt > 8.64e15) throw invalid()
        index = { path, fingerprint, id: String(row.id), cwd: await canonicalPath(row.cwd as string), updatedAt: file.mtimeMs, createdAt: row.createdAt, events: [] }
        if (index.cwd !== expectedCwd) throw new DshWorkspaceMismatch()
        continue
      }
      if (!Number.isSafeInteger(row.seq) || (row.seq as number) <= previousSeq || typeof row.type !== 'string') throw invalid()
      previousSeq = row.seq as number
      if (!KNOWN.has(row.type)) { if (row.ignorable === true) continue; throw new Error(runtimeMessages().session.dshSessionUnsupportedEvent) }
      if (row.type === 'session/title' && text(data.title)) index.title = text(data.title)!.slice(0, 4096)
      const model = row.type === 'model/selection' ? text(data.model)
        : row.type === 'request/header' ? text(object(object(data.header).config).model) : undefined
      if (model) index.model = model
      if (!SURFACE.has(row.type)) continue
      const ref: EventRef = { ...(line.frameStart === undefined ? {} : { frameStart: line.frameStart, frameEnd: line.frameEnd! }), offset: line.offset, length: line.length,
        index: 0, seq: row.seq as number, kind: row.type === 'tool/result' ? 'tool' : row.type === 'user/message' ? 'user' : row.type === 'assistant/message' ? 'assistant' : 'system',
        ...(row.type === 'user/message' ? { role: 'user' as const } : row.type === 'assistant/message' ? { role: 'assistant' as const } : {}),
        ...(typeof row.time === 'number' && Number.isFinite(row.time) ? { timestamp: row.time } : {}),
        ...(text(object(data.message).toolName) ? { toolName: text(object(data.message).toolName)! } : {}) }
      if (row.surfaceOp === 'append') surface.push(ref)
      else {
        const op = object(row.surfaceOp)
        if (op.op !== 'replace' || !Number.isSafeInteger(op.startSeq) || !Number.isSafeInteger(op.endSeq)) throw invalid()
        const start = surface.findIndex(item => item.seq === op.startSeq), end = surface.findIndex(item => item.seq === op.endSeq)
        if (start < 0 || end < start || !Array.isArray(row.sourceEventSeqs) || surface.slice(start, end + 1).some(item => !(row.sourceEventSeqs as unknown[]).includes(item.seq))) throw invalid()
        surface.splice(start, end - start + 1, ref)
      }
      if (surface.length > 100_000) throw new Error(runtimeMessages().session.dshSessionReadLimit)
    }
    if (!index) throw invalid()
    index.events = surface.map((ref, at) => ({ ...ref, index: at + 1 }))
    this.#indexes.delete(path); this.#indexes.set(path, index)
    while (this.#indexes.size > 16 || [...this.#indexes.values()].reduce((sum, value) => sum + value.events.length, 0) > 200_000) this.#indexes.delete(this.#indexes.keys().next().value!)
    return index
  }
  async #events(index: Index, refs: EventRef[], query: string): Promise<ClaudeSessionPreviewEvent[]> {
    const rows = await readDshLines(index.path, refs)
    return refs.map((ref, at) => {
      const content = eventText(rows[at]!)
      return { index: ref.index, kind: ref.kind, ...(ref.role ? { role: ref.role } : {}), ...(ref.timestamp === undefined ? {} : { timestamp: ref.timestamp }),
        ...(ref.toolName ? { toolName: ref.toolName } : {}), text: content.length > 100_000 ? content.slice(0, 100_000) + `\n[${runtimeMessages().session.dshPreviewShortened}]` : content,
        matched: Boolean(query && normalizeQuery(`${ref.toolName ?? ''} ${content}`).includes(query)) }
    })
  }
  async #search(index: Index, query: string): Promise<ClaudeSessionSearchHit[]> {
    const cached = this.#searches.get(index.path)
    if (cached?.fingerprint === index.fingerprint && cached.query === query) return cached.hits
    const hits: ClaudeSessionSearchHit[] = []
    for (const refs of eventBatches(index.events)) {
      const rows = await readDshLines(index.path, refs)
      for (let at = 0; at < rows.length; at++) {
        const content = eventText(rows[at]!), ref = refs[at]!
        if (normalizeQuery(`${ref.toolName ?? ''} ${content}`).includes(query)) hits.push({ eventIndex: ref.index, kind: ref.kind, excerpt: excerpt(content, query) })
      }
    }
    this.#searches.delete(index.path); this.#searches.set(index.path, { fingerprint: index.fingerprint, query, hits })
    while (this.#searches.size > 16 || [...this.#searches.values()].reduce((sum, entry) => sum + entry.hits.length, 0) > 200_000) this.#searches.delete(this.#searches.keys().next().value!)
    return hits
  }
}
function titleOf(index: Index): string { return index.title ?? `${new Date(index.createdAt).toISOString()} · ${index.id.slice(0, 8)}` }
function summary(index: Index, title: string): ClaudeSessionSummary {
  return { provider: 'dsh', providerSessionId: index.id, cwd: index.cwd, title, updatedAt: index.updatedAt,
    ...(index.model ? { model: index.model } : {}), permissionMode: 'default', eventCount: index.events.length, matchCount: 0, hits: [], availability: 'available' }
}
function contentText(value: unknown): string {
  if (typeof value === 'string') return value
  if (!Array.isArray(value)) return ''
  return value.flatMap(item => { const block = object(item); return typeof block.text === 'string' ? [block.text] : [] }).join('\n')
}
function eventText(row: Record<string, unknown>): string {
  const data = object(row.data), message = object(data.message)
  return contentText(message.content ?? data.content ?? data.text)
}

function* eventBatches(refs: EventRef[]): Generator<EventRef[]> {
  let batch: EventRef[] = [], size = 0
  for (const ref of refs) {
    if (batch.length && (batch.length >= 32 || size + ref.length > 4 * 1024 * 1024)) { yield batch; batch = []; size = 0 }
    batch.push(ref); size += ref.length
  }
  if (batch.length) yield batch
}
