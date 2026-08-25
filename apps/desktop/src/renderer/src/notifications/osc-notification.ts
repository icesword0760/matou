import type { AgentNotificationInput } from './AgentNotificationStore'

export function toOscNotification(oscId: number, content: string): Omit<AgentNotificationInput, 'eventId'> | null {
  if (oscId !== 9 && oscId !== 99 && oscId !== 777) return null
  return {
    eventType: 'osc-notification',
    title: 'Claude Code',
    subtitle: 'Terminal',
    body: compact(content, 180) || '终端通知',
    sound: true,
    cooldownKey: 'OSCNotification'
  }
}

function compact(value: string, maxLength: number): string {
  const singleLine = value.replace(/\s+/g, ' ').trim()
  return singleLine.length <= maxLength ? singleLine : `${singleLine.slice(0, maxLength - 1)}…`
}
