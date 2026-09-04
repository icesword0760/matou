import { existsSync, readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { basename, dirname, join } from 'node:path'

import { ProviderTranscriptHudReader } from './provider-transcript-hud'

export type HudPermissionMode = 'default' | 'auto' | 'acceptEdits' | 'plan' | 'bypassPermissions'
export type HudModelStrategy = 'opusplan' | 'claude-opus-4-6' | 'claude-sonnet-4-6'
export type HudTodoStatus = 'pending' | 'in_progress' | 'completed'
export interface HudUsageWindow { label: string; percent: number; resetsAt?: number }
export interface HudToolCount { name: string; count: number }
export interface HudToolActivity { name: string; target?: string; status: 'running' | 'completed' | 'error' }
export interface HudConfigCounts {
  instructionFiles: number
  projectInstructionFileExists: boolean
  mcpServers: number
  hooks: number
  mcpServerNames: string[]
  hookNames: string[]
}
export interface HudConfigWatchTarget { directory: string; names: string[] }
export interface SessionHudRunOwnership {
  runId: string
  currentRunId(): string | undefined
}

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
  sessionName?: string
  contextPercent?: number
  contextWindowSize?: number
  taskStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  teamRole?: string
  teamStatus?: 'idle' | 'running' | 'needs-input' | 'error'
  subagentCount?: number
  subagents?: string[]
  runningTools?: Array<{ name: string; target?: string }>
  toolCounts?: HudToolCount[]
  lastTool?: HudToolActivity
  usageWindows?: HudUsageWindow[]
  configCounts?: HudConfigCounts
  mcpErrors?: string[]
  todos?: Array<{ content: string; status: HudTodoStatus }>
  resumable?: boolean
}

interface MutableHud extends SessionHudSnapshot {
  activeTools: Map<string, { name: string; target?: string }>
  completedToolCounts: Map<string, number>
  failedMcpServers: Set<string>
  observedSubagents: Map<string, string>
  providerPermissionObserved: boolean
  pendingPermissionMode?: HudPermissionMode
  providerModelObserved: boolean
  configCheckedAt?: number
  configCwd?: string
}

export class SessionHudRegistry {
  readonly #states = new Map<string, MutableHud>()
  readonly #now: () => number
  readonly #configDir: string
  readonly #transcripts: ProviderTranscriptHudReader

  constructor(
    now: () => number = Date.now,
    configDir = process.env.CLAUDE_CONFIG_DIR || join(homedir(), '.claude'),
    transcripts = new ProviderTranscriptHudReader()
  ) {
    this.#now = now
    this.#configDir = configDir
    this.#transcripts = transcripts
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
    const configCounts = mode === 'agent' && input.cwd
      ? inspectProviderConfig(input.cwd, this.#configDir) : undefined
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
        subagents: [],
        runningTools: [],
        toolCounts: [],
        usageWindows: [],
        mcpErrors: [],
        ...(configCounts ? { configCounts } : {}),
        todos: [],
        resumable: input.resumable ?? previous?.resumable ?? false
      } : {}),
      activeTools: new Map(),
      completedToolCounts: new Map(),
      failedMcpServers: new Set(),
      observedSubagents: new Map(),
      providerPermissionObserved: false,
      providerModelObserved: false
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
      completedToolCounts: new Map(),
      failedMcpServers: new Set(),
      observedSubagents: new Map(),
      providerPermissionObserved: false,
      providerModelObserved: false
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
    if (current?.mode === 'agent' && normalized) {
      current.permissionMode = normalized
      current.pendingPermissionMode = normalized
    }
  }

  updateModel(sessionId: string, strategy: string): void {
    const current = this.#states.get(sessionId)
    const normalized = normalizeModelStrategy(strategy)
    if (!current || current.mode !== 'agent' || !normalized) return
    current.modelStrategy = normalized
    current.model = modelNameForStrategy(normalized)
  }

  updateProviderModel(sessionId: string, model: string): void {
    const current = this.#states.get(sessionId)
    if (!current || current.mode !== 'agent' || !model.trim()) return
    current.model = model
    const strategy = normalizeModelStrategy(model)
    if (strategy) current.modelStrategy = strategy
    current.providerModelObserved = false
  }

  updateSessionName(sessionId: string, name: string): void {
    const current = this.#states.get(sessionId)
    const normalized = text(name)
    if (current?.mode === 'agent' && normalized) current.sessionName = normalized.slice(0, 120)
  }

  markResumable(sessionId: string): void {
    const current = this.#states.get(sessionId)
    if (current?.mode === 'agent') current.resumable = true
  }

  ingestProvider(sessionId: string, payload: Record<string, unknown>): void {
    const current = this.#states.get(sessionId)
    if (!current || current.mode !== 'agent') return

    const cwd = text(payload.cwd)
    if (cwd) current.cwd = cwd
    const configCwd = cwd ?? current.cwd
    if (configCwd && (
      current.configCwd !== configCwd || current.configCheckedAt === undefined ||
      this.#now() - current.configCheckedAt >= 5_000
    )) {
      current.configCounts = inspectProviderConfig(configCwd, this.#configDir)
      current.configCwd = configCwd
      current.configCheckedAt = this.#now()
    }
    const permission = normalizePermission(text(payload.permission_mode) ?? text(payload.permissionMode))
    if (permission && (!current.pendingPermissionMode || permission === current.pendingPermissionMode)) {
      current.permissionMode = permission
      current.providerPermissionObserved = true
      delete current.pendingPermissionMode
    }

    const model = nestedText(payload.model, 'display_name') ?? nestedText(payload.model, 'displayName') ??
      nestedText(payload.model, 'id')
    if (model) {
      current.model = model
      current.providerModelObserved = true
    }
    const contextPercent = numeric(nested(payload.context_window, 'used_percentage')) ??
      numeric(nested(payload.contextWindow, 'usedPercentage')) ?? derivedContextPercent(payload)
    if (contextPercent !== undefined) current.contextPercent = Math.round(contextPercent)
    const contextWindowSize = numeric(nested(payload.context_window, 'context_window_size')) ??
      numeric(nested(payload.contextWindow, 'contextWindowSize'))
    if (contextWindowSize !== undefined && contextWindowSize > 0) current.contextWindowSize = contextWindowSize
    const duration = numeric(nested(payload.cost, 'total_duration_ms')) ??
      numeric(nested(payload.cost, 'totalDurationMs'))
    if (duration !== undefined && duration > Math.max(0, this.#now() - current.startedAt)) {
      current.startedAt = this.#now() - duration
    }
    const usageWindows = usageWindowsFromPayload(payload)
    if (usageWindows) current.usageWindows = usageWindows

    const eventName = text(payload.hook_event_name) ?? text(payload.hookEventName)
    if (!eventName) return
    const toolName = text(payload.tool_name) ?? text(payload.toolName)
    const toolId = text(payload.tool_use_id) ?? text(payload.toolUseId) ??
      `${toolName ?? 'tool'}:${this.#now()}`
    const toolInput = object(payload.tool_input) ?? object(payload.toolInput) ?? {}

    if (eventName === 'UserPromptSubmit' || eventName === 'PreToolUse') {
      current.taskStatus = 'running'
    } else if (eventName === 'PermissionRequest') {
      current.taskStatus = 'needs-input'
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
        current.observedSubagents.set(
          toolId,
          subagentLabel(toolInput, current.observedSubagents.size + 1)
        )
        current.subagentCount = current.observedSubagents.size
        current.subagents = [...current.observedSubagents.values()]
      }
      current.activeTools.set(toolId, {
        name: toolName,
        ...targetForTool(toolName, toolInput)
      })
      current.lastTool = { name: toolName, ...targetForTool(toolName, toolInput), status: 'running' }
    }
    if ((eventName === 'PostToolUse' || eventName === 'PostToolUseFailure') && toolName) {
      const active = current.activeTools.get(toolId)
      const status = eventName === 'PostToolUseFailure' ? 'error' : 'completed'
      current.completedToolCounts.set(toolName, (current.completedToolCounts.get(toolName) ?? 0) + 1)
      current.lastTool = { name: toolName, ...(active?.target ? { target: active.target } : {}), status }
      const mcpServer = mcpServerName(toolName)
      if (mcpServer) {
        if (status === 'error') current.failedMcpServers.add(mcpServer)
        else current.failedMcpServers.delete(mcpServer)
      }
      current.activeTools.delete(toolId)
    }
    current.runningTools = [...current.activeTools.values()]
    current.toolCounts = [...current.completedToolCounts.entries()].map(([name, count]) => ({ name, count }))
    current.mcpErrors = [...current.failedMcpServers]
  }

  async refreshTranscript(
    sessionId: string,
    transcriptPath: string,
    ownership: SessionHudRunOwnership
  ): Promise<boolean> {
    const current = this.#states.get(sessionId)
    if (!current || current.mode !== 'agent' || !ownsCurrentRun(ownership)) return false
    const history = await this.#transcripts.read(transcriptPath)
    // Transcript I/O can outlive the provider process that requested it. Recheck
    // both run ownership and HUD object identity immediately before any write.
    if (!history || !ownsCurrentRun(ownership) || this.#states.get(sessionId) !== current) return false
    if (history.sessionName) current.sessionName = history.sessionName.slice(0, 120)
    if (history.startedAt !== undefined) current.startedAt = Math.min(current.startedAt, history.startedAt)
    const permission = normalizePermission(history.permissionMode)
    if (permission && !current.providerPermissionObserved) current.permissionMode = permission
    if (history.model && !current.providerModelObserved) current.model = history.model
    current.subagentCount = history.subagentCount
    current.subagents = history.subagents
    current.observedSubagents = new Map(
      history.subagents.map((name, index) => [`transcript:${index}`, name])
    )
    current.runningTools = history.runningTools
    current.toolCounts = history.toolCounts
    if (history.lastTool) current.lastTool = history.lastTool
    else delete current.lastTool
    current.mcpErrors = history.mcpErrors
    current.todos = history.todos
    return true
  }

  refreshConfig(sessionId: string): boolean {
    const current = this.#states.get(sessionId)
    if (!current || current.mode !== 'agent' || !current.cwd) return false
    const next = inspectProviderConfig(current.cwd, this.#configDir)
    const previous = current.configCounts
    current.configCounts = next
    current.configCwd = current.cwd
    current.configCheckedAt = this.#now()
    return !previous || previous.instructionFiles !== next.instructionFiles ||
      previous.projectInstructionFileExists !== next.projectInstructionFileExists ||
      previous.mcpServers !== next.mcpServers || previous.hooks !== next.hooks ||
      previous.mcpServerNames.join('\0') !== next.mcpServerNames.join('\0') ||
      previous.hookNames.join('\0') !== next.hookNames.join('\0')
  }

  configWatchTargets(sessionId: string): HudConfigWatchTarget[] {
    const current = this.#states.get(sessionId)
    if (!current || current.mode !== 'agent' || !current.cwd) return []
    return providerConfigWatchTargets(current.cwd, this.#configDir)
  }

  snapshot(sessionId: string): SessionHudSnapshot | undefined {
    const state = this.#states.get(sessionId)
    if (!state) return undefined
    const {
      activeTools: _activeTools, observedSubagents: _observedSubagents,
      completedToolCounts: _completedToolCounts, failedMcpServers: _failedMcpServers,
      providerPermissionObserved: _providerPermissionObserved,
      pendingPermissionMode: _pendingPermissionMode,
      providerModelObserved: _providerModelObserved,
      configCheckedAt: _configCheckedAt, configCwd: _configCwd,
      ...snapshot
    } = state
    return structuredClone(snapshot)
  }

  snapshots(): SessionHudSnapshot[] {
    return [...this.#states.keys()].flatMap((sessionId) => {
      const value = this.snapshot(sessionId)
      return value ? [value] : []
    })
  }
}

function ownsCurrentRun(ownership: SessionHudRunOwnership): boolean {
  return ownership.currentRunId() === ownership.runId
}

export function inspectProviderConfig(cwd: string, configDir: string): HudConfigCounts {
  let instructionFiles = 0
  const projectInstructionFileExists = existsSync(join(cwd, 'CLAUDE.md'))
  const hookNames = new Set<string>()
  const userMcp = new Set<string>()
  const projectMcp = new Set<string>()

  for (const path of [
    join(configDir, 'CLAUDE.md'),
    join(cwd, 'CLAUDE.md'), join(cwd, 'CLAUDE.local.md'),
    join(cwd, '.claude', 'CLAUDE.md'), join(cwd, '.claude', 'CLAUDE.local.md')
  ]) {
    if (existsSync(path)) instructionFiles += 1
  }

  const userSettings = [join(configDir, 'settings.json'), join(configDir, 'settings.local.json')]
  for (const path of userSettings) {
    const config = readJson(path)
    addMcpNames(userMcp, config)
    addHookNames(hookNames, config)
  }
  const userRootConfig = readJson(join(dirname(configDir), '.claude.json'))
  addMcpNames(userMcp, userRootConfig)
  removeNames(userMcp, userRootConfig?.disabledMcpServers)

  const mcpJson = readJson(join(cwd, '.mcp.json'))
  addMcpNames(projectMcp, mcpJson)
  const projectSettings = [
    join(cwd, '.claude', 'settings.json'), join(cwd, '.claude', 'settings.local.json')
  ]
  for (const path of projectSettings) {
    const config = readJson(path)
    addMcpNames(projectMcp, config)
    removeNames(projectMcp, config?.disabledMcpjsonServers)
    addHookNames(hookNames, config)
  }
  const mcpServerNames = [...userMcp, ...projectMcp]
  return {
    instructionFiles,
    projectInstructionFileExists,
    mcpServers: mcpServerNames.length,
    hooks: hookNames.size,
    mcpServerNames,
    hookNames: [...hookNames]
  }
}

export function providerConfigWatchTargets(cwd: string, configDir: string): HudConfigWatchTarget[] {
  const targets = new Map<string, Set<string>>()
  const append = (directory: string, names: string[]) => {
    const current = targets.get(directory) ?? new Set<string>()
    for (const name of names) current.add(name)
    targets.set(directory, current)
  }
  append(dirname(configDir), ['.claude.json', basename(configDir)])
  append(configDir, ['CLAUDE.md', 'settings.json', 'settings.local.json'])
  append(cwd, ['CLAUDE.md', 'CLAUDE.local.md', '.mcp.json', '.claude'])
  append(join(cwd, '.claude'), [
    'CLAUDE.md', 'CLAUDE.local.md', 'settings.json', 'settings.local.json'
  ])
  return [...targets].map(([directory, names]) => ({ directory, names: [...names] }))
}

function readJson(path: string): Record<string, unknown> | undefined {
  try {
    const value = JSON.parse(readFileSync(path, 'utf8')) as unknown
    return object(value)
  } catch {
    return undefined
  }
}

function addMcpNames(output: Set<string>, config: Record<string, unknown> | undefined): void {
  const servers = object(config?.mcpServers)
  if (!servers) return
  for (const name of Object.keys(servers)) output.add(name)
}

function removeNames(output: Set<string>, value: unknown): void {
  if (!Array.isArray(value)) return
  for (const name of value) if (typeof name === 'string') output.delete(name)
}

function addHookNames(output: Set<string>, config: Record<string, unknown> | undefined): void {
  for (const name of Object.keys(object(config?.hooks) ?? {})) output.add(name)
}

function subagentLabel(input: Record<string, unknown>, ordinal: number): string {
  return text(input.name) ?? text(input.description) ?? text(input.subagent_type) ??
    text(input.subagentType) ?? `Agent ${ordinal}`
}

function normalizePermission(value: unknown): HudPermissionMode | undefined {
  if (typeof value !== 'string') return undefined
  const normalized = value.replace(/[\s_-]/g, '').toLowerCase()
  if (normalized === 'default') return 'default'
  if (normalized === 'auto') return 'auto'
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

function usageWindowsFromPayload(payload: Record<string, unknown>): HudUsageWindow[] | undefined {
  const limits = object(payload.rate_limits) ?? object(payload.rateLimits)
  if (!limits) return undefined
  const windows: HudUsageWindow[] = []
  appendUsageWindow(windows, '5h', object(limits.five_hour) ?? object(limits.fiveHour))
  appendUsageWindow(windows, 'Weekly', object(limits.seven_day) ?? object(limits.sevenDay))
  const scoped = limits.model_scoped ?? limits.modelScoped
  if (Array.isArray(scoped)) {
    for (const item of scoped.slice(0, 8)) {
      const window = object(item)
      const label = text(window?.display_name) ?? text(window?.displayName)
      const percent = numeric(window?.utilization)
      if (!label || percent === undefined) continue
      const resetsAt = timestampMilliseconds(window?.resets_at ?? window?.resetsAt)
      windows.push({ label: label.slice(0, 64), percent: clampPercent(percent), ...(resetsAt ? { resetsAt } : {}) })
    }
  }
  return windows
}

function appendUsageWindow(
  output: HudUsageWindow[], label: string, value: Record<string, unknown> | undefined
): void {
  if (!value) return
  const percent = numeric(value.used_percentage) ?? numeric(value.usedPercentage)
  if (percent === undefined) return
  const resetsAt = timestampMilliseconds(value.resets_at ?? value.resetsAt)
  output.push({ label, percent: clampPercent(percent), ...(resetsAt ? { resetsAt } : {}) })
}

function timestampMilliseconds(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value > 1_000_000_000_000 ? Math.round(value) : Math.round(value * 1_000)
  }
  if (typeof value !== 'string') return undefined
  const parsed = Date.parse(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

function clampPercent(value: number): number {
  return Math.max(0, Math.min(100, Math.round(value)))
}

function mcpServerName(toolName: string): string | undefined {
  const match = /^mcp__(.+?)__.+$/.exec(toolName)
  return match?.[1]
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
