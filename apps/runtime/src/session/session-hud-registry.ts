export type HudPermissionMode = 'default' | 'acceptEdits' | 'plan' | 'bypassPermissions'
export type HudModelStrategy = 'opusplan' | 'claude-opus-4-6' | 'claude-sonnet-4-6'
export type HudTodoStatus = 'pending' | 'in_progress' | 'completed'

export interface SessionHudSnapshot {
  sessionId: string
  mode: 'shell' | 'agent'
  shell?: string
  cwd?: string
  gitBranch?: string
  gitDirty?: boolean
  startedAt: number
  permissionMode?: HudPermissionMode
  modelStrategy?: HudModelStrategy
  model?: string
  contextPercent?: number
  taskStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  teamRole?: string
  teamStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  subagentCount?: number
  runningTools?: Array<{ name: string; target?: string }>
  todos?: Array<{ content: string; status: HudTodoStatus }>
  resumable?: boolean
}

interface MutableHud extends SessionHudSnapshot {
  activeTools: Map<string, { name: string; target?: string }>
  observedSubagents: Set<string>
}

export class SessionHudRegistry {
  readonly #states = new Map<string, MutableHud>()
  readonly #now: () => number

  constructor(now: () => number = Date.now) {
    this.#now = now
  }

  spawn(input: {
    sessionId: string
    profile: 'shell' | 'claude-code' | 'codex'
    shell?: string
    cwd?: string
    startedAt?: number
    permissionMode?: string
    modelStrategy?: string
    model?: string
    resumable?: boolean
  }): void {
    const previous = this.#states.get(input.sessionId)
    const mode = input.profile === 'shell' ? 'shell' : 'agent'
    const permissionMode = normalizePermission(input.permissionMode)
    const modelStrategy = normalizeModelStrategy(input.modelStrategy ?? input.model)
    this.#states.set(input.sessionId, {
      sessionId: input.sessionId,
      mode,
      ...(input.shell ? { shell: input.shell } : {}),
      ...(input.cwd ? { cwd: input.cwd } : {}),
      startedAt: previous?.startedAt ?? input.startedAt ?? this.#now(),
      ...(mode === 'agent' ? {
        permissionMode: permissionMode ?? 'default',
        modelStrategy: modelStrategy ?? 'opusplan',
        ...(input.model ? { model: input.model } : {}),
        taskStatus: 'idle' as const,
        subagentCount: 0,
        runningTools: [],
        todos: [],
        resumable: input.resumable ?? previous?.resumable ?? false
      } : {}),
      activeTools: new Map(),
      observedSubagents: new Set()
    })
  }

  exit(sessionId: string, options: { fallbackToShell?: boolean } = {}): void {
    const current = this.#states.get(sessionId)
    if (!current) return
    if (!options.fallbackToShell) {
      this.#states.delete(sessionId)
      return
    }
    this.#states.set(sessionId, {
      sessionId,
      mode: 'shell',
      ...(current.shell ? { shell: current.shell } : {}),
      ...(current.cwd ? { cwd: current.cwd } : {}),
      ...(current.gitBranch ? { gitBranch: current.gitBranch, gitDirty: current.gitDirty === true } : {}),
      startedAt: current.startedAt,
      activeTools: new Map(),
      observedSubagents: new Set()
    })
  }

  delete(sessionId: string): void {
    this.#states.delete(sessionId)
  }

  updateEnvironment(sessionId: string, input: {
    cwd?: string
    gitBranch?: string
    gitDirty?: boolean
    shell?: string
  }): void {
    const current = this.#states.get(sessionId)
    if (!current) return
    if (input.cwd) current.cwd = input.cwd
    if (input.shell) current.shell = input.shell
    if (input.gitBranch) {
      current.gitBranch = input.gitBranch
      current.gitDirty = input.gitDirty === true
    } else {
      delete current.gitBranch
      delete current.gitDirty
    }
  }

  updatePermission(sessionId: string, permissionMode: string): void {
    const current = this.#states.get(sessionId)
    const normalized = normalizePermission(permissionMode)
    if (current?.mode === 'agent' && normalized) current.permissionMode = normalized
  }

  updateModel(sessionId: string, strategy: string): void {
    const current = this.#states.get(sessionId)
    const normalized = normalizeModelStrategy(strategy)
    if (!current || current.mode !== 'agent' || !normalized) return
    current.modelStrategy = normalized
    current.model = modelNameForStrategy(normalized)
  }

  ingestProvider(sessionId: string, payload: Record<string, unknown>): void {
    const current = this.#states.get(sessionId)
    if (!current || current.mode !== 'agent') return

    const cwd = text(payload.cwd)
    if (cwd) current.cwd = cwd
    const providerId = text(payload.session_id) ?? text(payload.sessionId)
    if (providerId) current.resumable = true
    const permission = normalizePermission(text(payload.permission_mode) ?? text(payload.permissionMode))
    if (permission) current.permissionMode = permission

    const model = nestedText(payload.model, 'display_name') ?? nestedText(payload.model, 'displayName') ??
      nestedText(payload.model, 'id')
    if (model) current.model = model
    const contextPercent = numeric(nested(payload.context_window, 'used_percentage')) ??
      numeric(nested(payload.contextWindow, 'usedPercentage')) ?? derivedContextPercent(payload)
    if (contextPercent !== undefined) current.contextPercent = Math.round(contextPercent)

    const eventName = text(payload.hook_event_name) ?? text(payload.hookEventName)
    if (!eventName) return
    const toolName = text(payload.tool_name) ?? text(payload.toolName)
    const toolId = text(payload.tool_use_id) ?? text(payload.toolUseId) ??
      `${toolName ?? 'tool'}:${this.#now()}`
    const toolInput = object(payload.tool_input) ?? object(payload.toolInput) ?? {}

    if (eventName === 'UserPromptSubmit' || eventName === 'PreToolUse') {
      current.taskStatus = 'running'
    } else if (eventName === 'Notification') {
      current.taskStatus = notificationStatus(payload)
    } else if (eventName === 'Stop') {
      current.taskStatus = 'idle'
      current.activeTools.clear()
    } else if (eventName === 'SessionEnd') {
      current.taskStatus = 'idle'
      current.activeTools.clear()
    }

    if (eventName === 'PreToolUse' && toolName) {
      if (toolName === 'TodoWrite') current.todos = todosFromInput(toolInput.todos)
      if (toolName === 'TaskCreate') current.todos = applyTaskCreate(current.todos ?? [], toolInput)
      if (toolName === 'TaskUpdate') current.todos = applyTaskUpdate(current.todos ?? [], toolInput)
      if (toolName === 'Agent') {
        current.observedSubagents.add(toolId)
        current.subagentCount = current.observedSubagents.size
      }
      current.activeTools.set(toolId, {
        name: toolName,
        ...targetForTool(toolName, toolInput)
      })
    }
    if ((eventName === 'PostToolUse' || eventName === 'PostToolUseFailure') && toolName) {
      current.activeTools.delete(toolId)
    }
    current.runningTools = [...current.activeTools.values()]
  }

  snapshot(sessionId: string): SessionHudSnapshot | undefined {
    const state = this.#states.get(sessionId)
    if (!state) return undefined
    const { activeTools: _activeTools, observedSubagents: _observedSubagents, ...snapshot } = state
    return structuredClone(snapshot)
  }

  snapshots(): SessionHudSnapshot[] {
    return [...this.#states.keys()].flatMap((sessionId) => {
      const value = this.snapshot(sessionId)
      return value ? [value] : []
    })
  }
}

function normalizePermission(value: unknown): HudPermissionMode | undefined {
  if (typeof value !== 'string') return undefined
  const normalized = value.replace(/[\s_-]/g, '').toLowerCase()
  if (normalized === 'default') return 'default'
  if (normalized === 'acceptedits') return 'acceptEdits'
  if (normalized === 'plan' || normalized === 'planmode') return 'plan'
  if (normalized === 'bypasspermissions' || normalized === 'bypass') return 'bypassPermissions'
  return undefined
}

function normalizeModelStrategy(value: unknown): HudModelStrategy | undefined {
  if (typeof value !== 'string') return undefined
  const normalized = value.toLowerCase()
  if (normalized.includes('opusplan') || normalized.includes('opus plan')) return 'opusplan'
  if (normalized.includes('sonnet')) return 'claude-sonnet-4-6'
  if (normalized.includes('opus')) return 'claude-opus-4-6'
  return undefined
}

function modelNameForStrategy(strategy: HudModelStrategy): string {
  if (strategy === 'claude-sonnet-4-6') return 'Claude Sonnet 4.6'
  if (strategy === 'claude-opus-4-6') return 'Claude Opus 4.6'
  return 'Opus Plan'
}

function derivedContextPercent(payload: Record<string, unknown>): number | undefined {
  const window = object(payload.context_window) ?? object(payload.contextWindow)
  const size = numeric(window?.context_window_size) ?? numeric(window?.contextWindowSize)
  const usage = object(window?.current_usage) ?? object(window?.currentUsage)
  if (!size || !usage) return undefined
  const used = ['input_tokens', 'cache_creation_input_tokens', 'cache_read_input_tokens']
    .reduce((sum, key) => sum + (numeric(usage[key]) ?? 0), 0)
  return used / size * 100
}

function notificationStatus(payload: Record<string, unknown>): 'needs-input' | 'error' {
  const value = [payload.message, payload.body, payload.text, payload.error]
    .map(text).filter(Boolean).join(' ').toLowerCase()
  return /error|failed|exception/.test(value) ? 'error' : 'needs-input'
}

function todosFromInput(value: unknown): Array<{ content: string; status: HudTodoStatus }> {
  if (!Array.isArray(value)) return []
  return value.flatMap((item) => {
    const data = object(item)
    const content = text(data?.content) ?? text(data?.subject) ?? text(data?.description)
    if (!content) return []
    return [{ content, status: normalizeTodoStatus(data?.status) }]
  })
}

function applyTaskCreate(
  todos: Array<{ content: string; status: HudTodoStatus }>,
  input: Record<string, unknown>
): Array<{ content: string; status: HudTodoStatus }> {
  const content = text(input.subject) ?? text(input.description) ?? 'Untitled'
  return [...todos, { content, status: normalizeTodoStatus(input.status) }]
}

function applyTaskUpdate(
  todos: Array<{ content: string; status: HudTodoStatus }>,
  input: Record<string, unknown>
): Array<{ content: string; status: HudTodoStatus }> {
  const taskId = text(input.taskId) ?? text(input.task_id)
  const index = taskId && /^\d+$/.test(taskId) ? Number(taskId) - 1 : -1
  if (index < 0 || index >= todos.length) return todos
  return todos.map((todo, ordinal) => ordinal === index ? {
    content: text(input.subject) ?? text(input.description) ?? todo.content,
    status: input.status === undefined ? todo.status : normalizeTodoStatus(input.status)
  } : todo)
}

function normalizeTodoStatus(value: unknown): HudTodoStatus {
  const normalized = typeof value === 'string' ? value.toLowerCase() : ''
  if (['completed', 'complete', 'done'].includes(normalized)) return 'completed'
  if (['in_progress', 'running'].includes(normalized)) return 'in_progress'
  return 'pending'
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

function object(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : undefined
}
function nested(value: unknown, key: string): unknown { return object(value)?.[key] }
function nestedText(value: unknown, key: string): string | undefined { return text(nested(value, key)) }
function text(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  return trimmed || undefined
}
function numeric(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}
