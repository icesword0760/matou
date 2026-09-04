import { readFile, readdir, realpath, stat } from 'node:fs/promises'
import { resolve } from 'node:path'

import type {
  ClaudeSessionDetail,
  ClaudeSessionEventKind,
  ClaudeSessionListResult,
  ClaudeSessionPermissionMode,
  ClaudeSessionPreviewEvent,
  ClaudeSessionSearchHit,
  ClaudeSessionSummary
} from '@matou/contracts'

interface CatalogQuery {
  cwd: string
  query: string
}

type CatalogSearchScope = 'metadata' | 'all'

interface ParsedTranscript {
  providerSessionId: string
  title: string
  autoTitle?: string
  cwd: string
  updatedAt: number
  model?: string
  permissionMode: ClaudeSessionPermissionMode
  events: Omit<ClaudeSessionPreviewEvent, 'matched'>[]
}

export class ClaudeSessionCatalog {
  readonly #projectsRoot: string
  readonly #fileCache = new Map<string, {
    mtimeMs: number
    size: number
    transcript: ParsedTranscript | undefined
  }>()

  constructor(projectsRoot: string) {
    this.#projectsRoot = resolve(projectsRoot)
  }

  async list(input: CatalogQuery & { limit?: number; searchScope?: CatalogSearchScope }): Promise<ClaudeSessionListResult> {
    const transcripts = await this.#readWorkspace(input.cwd)
    const query = normalizeQuery(input.query)
    const searchScope = input.searchScope ?? 'all'
    const sessions = transcripts
      .map((transcript) => summarize(transcript, query, searchScope))
      .filter((session) => !query || session.matchCount > 0)
      .sort((left, right) => right.updatedAt - left.updatedAt || left.title.localeCompare(right.title))
    const limit = Math.max(1, Math.min(input.limit ?? 100, 500))
    return { sessions: sessions.slice(0, limit), total: sessions.length }
  }

  async detail(input: CatalogQuery & { providerSessionId: string; previewLimit?: number }): Promise<ClaudeSessionDetail> {
    requireProviderSessionId(input.providerSessionId)
    const transcript = (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
    if (!transcript) throw new Error('Claude Code 会话不存在或不属于当前工作空间')
    const query = normalizeQuery(input.query)
    const summary = summarize(transcript, '', 'metadata')
    const previewLimit = input.previewLimit === undefined
      ? undefined
      : Math.max(1, Math.min(input.previewLimit, 1_000))
    const hits: ClaudeSessionSearchHit[] = []
    let matchCount = 0
    const events: ClaudeSessionDetail['events'] = []
    if (query) {
      for (const event of transcript.events) {
        const matched = searchableEventText(event).includes(query)
        if (matched) {
          matchCount += 1
          if (hits.length < 4) {
            hits.push({ eventIndex: event.index, kind: event.kind, excerpt: excerpt(event.text, query) })
          }
        }
        if (previewLimit === undefined || matched && matchCount <= previewLimit) {
          events.push({ ...event, matched })
        }
      }
    } else {
      const previewSource = previewLimit === undefined
        ? transcript.events
        : transcript.events.slice(-previewLimit)
      events.push(...previewSource.map((event) => ({ ...event, matched: false })))
    }
    return {
      ...summary,
      matchCount,
      hits,
      events
    }
  }

  async autoTitle(input: { cwd: string; providerSessionId: string }): Promise<string | undefined> {
    requireProviderSessionId(input.providerSessionId)
    return (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
      ?.autoTitle
  }

  async #readWorkspace(cwd: string): Promise<ParsedTranscript[]> {
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
    const transcripts = await Promise.all(candidates.map(async ({ directory, name }) => {
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
          transcript = parseTranscript(providerSessionId, await readFile(path, 'utf8'))
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
    }))
    return [...new Map(transcripts
      .filter((value): value is ParsedTranscript => value !== undefined)
      .map((value) => [value.providerSessionId, value])).values()]
  }
}

export function encodeClaudeProjectPath(cwd: string): string {
  return resolve(cwd).replace(/[^A-Za-z0-9]/g, '-')
}

function parseTranscript(providerSessionId: string, source: string): ParsedTranscript | undefined {
  let cwd = ''
  let title = ''
  let autoTitle = ''
  let updatedAt = 0
  let model: string | undefined
  let permissionMode: ClaudeSessionPermissionMode = 'default'
  const events: Omit<ClaudeSessionPreviewEvent, 'matched'>[] = []
  for (const line of source.split(/\r?\n/)) {
    if (!line.trim()) continue
    let row: Record<string, unknown>
    try {
      const parsed = JSON.parse(line) as unknown
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
    events.push(event)
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
    events
  }
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
  transcript: ParsedTranscript,
  query: string,
  searchScope: CatalogSearchScope
): ClaudeSessionSummary {
  const hits: ClaudeSessionSearchHit[] = []
  let contentMatchCount = 0
  if (query && searchScope === 'all') {
    for (const event of transcript.events) {
      const text = searchableEventText(event)
      if (!text.includes(query)) continue
      contentMatchCount += 1
      if (hits.length < 4) {
        hits.push({ eventIndex: event.index, kind: event.kind, excerpt: excerpt(event.text, query) })
      }
    }
  }
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
