import {
  getAgentDisplayName,
  getAgentShortName,
  type AgentProfileId
} from '@matou/domain'

export interface ProviderNotificationEvent {
  eventType: 'completed' | 'permission' | 'error' | 'waiting' | 'attention'
  title: string
  subtitle: string
  body: string
  sound: boolean
  cooldownKey: 'Stop' | 'Notification'
  replacementKey?: string
}

export function toProviderNotificationEvent(
  payload: Record<string, unknown>,
  provider: AgentProfileId = 'claude-code'
): ProviderNotificationEvent | null {
  const title = getAgentDisplayName(provider, 'Claude Code')
  const shortName = getAgentShortName(provider, 'Claude')
  const hookEvent = text(payload.hook_event_name) ?? text(payload.hookEventName)
  if (hookEvent === 'Stop') return stopNotification(payload, title, shortName)
  if (hookEvent === 'PermissionRequest') return permissionRequestNotification(payload, title)
  if (hookEvent === 'Notification') return hookNotification(payload, title, shortName)
  return null
}

function permissionRequestNotification(
  payload: Record<string, unknown>,
  title: string
): ProviderNotificationEvent {
  const tool = text(payload.tool_name) ?? text(payload.toolName) ?? 'operation'
  const detail = firstText([
    payload.message,
    payload.description,
    nested(payload.tool_input, 'command'),
    nested(payload.tool_input, 'file_path'),
    nested(payload.toolInput, 'command'),
    nested(payload.toolInput, 'filePath')
  ])
  return {
    eventType: 'permission',
    title,
    subtitle: 'Permission',
    body: compact(detail ? `${tool}: ${detail}` : `${tool} requires approval`, 180),
    sound: true,
    cooldownKey: 'Notification'
  }
}

function stopNotification(
  payload: Record<string, unknown>,
  title: string,
  shortName: string
): ProviderNotificationEvent {
  const projectName = tailName(text(payload.cwd))
  const latestAnswer = text(payload.last_assistant_message) ??
    text(payload.lastAssistantMessage) ??
    text(payload.result)
  return {
    eventType: 'completed',
    title,
    subtitle: projectName ? `Completed in ${projectName}` : 'Completed',
    body: compact(
      latestAnswer ?? (projectName
        ? `${shortName} session completed in ${projectName}`
        : `${shortName} session completed`),
      200
    ),
    sound: true,
    cooldownKey: 'Stop'
  }
}

function hookNotification(
  payload: Record<string, unknown>,
  title: string,
  shortName: string
): ProviderNotificationEvent {
  const raw = firstText([
    payload.message,
    payload.body,
    payload.text,
    payload.prompt,
    payload.error,
    payload.description,
    nested(payload.notification, 'message'),
    nested(payload.data, 'message')
  ])
  const lower = (raw ?? '').toLowerCase()
  let eventType: ProviderNotificationEvent['eventType'] = 'attention'
  let subtitle = 'Attention'
  if (/permission|approve|approval/.test(lower)) {
    eventType = 'permission'
    subtitle = 'Permission'
  } else if (/complet|finish|done|success/.test(lower)) {
    eventType = 'completed'
    subtitle = 'Completed'
  } else if (/idle|wait|input/.test(lower)) {
    eventType = 'waiting'
    subtitle = 'Waiting'
  }
  return {
    eventType,
    title,
    subtitle,
    body: compact(raw ?? `${shortName} needs your attention`, 180),
    sound: true,
    cooldownKey: 'Notification'
  }
}

function firstText(values: unknown[]): string | undefined {
  for (const value of values) {
    const result = text(value)
    if (result) return result
  }
  return undefined
}

function nested(value: unknown, key: string): unknown {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)[key]
    : undefined
}

function text(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  return trimmed || undefined
}

function tailName(path: string | undefined): string {
  if (!path) return ''
  const normalized = path.replace(/[/\\]+$/, '')
  return normalized.split(/[/\\]/).filter(Boolean).at(-1) ?? ''
}

function compact(value: string, maxLength: number): string {
  const singleLine = value.replace(/\s+/g, ' ').trim()
  return singleLine.length <= maxLength
    ? singleLine
    : `${singleLine.slice(0, maxLength - 1)}…`
}
