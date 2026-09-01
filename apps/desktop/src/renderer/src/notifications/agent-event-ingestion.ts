import type { DomainEventWireEnvelope } from '@matou/contracts'

import type { HierarchyProjection } from '../hierarchy/hierarchy-types'
import { AgentNotificationStore, type AgentNotificationInput } from './AgentNotificationStore'

export function ingestAgentNotification(
  envelope: DomainEventWireEnvelope,
  projection: HierarchyProjection,
  focusedSessionId: string | undefined,
  store: AgentNotificationStore
): boolean {
  if (envelope.eventType !== 'agent.notification') return false
  const payload = object(envelope.payload)
  const providerEvent = object(payload?.event)
  if (!providerEvent) return false
  const replacementKey = text(providerEvent.replacementKey)
  if (replacementKey?.startsWith('provider-restore:')) {
    store.removeByReplacementKey(replacementKey)
    return true
  }
  if (text(providerEvent.operation) === 'dismiss') {
    if (replacementKey) store.removeByReplacementKey(replacementKey)
    return true
  }
  const sessionId = text(envelope.sessionId) ?? text(payload?.targetSessionId) ?? text(envelope.aggregateId)
  const sceneId = projection.sceneSnapshots?.find(({ mounts }) =>
    mounts.some(({ sessionId: mounted }) => mounted === sessionId)
  )?.scene.id ?? null
  const input: AgentNotificationInput = {
    eventId: envelope.eventId,
    eventType: text(providerEvent.eventType) ?? 'attention',
    title: text(providerEvent.title) ?? 'Claude Code',
    subtitle: text(providerEvent.subtitle) ?? '',
    body: text(providerEvent.body) ?? '',
    workspaceId: envelope.workspaceId ?? null,
    taskId: envelope.taskId ?? null,
    sceneId,
    sessionId: sessionId ?? null,
    sound: providerEvent.sound !== false,
    ...(text(providerEvent.cooldownKey) ? { cooldownKey: text(providerEvent.cooldownKey)! } : {}),
    ...(replacementKey ? { replacementKey } : {}),
    isFocusedSession: Boolean(sessionId && sessionId === focusedSessionId),
    teamRole: text(providerEvent.teamRole) ?? '',
    teamStatus: text(providerEvent.teamStatus) ?? '',
    teamStatusTone: text(providerEvent.teamStatusTone) ?? ''
  }
  store.push(input)
  return true
}

function object(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function text(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  return trimmed || undefined
}
