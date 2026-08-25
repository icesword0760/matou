export interface ProviderNotificationEvent {
  eventType: 'completed' | 'permission' | 'error' | 'waiting' | 'attention'
  title: 'Claude Code'
  subtitle: string
  body: string
  sound: boolean
  cooldownKey: 'Stop' | 'Notification'
}

const TITLE = 'Claude Code' as const

export function toProviderNotificationEvent(
  payload: Record<string, unknown>
): ProviderNotificationEvent | null {
  const hookEvent = text(payload.hook_event_name) ?? text(payload.hookEventName)
  if (hookEvent === 'Stop') return stopNotification(payload)
  if (hookEvent === 'Notification') return hookNotification(payload)
  return null
}

function stopNotification(payload: Record<string, unknown>): ProviderNotificationEvent {
  const projectName = tailName(text(payload.cwd))
  const latestAnswer = text(payload.last_assistant_message) ??
    text(payload.lastAssistantMessage) ??
    text(payload.result)
  return {
    eventType: 'completed',
    title: TITLE,
    subtitle: projectName ? `Completed in ${projectName}` : 'Completed',
    body: compact(
      latestAnswer ?? (projectName
        ? `Claude session completed in ${projectName}`
        : 'Claude session completed'),
      200
    ),
    sound: true,
    cooldownKey: 'Stop'
  }
}

function hookNotification(payload: Record<string, unknown>): ProviderNotificationEvent {
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
  } else if (/error|failed|exception/.test(lower)) {
    eventType = 'error'
    subtitle = 'Error'
  } else if (/complet|finish|done|success/.test(lower)) {
    eventType = 'completed'
    subtitle = 'Completed'
  } else if (/idle|wait|input/.test(lower)) {
    eventType = 'waiting'
    subtitle = 'Waiting'
  }
  return {
    eventType,
    title: TITLE,
    subtitle,
    body: compact(raw ?? 'Claude needs your attention', 180),
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
