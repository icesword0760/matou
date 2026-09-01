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

export async function runMt(
  argv: string[],
  environment: MtEnvironment,
  io: MtIo,
  injectedRequest?: MtRequest
): Promise<number> {
  try {
    if (argv.length === 0 || argv.includes('--help') || argv.includes('-h')) {
      io.stdout(HELP_TEXT)
      return 0
    }
    const request = injectedRequest ?? requestFromEnvironment(environment)
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
    io.stderr(error instanceof Error ? error.message : String(error))
    return exitCode(error)
  }
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
  if (['TARGET_NOT_FOUND', 'AMBIGUOUS_TARGET', 'CONFLICT'].includes(error.code)) return 3
  if (['CAPABILITY_DENIED', 'TARGET_NOT_READY', 'UNSUPPORTED'].includes(error.code)) return 4
  if (['TIMEOUT', 'CONNECTION_ERROR'].includes(error.code)) return 5
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

Targets: self, left, right, parent, child:N, sibling:N, or a ref from mt list --json`

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
