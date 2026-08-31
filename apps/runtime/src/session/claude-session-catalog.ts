import { readFile, readdir, realpath } from 'node:fs/promises'
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

interface ParsedTranscript {
  providerSessionId: string
  title: string
  cwd: string
  updatedAt: number
  model?: string
  permissionMode: ClaudeSessionPermissionMode
  events: Omit<ClaudeSessionPreviewEvent, 'matched'>[]
}

export class ClaudeSessionCatalog {
  readonly #projectsRoot: string

  constructor(projectsRoot: string) {
    this.#projectsRoot = resolve(projectsRoot)
  }

  async list(input: CatalogQuery & { limit?: number }): Promise<ClaudeSessionListResult> {
    const transcripts = await this.#readWorkspace(input.cwd)
    const query = normalizeQuery(input.query)
    const sessions = transcripts
      .map((transcript) => summarize(transcript, query))
      .filter((session) => !query || session.matchCount > 0)
      .sort((left, right) => right.updatedAt - left.updatedAt || left.title.localeCompare(right.title))
    const limit = Math.max(1, Math.min(input.limit ?? 100, 500))
    return { sessions: sessions.slice(0, limit), total: sessions.length }
  }

  async detail(input: CatalogQuery & { providerSessionId: string }): Promise<ClaudeSessionDetail> {
    requireProviderSessionId(input.providerSessionId)
    const transcript = (await this.#readWorkspace(input.cwd))
      .find(({ providerSessionId }) => providerSessionId === input.providerSessionId)
    if (!transcript) throw new Error('Claude Code 会话不存在或不属于当前工作空间')
    const query = normalizeQuery(input.query)
    const summary = summarize(transcript, query)
    return {
      ...summary,
      events: transcript.events.map((event) => ({
        ...event,
        matched: Boolean(query && searchableEventText(event).includes(query))
      }))
    }
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
      try {
        const text = await readFile(resolve(directory, name), 'utf8')
        const transcript = parseTranscript(providerSessionId, text)
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
    title: title || '未命名 Claude 会话',
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

function summarize(transcript: ParsedTranscript, query: string): ClaudeSessionSummary {
  const hits: ClaudeSessionSearchHit[] = []
  if (query) {
    for (const event of transcript.events) {
      const text = searchableEventText(event)
      if (!text.includes(query)) continue
      hits.push({ eventIndex: event.index, kind: event.kind, excerpt: excerpt(event.text, query) })
    }
  }
  const metadataMatch = Boolean(query && normalizeQuery([
    transcript.title,
    transcript.providerSessionId,
    transcript.model ?? '',
    transcript.permissionMode
  ].join(' ')).includes(query))
  return {
    providerSessionId: transcript.providerSessionId,
    title: transcript.title,
    cwd: transcript.cwd,
    updatedAt: transcript.updatedAt,
    ...(transcript.model ? { model: transcript.model } : {}),
    permissionMode: transcript.permissionMode,
    eventCount: transcript.events.length,
    matchCount: hits.length + (metadataMatch ? 1 : 0),
    hits: hits.slice(0, 4)
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
