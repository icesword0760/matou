import { randomUUID } from 'node:crypto'
import { TextDecoder } from 'node:util'

import type {
  ForkBatchResult,
  ForkEnvironmentChoice,
  ForkItemInput,
  HostActionMethod,
  HostActionTargetSelector,
  HostEntitySelector,
  HostImpactSummary,
  HostResultPath
} from '../control/host-action-types'
import type {
  AllowedControlKey,
  HostControlScope,
  HostTarget,
  HostTargetSelector
} from '../control/host-control-types'
import { HostControlClient, HostControlClientError } from '../control/host-control-client'

export interface MtIo {
  stdout(text: string): void
  stderr(text: string): void
}

export interface MtEnvironment {
  MATOU_CONTROL_ENDPOINT?: string
  MATOU_CONTROL_TOKEN?: string
}

export type MtRequest = (method: HostControlScope, params: unknown) => Promise<unknown>

export interface MtDependencies {
  request?: MtRequest
  readStdin?: () => Promise<string>
}

interface ParsedActionCommand {
  method: HostActionMethod
  params: unknown
  json: boolean
}

export async function runMt(
  argv: string[],
  environment: MtEnvironment,
  io: MtIo,
  dependencies: MtDependencies | MtRequest = {},
  legacyReadStdin?: () => Promise<string>
): Promise<number> {
  try {
    if (argv.length === 0 || argv.includes('--help') || argv.includes('-h')) {
      io.stdout(HELP_TEXT)
      return 0
    }
    const resolvedDependencies = normalizeDependencies(dependencies, legacyReadStdin)
    const request = resolvedDependencies.request ?? requestFromEnvironment(environment)
    const readStdin = resolvedDependencies.readStdin ?? readProcessStdin
    const command = argv[0]
    const json = argv.includes('--json')
    if (command === 'identify') {
      const result = await request('host.identify', {})
      printResult(result, json, io, formatIdentity)
      return 0
    }
    if (command === 'list') {
      const result = await request('host.list', { scope: argv.includes('--all') ? 'all' : 'current-level' })
      printResult(result, json, io, formatList)
      return 0
    }
    const action = command === 'create'
      ? await parseCreateCommand(argv.slice(1), request)
      : command === 'fork'
        ? await parseForkCommand(argv.slice(1), request, readStdin)
        : command === 'remove'
          ? await parseRemoveCommand(argv.slice(1), request)
          : command === 'close'
            ? await parseCloseCommand(argv.slice(1), request)
            : command === 'focus' || command === 'switch'
              ? await parseNavigationCommand(argv, request)
              : undefined
    if (action) {
      const result = await request(action.method, action.params)
      printResult(result, action.json, io, formatActionResult)
      return partialSuccessExitCode(result)
    }
    if (!['read', 'history', 'commands', 'send', 'key'].includes(command ?? '')) {
      throw new MtUsageError(`未知命令：${command ?? ''}`)
    }
    const targetText = argv[1]
    if (!targetText || targetText.startsWith('--')) throw new MtUsageError(`${command} 需要目标`)
    const selector = await parseTarget(targetText, request)
    if (command === 'read' || command === 'history') {
      const result = await request(
        command === 'read' ? 'terminal.read-current' : 'terminal.read-history',
        {
          target: selector,
          maxLines: numberFlag(argv, '--lines', command === 'read' ? 200 : 1000, 1, 5000),
          maxBytes: numberFlag(argv, '--bytes', 64 * 1024, 1, 1024 * 1024)
        }
      )
      printResult(result, json, io, formatTerminalText)
      return 0
    }
    if (command === 'commands') {
      const result = await request('terminal.read-commands', {
        target: selector,
        limit: numberFlag(argv, '--limit', 100, 1, 1000)
      })
      printResult(result, json, io, (value) => formatCommands(value))
      return 0
    }
    if (command === 'send') {
      const text = positionalTail(argv.slice(2), ['--enter', '--json'])
      if (!text) throw new MtUsageError('send 需要发送文本')
      const result = await request('terminal.send-text', {
        target: selector, text, submit: argv.includes('--enter')
      })
      printResult(result, json, io, () => argv.includes('--enter') ? '已发送文本并回车' : '已发送文本')
      return 0
    }
    const rawKey = argv[2]
    if (!rawKey) throw new MtUsageError('key 需要按键名称')
    const key = normalizeKey(rawKey)
    const result = await request('terminal.send-key', { target: selector, key })
    printResult(result, json, io, () => `已发送按键 ${key}`)
    return 0
  } catch (error) {
    io.stderr(formatError(error))
    return exitCode(error)
  }
}

function normalizeDependencies(
  dependencies: MtDependencies | MtRequest,
  legacyReadStdin?: () => Promise<string>
): MtDependencies {
  return typeof dependencies === 'function'
    ? { request: dependencies, ...(legacyReadStdin ? { readStdin: legacyReadStdin } : {}) }
    : dependencies
}

function requestFromEnvironment(environment: MtEnvironment): MtRequest {
  const endpoint = environment.MATOU_CONTROL_ENDPOINT
  const token = environment.MATOU_CONTROL_TOKEN
  if (!endpoint || !token) {
    throw new HostControlClientError(
      'CAPABILITY_DENIED',
      '`mt` 仅在 Matou 托管终端中可用；当前进程缺少会话控制身份'
    )
  }
  const client = new HostControlClient({ endpoint, token })
  return (method, params) => client.request(method, params)
}

async function parseCreateCommand(
  argv: string[],
  request: MtRequest
): Promise<ParsedActionCommand> {
  const entity = argv[0]
  if (!['workspace', 'task', 'canvas', 'session'].includes(entity ?? '')) {
    throw new MtUsageError('create 需要 workspace、task、canvas 或 session')
  }
  const allowedValues: Record<string, string> = {
    '--title': 'title', '--submission-key': 'submissionKey'
  }
  const requiredTargetFlag = entity === 'workspace'
    ? '--path'
    : entity === 'task'
      ? '--workspace'
      : entity === 'canvas'
        ? '--task'
        : '--canvas'
  allowedValues[requiredTargetFlag] = 'target'
  if (entity === 'session') allowedValues['--profile'] = 'profile'
  const options = parseOptions(argv.slice(1), allowedValues, ['--enter', '--json'])
  const targetText = requiredOption(options, 'target', requiredTargetFlag)
  const submissionKey = options.values.submissionKey ?? randomUUID()
  const common = {
    ...(options.values.title === undefined ? {} : { title: options.values.title }),
    submissionKey,
    ...(options.booleans.has('--enter') ? { enter: true } : {})
  }

  if (entity === 'workspace') {
    return {
      method: 'structure.create.workspace',
      params: { path: targetText, ...common },
      json: options.booleans.has('--json')
    }
  }
  if (entity === 'task') {
    return {
      method: 'structure.create.task',
      params: { workspace: await parseActionTarget(targetText, request, 'workspace'), ...common },
      json: options.booleans.has('--json')
    }
  }
  if (entity === 'canvas') {
    return {
      method: 'structure.create.canvas',
      params: { task: await parseActionTarget(targetText, request, 'task'), ...common },
      json: options.booleans.has('--json')
    }
  }

  const profile = options.values.profile ?? 'shell'
  if (!['shell', 'claude-code', 'codex'].includes(profile)) {
    throw new MtUsageError('--profile 需要 shell、claude-code 或 codex')
  }
  return {
    method: 'structure.create.session',
    params: {
      canvas: await parseActionTarget(targetText, request, 'canvas'), profile, ...common
    },
    json: options.booleans.has('--json')
  }
}

async function parseForkCommand(
  argv: string[],
  request: MtRequest,
  readStdin: () => Promise<string>
): Promise<ParsedActionCommand> {
  const relation = argv[0]
  if (!['child', 'sibling', 'children'].includes(relation ?? '')) {
    throw new MtUsageError('fork 需要 child、sibling 或 children')
  }
  const sourceText = argv[1]
  if (!sourceText || sourceText.startsWith('--')) throw new MtUsageError(`fork ${relation} 需要源会话`)

  if (relation === 'children') {
    const options = parseOptions(
      argv.slice(2),
      {
        '--items-json': 'itemsJson',
        '--batch-key': 'batchKey',
        '--retry-item-keys-json': 'retryItemKeysJson',
        '--retry-items-json': 'retryItemKeysJson'
      },
      ['--json'],
      { '--retry-item-key': 'retryItemKeys' }
    )
    const rawItems = requiredOption(options, 'itemsJson', '--items-json')
    const itemsText = rawItems === '-'
      ? await boundedStdin(readStdin)
      : rawItems
    const items = parseJsonArray(itemsText, '--items-json') as ForkItemInput[]
    const repeatedRetryKeys = options.repeated.retryItemKeys ?? []
    const jsonRetryKeys = options.values.retryItemKeysJson === undefined
      ? []
      : parseJsonStringArray(options.values.retryItemKeysJson, '--retry-item-keys-json')
    const retryItemKeys = [...repeatedRetryKeys, ...jsonRetryKeys]
    return {
      method: 'structure.fork.children',
      params: {
        source: await parseActionTarget(sourceText, request),
        batchKey: options.values.batchKey ?? randomUUID(),
        items,
        ...(retryItemKeys.length === 0 ? {} : { retryItemKeys })
      },
      json: options.booleans.has('--json')
    }
  }

  const options = parseOptions(
    argv.slice(2),
    {
      '--title': 'title',
      '--environment-json': 'environmentJson',
      '--prompt': 'prompt',
      '--submission-key': 'submissionKey'
    },
    ['--start', '--json']
  )
  const environment = parseJsonObject(
    requiredOption(options, 'environmentJson', '--environment-json'),
    '--environment-json'
  ) as ForkEnvironmentChoice
  return {
    method: relation === 'child' ? 'structure.fork.child' : 'structure.fork.sibling',
    params: {
      source: await parseActionTarget(sourceText, request),
      title: requiredOption(options, 'title', '--title'),
      environment,
      ...(options.values.prompt === undefined ? {} : { prompt: options.values.prompt }),
      ...(options.booleans.has('--start') ? { start: true } : {}),
      submissionKey: options.values.submissionKey ?? randomUUID()
    },
    json: options.booleans.has('--json')
  }
}

async function parseRemoveCommand(
  argv: string[],
  request: MtRequest
): Promise<ParsedActionCommand> {
  const operation = argv[0]
  if (operation === 'preview') {
    const targetText = requiredPositional(argv, 1, 'remove preview 需要目标')
    const options = parseOptions(argv.slice(2), { '--scope': 'scope' }, ['--json'])
    const scope = requiredOption(options, 'scope', '--scope')
    if (scope !== 'node' && scope !== 'subtree') {
      throw new MtUsageError('--scope 需要 node 或 subtree')
    }
    return {
      method: 'structure.remove.preview',
      params: { target: await parseActionTarget(targetText, request), scope },
      json: options.booleans.has('--json')
    }
  }
  if (operation === 'commit') {
    const confirmationRef = requiredPositional(argv, 1, 'remove commit 需要确认引用')
    const options = parseOptions(argv.slice(2), {}, ['--json'])
    return {
      method: 'structure.remove.commit', params: { confirmationRef },
      json: options.booleans.has('--json')
    }
  }
  throw new MtUsageError('remove 需要 preview 或 commit')
}

async function parseCloseCommand(
  argv: string[],
  request: MtRequest
): Promise<ParsedActionCommand> {
  const operation = argv[0]
  if (operation === 'canvas-preview') {
    const targetText = requiredPositional(argv, 1, 'close canvas-preview 需要画布目标')
    const options = parseOptions(argv.slice(2), {}, ['--json'])
    return {
      method: 'structure.canvas-close.preview',
      params: { target: await parseActionTarget(targetText, request, 'canvas') },
      json: options.booleans.has('--json')
    }
  }
  if (operation === 'canvas-commit') {
    const confirmationRef = requiredPositional(argv, 1, 'close canvas-commit 需要确认引用')
    const options = parseOptions(argv.slice(2), {}, ['--json'])
    return {
      method: 'structure.canvas-close.commit', params: { confirmationRef },
      json: options.booleans.has('--json')
    }
  }
  throw new MtUsageError('close 需要 canvas-preview 或 canvas-commit')
}

async function parseNavigationCommand(
  argv: string[],
  request: MtRequest
): Promise<ParsedActionCommand> {
  if (argv[0] === 'focus') {
    const targetText = requiredPositional(argv, 1, 'focus 需要会话目标')
    const options = parseOptions(argv.slice(2), {}, ['--json'])
    return {
      method: 'navigation.focus.session',
      params: { target: await parseActionTarget(targetText, request, 'session') },
      json: options.booleans.has('--json')
    }
  }

  const entity = argv[1]
  if (!['workspace', 'task', 'canvas'].includes(entity ?? '')) {
    throw new MtUsageError('switch 需要 workspace、task 或 canvas')
  }
  const targetText = requiredPositional(argv, 2, `switch ${entity} 需要目标`)
  const options = parseOptions(argv.slice(3), {}, ['--json'])
  return {
    method: `navigation.switch.${entity}` as HostActionMethod,
    params: { target: await parseActionTarget(targetText, request, entity as 'workspace' | 'task' | 'canvas') },
    json: options.booleans.has('--json')
  }
}

async function parseActionTarget(
  target: string,
  request: MtRequest,
  currentEntity?: 'workspace' | 'task' | 'canvas' | 'session'
): Promise<HostEntitySelector | HostActionTargetSelector> {
  if (target === 'current') {
    if (!currentEntity) throw new MtUsageError('current 需要明确的层级')
    return { kind: 'current', entity: currentEntity }
  }
  if (target === 'self') return { kind: 'self' }

  const relative = target === 'left' || target === 'right'
    ? { kind: 'relative' as const, direction: target as 'left' | 'right' }
    : undefined
  const parent = target === 'parent'
    ? { kind: 'relation' as const, relation: 'parent' as const }
    : undefined
  const child = /^child:(\d+)$/.exec(target)
  const childRelation = child
    ? {
        kind: 'relation' as const,
        relation: 'child' as const,
        ordinal: positiveInteger(child[1]!, 'child ordinal')
      }
    : undefined
  const sibling = /^sibling:(\d+)$/.exec(target)
  const siblingSelector = sibling
    ? { kind: 'sibling' as const, ordinal: positiveInteger(sibling[1]!, 'sibling ordinal') }
    : undefined
  const positional = relative ?? parent ?? childRelation ?? siblingSelector
  if (positional) {
    const listing = asListing(await request('host.list', { scope: 'current-level' }))
    return { ...positional, projectionRevision: listing.projectionRevision }
  }

  const listing = asListing(await request('host.list', { scope: 'all' }))
  return { kind: 'ref', ref: target, projectionRevision: listing.projectionRevision }
}

interface ParsedOptions {
  values: Record<string, string | undefined>
  booleans: Set<string>
  repeated: Record<string, string[] | undefined>
}

function parseOptions(
  argv: string[],
  valueFlags: Readonly<Record<string, string>>,
  booleanFlags: readonly string[],
  repeatedValueFlags: Readonly<Record<string, string>> = {}
): ParsedOptions {
  const values: Record<string, string | undefined> = {}
  const booleans = new Set<string>()
  const repeated: Record<string, string[] | undefined> = {}
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index]!
    const valueName = valueFlags[flag]
    const repeatedName = repeatedValueFlags[flag]
    if (booleanFlags.includes(flag)) {
      if (booleans.has(flag)) throw new MtUsageError(`${flag} 不得重复`)
      booleans.add(flag)
      continue
    }
    if (valueName || repeatedName) {
      const value = argv[index + 1]
      if (value === undefined || value.startsWith('--')) throw new MtUsageError(`${flag} 需要值`)
      index += 1
      if (valueName) {
        if (values[valueName] !== undefined) throw new MtUsageError(`${flag} 不得重复`)
        values[valueName] = value
      } else if (repeatedName) {
        ;(repeated[repeatedName] ??= []).push(value)
      }
      continue
    }
    if (flag.startsWith('--')) throw new MtUsageError(`未知选项：${flag}`)
    throw new MtUsageError(`多余参数：${flag}`)
  }
  return { values, booleans, repeated }
}

function requiredOption(options: ParsedOptions, name: string, flag: string): string {
  const value = options.values[name]
  if (value === undefined) throw new MtUsageError(`${flag} 为必填项`)
  return value
}

function requiredPositional(argv: string[], index: number, message: string): string {
  const value = argv[index]
  if (!value || value.startsWith('--')) throw new MtUsageError(message)
  return value
}

function parseJsonObject(text: string, flag: string): Record<string, unknown> {
  const value = parseJson(text, flag)
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new MtUsageError(`${flag} 需要 JSON 对象`)
  }
  return value as Record<string, unknown>
}

function parseJsonArray(text: string, flag: string): unknown[] {
  const value = parseJson(text, flag)
  if (!Array.isArray(value)) throw new MtUsageError(`${flag} 需要 JSON 数组`)
  return value
}

function parseJsonStringArray(text: string, flag: string): string[] {
  const values = parseJsonArray(text, flag)
  if (!values.every((value) => typeof value === 'string')) {
    throw new MtUsageError(`${flag} 需要字符串 JSON 数组`)
  }
  return values as string[]
}

function parseJson(text: string, flag: string): unknown {
  if (!hasWellFormedUtf16(text)) throw new MtUsageError(`${flag} 包含无效 UTF-8 文本`)
  try {
    return JSON.parse(text)
  } catch {
    throw new MtUsageError(`${flag} 需要有效 JSON`)
  }
}

const MAX_STDIN_BYTES = 1024 * 1024

async function boundedStdin(readStdin: () => Promise<string>): Promise<string> {
  const input = await readStdin()
  if (!hasWellFormedUtf16(input)) throw new MtUsageError('标准输入包含无效 UTF-8 文本')
  if (Buffer.byteLength(input, 'utf8') > MAX_STDIN_BYTES) {
    throw new MtUsageError('标准输入超过 1 MiB 上限')
  }
  return input
}

async function readProcessStdin(): Promise<string> {
  const chunks: Buffer[] = []
  let total = 0
  for await (const rawChunk of process.stdin) {
    const chunk = Buffer.isBuffer(rawChunk) ? rawChunk : Buffer.from(rawChunk)
    total += chunk.byteLength
    if (total > MAX_STDIN_BYTES) throw new MtUsageError('标准输入超过 1 MiB 上限')
    chunks.push(chunk)
  }
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks))
  } catch {
    throw new MtUsageError('标准输入包含无效 UTF-8 文本')
  }
}

function hasWellFormedUtf16(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1)
      if (next < 0xdc00 || next > 0xdfff) return false
      index += 1
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return false
    }
  }
  return true
}

async function parseTarget(target: string, request: MtRequest): Promise<HostTargetSelector> {
  if (target === 'self') return { kind: 'self' }
  if (target === 'left' || target === 'right') return { kind: 'relative', direction: target }
  if (target === 'parent') return { kind: 'relation', relation: 'parent' }
  const child = /^child:(\d+)$/.exec(target)
  if (child) return { kind: 'relation', relation: 'child', ordinal: positiveInteger(child[1]!, 'child ordinal') }
  const sibling = /^sibling:(\d+)$/.exec(target)
  if (sibling) {
    const listing = asListing(await request('host.list', { scope: 'current-level' }))
    return {
      kind: 'sibling', ordinal: positiveInteger(sibling[1]!, 'sibling ordinal'),
      projectionRevision: listing.projectionRevision
    }
  }
  const listing = asListing(await request('host.list', { scope: 'all' }))
  if (!listing.targets.some(({ ref }) => ref === target)) {
    throw new HostControlClientError('TARGET_NOT_FOUND', `目标 ${target} 不在当前 Matou 拓扑中`)
  }
  return { kind: 'ref', ref: target, projectionRevision: listing.projectionRevision }
}

function asListing(value: unknown): { projectionRevision: string; targets: HostTarget[] } {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new HostControlClientError('INVALID_RESPONSE', 'Host Control 列表响应格式错误')
  }
  const listing = value as { projectionRevision?: unknown; targets?: unknown }
  if (typeof listing.projectionRevision !== 'string' || !Array.isArray(listing.targets)) {
    throw new HostControlClientError('INVALID_RESPONSE', 'Host Control 列表响应缺少投影版本')
  }
  return { projectionRevision: listing.projectionRevision, targets: listing.targets as HostTarget[] }
}

function printResult(
  result: unknown,
  json: boolean,
  io: MtIo,
  format: (value: unknown) => string
): void {
  io.stdout(json ? JSON.stringify(result, null, 2) : format(result))
}

function formatIdentity(value: unknown): string {
  const target = (value as { target?: HostTarget })?.target
  if (!target) return '已识别当前 Matou 会话'
  return [
    `窗口 ${target.window.ordinal}`,
    `工作空间 ${target.workspace.name}`,
    `事项 ${target.task.name}`,
    `画布 ${target.canvas.name}`,
    `会话 ${target.session.ordinal}（${target.title}）`
  ].join(' / ')
}

function formatList(value: unknown): string {
  const targets = (value as { targets?: HostTarget[] })?.targets ?? []
  if (targets.length === 0) return '当前范围内没有会话'
  return targets.map((target) => [
    `${target.session.ordinal}. ${target.title}`,
    `[${target.profile}]`,
    target.cwd,
    target.session.detached ? '（独立窗口）' : ''
  ].filter(Boolean).join(' ')).join('\n')
}

function formatTerminalText(value: unknown): string {
  return typeof (value as { text?: unknown })?.text === 'string'
    ? (value as { text: string }).text
    : ''
}

function formatCommands(value: unknown): string {
  if (!Array.isArray(value)) return ''
  return value.map((item, index) => {
    if (typeof item === 'string') return `${index + 1}. ${item}`
    if (typeof item === 'object' && item !== null) {
      const record = item as Record<string, unknown>
      const command = record.command ?? record.text ?? record.input
      if (typeof command === 'string') return `${index + 1}. ${command}`
    }
    return `${index + 1}. ${JSON.stringify(item)}`
  }).join('\n')
}

function formatActionResult(value: unknown): string {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return '操作已完成'
  const result = value as Record<string, unknown>
  if (result.kind === 'created') {
    const entityNames: Record<string, string> = {
      workspace: '工作空间', task: '事项', canvas: '画布', session: '会话'
    }
    const entity = typeof result.entity === 'string' ? (entityNames[result.entity] ?? '对象') : '对象'
    const path = formatResultPath(result.path)
    const focused = formatResultPath(result.focusedPath)
    return `已创建${entity}${path ? `：${path}` : ''}${focused ? `\n当前焦点：${focused}` : ''}`
  }
  if (result.kind === 'forked') {
    const environment = formatEnvironment(result.environment)
    const state = formatForkState(result.state)
    const path = formatResultPath(result.path)
    return [path || '新会话', `环境：${environment}`, `状态：${state}`].join(' | ')
  }
  if (result.kind === 'fork-batch') return formatForkBatch(value as ForkBatchResult)
  if (result.kind === 'removal-preview' || result.kind === 'canvas-close-preview') {
    return formatImpactPreview(result.impact)
  }
  if (result.kind === 'removed') {
    return [
      `已移除：事项 ${numberField(result.removedTasks)}，画布 ${numberField(result.removedCanvases)}，会话 ${numberField(result.removedSessions)}`,
      `当前位置：${formatResultPath(result.activePath) || '已更新'}`,
      '项目文件、Git 分支和 Worktree 保持不变。'
    ].join('\n')
  }
  if (result.kind === 'canvas-closed') {
    return [
      `已关闭画布，结束会话 ${numberField(result.removedSessions)} 个。`,
      `当前位置：${formatResultPath(result.activePath) || '已更新'}`,
      '项目文件、Git 分支和 Worktree 保持不变。'
    ].join('\n')
  }
  if (result.kind === 'navigated') return '已切换到目标位置并完成聚焦'
  return '操作已完成'
}

function formatForkBatch(result: ForkBatchResult): string {
  const lines = result.items.map((item, index) => {
    const state = formatForkState(item.state)
    return `${index + 1}. ${item.title} | 环境：${formatEnvironment(item.environment)} | 状态：${state}`
  })
  lines.push(`汇总：成功 ${result.succeeded} 项，失败 ${result.failed} 项。`)
  return lines.join('\n')
}

function formatEnvironment(value: unknown): string {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return '未知环境'
  const environment = value as Record<string, unknown>
  if (environment.mode === 'current') return '当前执行环境'
  const branch = typeof environment.branch === 'string' ? environment.branch : '未命名分支'
  if (environment.mode === 'new-worktree') return `新 Worktree（${branch}）`
  if (environment.mode === 'existing-worktree') return `现有 Worktree（${branch}）`
  return '未知环境'
}

function formatForkState(value: unknown): string {
  const states: Record<string, string> = {
    created: '已创建', ready: '已就绪', started: '已启动', failed: '失败'
  }
  return typeof value === 'string' ? (states[value] ?? value) : '未知'
}

function formatImpactPreview(value: unknown): string {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return '已生成影响预览。\n项目文件、Git 分支和 Worktree 保持不变。'
  }
  const impact = value as unknown as HostImpactSummary
  const target = formatResultPath(impact.target)
  const scope = impact.scope === 'subtree' ? '当前节点及全部子节点' : '当前节点'
  return [
    `预览目标：${target || '已选定对象'}`,
    `操作范围：${scope}`,
    `影响：事项 ${numberField(impact.tasks)}，画布 ${numberField(impact.canvases)}，会话 ${numberField(impact.sessions)}，子节点 ${numberField(impact.descendants)}`,
    `将结束：运行或等待中会话 ${numberField(impact.liveRuns)}，终端进程 ${numberField(impact.terminalProcesses)}`,
    '项目文件、Git 分支和 Worktree 保持不变。'
  ].join('\n')
}

function formatResultPath(value: unknown): string {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return ''
  const path = value as Partial<HostResultPath>
  const labels: string[] = []
  if (path.window && typeof path.window.title === 'string') labels.push(`窗口 ${path.window.title}`)
  if (path.workspace && typeof path.workspace.title === 'string') {
    const directory = typeof path.workspace.path === 'string' ? `（${path.workspace.path}）` : ''
    labels.push(`工作空间 ${path.workspace.title}${directory}`)
  }
  if (path.task && typeof path.task.title === 'string') labels.push(`事项 ${path.task.title}`)
  if (path.canvas && typeof path.canvas.title === 'string') labels.push(`画布 ${path.canvas.title}`)
  if (path.session && typeof path.session.title === 'string') labels.push(`会话 ${path.session.title}`)
  return labels.join(' / ')
}

function numberField(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0
}

function partialSuccessExitCode(value: unknown): number {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return 0
  const result = value as { kind?: unknown; failed?: unknown }
  return result.kind === 'fork-batch' && typeof result.failed === 'number' && result.failed > 0 ? 6 : 0
}

function formatError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  if (!(error instanceof HostControlClientError) || error.code !== 'AMBIGUOUS_TARGET') return message
  const candidates = error.details?.candidates ?? []
  if (candidates.length > 5) {
    return `${message}\n候选项超过 5 个，请补充筛选条件，例如工作空间、事项、画布、会话标题或路径。`
  }
  if (candidates.length >= 2) {
    return `${message}\n候选项：\n${candidates.map(({ humanPath }, index) => `${index + 1}. ${humanPath}`).join('\n')}`
  }
  return message
}

function numberFlag(
  argv: string[], flag: string, fallback: number, minimum: number, maximum: number
): number {
  const index = argv.indexOf(flag)
  if (index < 0) return fallback
  const value = positiveInteger(argv[index + 1] ?? '', flag)
  if (value < minimum || value > maximum) throw new MtUsageError(`${flag} 需要 ${minimum}-${maximum} 的整数`)
  return value
}

function positiveInteger(value: string, label: string): number {
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed) || parsed < 1) throw new MtUsageError(`${label} 需要正整数`)
  return parsed
}

function positionalTail(argv: string[], booleanFlags: string[]): string {
  return argv.filter((value) => !booleanFlags.includes(value)).join(' ').trim()
}

function normalizeKey(value: string): AllowedControlKey {
  const aliases: Record<string, AllowedControlKey> = {
    enter: 'Enter', return: 'Enter', tab: 'Tab', escape: 'Escape', esc: 'Escape',
    backspace: 'Backspace', delete: 'Delete', up: 'ArrowUp', arrowup: 'ArrowUp',
    down: 'ArrowDown', arrowdown: 'ArrowDown', left: 'ArrowLeft', arrowleft: 'ArrowLeft',
    right: 'ArrowRight', arrowright: 'ArrowRight', home: 'Home', end: 'End',
    pageup: 'PageUp', pagedown: 'PageDown', 'ctrl+c': 'CtrlC', 'ctrl+d': 'CtrlD',
    'ctrl+l': 'CtrlL', 'ctrl+u': 'CtrlU', 'ctrl+z': 'CtrlZ'
  }
  const key = aliases[value.trim().toLowerCase()]
  if (!key) throw new MtUsageError(`不支持的按键：${value}`)
  return key
}

function exitCode(error: unknown): number {
  if (error instanceof MtUsageError) return 2
  if (!(error instanceof HostControlClientError)) return 1
  if (error.code === 'INVALID_REQUEST') return 2
  if (['TARGET_NOT_FOUND', 'AMBIGUOUS_TARGET', 'STALE_PROJECTION', 'CONFLICT'].includes(error.code)) return 3
  if ([
    'CAPABILITY_DENIED', 'TARGET_NOT_READY', 'UNSUPPORTED',
    'CONFIRMATION_REQUIRED', 'CONFIRMATION_EXPIRED', 'CONFIRMATION_STALE',
    'PATH_CONFLICT', 'BRANCH_CONFLICT', 'WORKTREE_CONFLICT', 'STORAGE_READ_ONLY'
  ].includes(error.code)) return 4
  if (['TIMEOUT', 'CONNECTION_ERROR', 'NAVIGATION_TIMEOUT'].includes(error.code)) return 5
  if (error.code === 'PARTIAL_SUCCESS') return 6
  return 1
}

class MtUsageError extends Error {}

const HELP_TEXT = `Matou Host Control

Usage:
  mt identify [--json]
  mt list [--all] [--json]
  mt read TARGET [--lines N] [--bytes N] [--json]
  mt history TARGET [--lines N] [--bytes N] [--json]
  mt commands TARGET [--limit N] [--json]
  mt send TARGET TEXT [--enter] [--json]
  mt key TARGET KEY [--json]

  mt create workspace --path PATH [--title TITLE] [--submission-key KEY] [--enter] [--json]
  mt create task --workspace TARGET [--title TITLE] [--submission-key KEY] [--enter] [--json]
  mt create canvas --task TARGET [--title TITLE] [--submission-key KEY] [--enter] [--json]
  mt create session --canvas TARGET [--profile shell|claude-code|codex] [--title TITLE] [--submission-key KEY] [--enter] [--json]

  mt fork child SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start] [--submission-key KEY] [--json]
  mt fork sibling SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start] [--submission-key KEY] [--json]
  mt fork children SOURCE --items-json JSON|- [--batch-key KEY] [--retry-item-key KEY] [--json]

  mt remove preview TARGET --scope node|subtree [--json]
  mt remove commit CONFIRMATION_REF [--json]
  mt close canvas-preview TARGET [--json]
  mt close canvas-commit CONFIRMATION_REF [--json]

  mt focus TARGET [--json]
  mt switch workspace TARGET [--json]
  mt switch task TARGET [--json]
  mt switch canvas TARGET [--json]

Targets: current (where supported), self, left, right, parent, child:N, sibling:N, or a ref from mt list --json

Batch JSON can be read from standard input with --items-json - (maximum 1 MiB).`

if (typeof require !== 'undefined' && require.main === module) {
  void runMt(
    process.argv.slice(2),
    process.env,
    {
      stdout: (text) => process.stdout.write(`${text}\n`),
      stderr: (text) => process.stderr.write(`${text}\n`)
    }
  ).then((code) => { process.exitCode = code })
}
