import { createReadStream } from 'node:fs'
import { stat } from 'node:fs/promises'
import { StringDecoder } from 'node:string_decoder'

export interface ProviderTranscriptHudSnapshot {
  sessionName?: string
  startedAt?: number
  permissionMode?: string
  model?: string
  subagentCount: number
  subagents: string[]
  runningTools: Array<{ name: string; target?: string }>
  toolCounts: Array<{ name: string; count: number }>
  lastTool?: { name: string; target?: string; status: 'completed' | 'error' }
  mcpErrors: string[]
  todos: Array<{ content: string; status: 'pending' | 'in_progress' | 'completed' }>
}

interface TranscriptState {
  offset: number
  remainder: string
  fileMtimeMs: number
  customTitle?: string
  autoTitle?: string
  slug?: string
  firstPrompt?: string
  startedAt?: number
  permissionMode?: string
  model?: string
  activeTools: Map<string, { name: string; target?: string }>
  completedTools: Set<string>
  completedToolCounts: Map<string, number>
  failedMcpServers: Set<string>
  observedSubagents: Map<string, string>
  lastTool?: { name: string; target?: string; status: 'completed' | 'error' }
  todos: Array<{ content: string; status: 'pending' | 'in_progress' | 'completed' }>
}

export class ProviderTranscriptHudReader {
  readonly #states = new Map<string, TranscriptState>()
  readonly #inFlight = new Map<string, Promise<ProviderTranscriptHudSnapshot | undefined>>()

  read(path: string): Promise<ProviderTranscriptHudSnapshot | undefined> {
    const current = this.#inFlight.get(path)
    if (current) return current
    const pending = this.#read(path).finally(() => this.#inFlight.delete(path))
    this.#inFlight.set(path, pending)
    return pending
  }

  async #read(path: string): Promise<ProviderTranscriptHudSnapshot | undefined> {
    const file = await stat(path).catch(() => undefined)
    if (!file?.isFile()) return undefined
    let state = this.#states.get(path)
    if (!state || file.size < state.offset || (file.size === state.offset && file.mtimeMs !== state.fileMtimeMs)) {
      state = emptyState()
      this.#states.set(path, state)
    }
    if (file.size > state.offset) await appendTranscript(path, state)
    state.fileMtimeMs = file.mtimeMs
    return snapshot(state)
  }
}

function emptyState(): TranscriptState {
  return {
    offset: 0,
    remainder: '',
    fileMtimeMs: 0,
    activeTools: new Map(),
    completedTools: new Set(),
    completedToolCounts: new Map(),
    failedMcpServers: new Set(),
    observedSubagents: new Map(),
    todos: []
  }
}

async function appendTranscript(path: string, state: TranscriptState): Promise<void> {
  const stream = createReadStream(path, { start: state.offset })
  const decoder = new StringDecoder('utf8')
  let pending = state.remainder
  for await (const chunk of stream) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)
    state.offset += buffer.byteLength
    pending += decoder.write(buffer)
    const lines = pending.split('\n')
    pending = lines.pop() ?? ''
    for (const line of lines) ingestLine(state, line)
  }
  pending += decoder.end()
  if (isCompleteJsonLine(pending)) {
    ingestLine(state, pending)
    pending = ''
  }
  state.remainder = pending
}

function isCompleteJsonLine(value: string): boolean {
  if (!value.trim()) return false
  try {
    return isRecord(JSON.parse(value) as unknown)
  } catch {
    return false
  }
}

function ingestLine(state: TranscriptState, line: string): void {
  let row: Record<string, unknown>
  try {
    const parsed = JSON.parse(line) as unknown
    if (!isRecord(parsed)) return
    row = parsed
  } catch {
    return
  }
  const timestamp = timestampMs(row.timestamp)
  if (timestamp !== undefined) state.startedAt = Math.min(state.startedAt ?? timestamp, timestamp)
  const customTitle = text(row.customTitle)
  const autoTitle = text(row.aiTitle)
  const slug = text(row.slug)
  if (customTitle) state.customTitle = customTitle
  if (autoTitle) state.autoTitle = autoTitle
  if (slug) state.slug = slug
  const permission = text(row.permissionMode)
  if (permission) state.permissionMode = permission

  const message = isRecord(row.message) ? row.message : undefined
  if (!message) return
  const messagePermission = text(message.permissionMode)
  const model = text(message.model)
  if (messagePermission) state.permissionMode = messagePermission
  if (model) state.model = model
  const content = message.content
  if (message.role === 'user' && !state.firstPrompt && typeof content === 'string' && !content.trimStart().startsWith('<')) {
    const prompt = compactTitle(content)
    if (prompt) state.firstPrompt = prompt
  }
  if (!Array.isArray(content)) return
  for (const item of content) {
    if (!isRecord(item)) continue
    if (item.type === 'tool_use') ingestToolUse(state, item)
    if (item.type === 'tool_result') ingestToolResult(state, item)
  }
}

function ingestToolUse(state: TranscriptState, item: Record<string, unknown>): void {
  const id = text(item.id)
  const name = text(item.name)
  if (!id || !name || state.completedTools.has(id)) return
  const input = isRecord(item.input) ? item.input : {}
  const tool = { name, ...targetForTool(name, input) }
  state.activeTools.set(id, tool)
  if (name === 'TodoWrite') state.todos = todosFromInput(input.todos)
  if (name === 'TaskCreate') state.todos = applyTaskCreate(state.todos, input)
  if (name === 'TaskUpdate') state.todos = applyTaskUpdate(state.todos, input)
  if (name === 'Agent') {
    state.observedSubagents.set(id, subagentLabel(input, state.observedSubagents.size + 1))
  }
}

function ingestToolResult(state: TranscriptState, item: Record<string, unknown>): void {
  const id = text(item.tool_use_id) ?? text(item.toolUseId)
  if (!id || state.completedTools.has(id)) return
  const tool = state.activeTools.get(id)
  if (!tool) return
  const status = item.is_error === true || item.isError === true ? 'error' : 'completed'
  state.completedTools.add(id)
  state.completedToolCounts.set(tool.name, (state.completedToolCounts.get(tool.name) ?? 0) + 1)
  state.lastTool = { ...tool, status }
  const server = mcpServerName(tool.name)
  if (server) {
    if (status === 'error') state.failedMcpServers.add(server)
    else state.failedMcpServers.delete(server)
  }
  state.activeTools.delete(id)
}

function snapshot(state: TranscriptState): ProviderTranscriptHudSnapshot {
  const sessionName = state.customTitle ?? state.autoTitle ?? state.slug ?? state.firstPrompt
  return {
    ...(sessionName ? { sessionName } : {}),
    ...(state.startedAt === undefined ? {} : { startedAt: state.startedAt }),
    ...(state.permissionMode ? { permissionMode: state.permissionMode } : {}),
    ...(state.model ? { model: state.model } : {}),
    subagentCount: state.observedSubagents.size,
    subagents: [...state.observedSubagents.values()],
    runningTools: [...state.activeTools.values()],
    toolCounts: [...state.completedToolCounts.entries()].map(([name, count]) => ({ name, count })),
    ...(state.lastTool ? { lastTool: { ...state.lastTool } } : {}),
    mcpErrors: [...state.failedMcpServers],
    todos: state.todos.map((todo) => ({ ...todo }))
  }
}

function targetForTool(name: string, input: Record<string, unknown>): { target?: string } {
  let target: string | undefined
  if (['Read', 'Write', 'Edit', 'MultiEdit', 'NotebookEdit'].includes(name)) {
    target = text(input.file_path) ?? text(input.path)
  } else if (name === 'Glob' || name === 'Grep') {
    target = text(input.pattern)
  } else if (name === 'Bash') {
    const command = text(input.command)
    target = command && command.length > 30 ? `${command.slice(0, 27)}...` : command
  }
  return target ? { target } : {}
}

function subagentLabel(input: Record<string, unknown>, ordinal: number): string {
  return text(input.name) ?? text(input.description) ?? text(input.subagent_type) ??
    text(input.subagentType) ?? `Agent ${ordinal}`
}

function todosFromInput(value: unknown): ProviderTranscriptHudSnapshot['todos'] {
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    if (!isRecord(item)) return []
    const content = text(item.content) ?? text(item.subject) ?? text(item.description)
    return content ? [{ content, status: todoStatus(item.status) }] : []
  })
}

function applyTaskCreate(
  todos: ProviderTranscriptHudSnapshot['todos'], input: Record<string, unknown>
): ProviderTranscriptHudSnapshot['todos'] {
  return [...todos, {
    content: text(input.subject) ?? text(input.description) ?? 'Untitled',
    status: todoStatus(input.status)
  }]
}

function applyTaskUpdate(
  todos: ProviderTranscriptHudSnapshot['todos'], input: Record<string, unknown>
): ProviderTranscriptHudSnapshot['todos'] {
  const id = text(input.taskId) ?? text(input.task_id)
  const index = id && /^\d+$/.test(id) ? Number(id) - 1 : -1
  if (index < 0 || index >= todos.length) return todos
  return todos.map((todo, ordinal) => ordinal === index ? {
    content: text(input.subject) ?? text(input.description) ?? todo.content,
    status: input.status === undefined ? todo.status : todoStatus(input.status)
  } : todo)
}

function todoStatus(value: unknown): 'pending' | 'in_progress' | 'completed' {
  const normalized = typeof value === 'string' ? value.toLowerCase() : ''
  if (['completed', 'complete', 'done'].includes(normalized)) return 'completed'
  if (['in_progress', 'running'].includes(normalized)) return 'in_progress'
  return 'pending'
}

function mcpServerName(name: string): string | undefined {
  return /^mcp__(.+?)__.+$/.exec(name)?.[1]
}

function compactTitle(value: string): string | undefined {
  const compact = value.replace(/\s+/g, ' ').trim()
  return compact ? compact.slice(0, 80) : undefined
}

function timestampMs(value: unknown): number | undefined {
  if (typeof value !== 'string') return undefined
  const parsed = Date.parse(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

function text(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  const normalized = value.trim()
  return normalized || undefined
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
