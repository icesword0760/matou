import { createHash } from 'node:crypto'

import type { AgentSemanticEvent, AgentSemanticKind } from '@matou/domain'

export interface AdapterDiagnostic {
  code: 'MALFORMED_PROVIDER_EVENT' | 'UNSUPPORTED_PROVIDER_EVENT'
  message: string
}

export interface AdapterResult {
  events: AgentSemanticEvent[]
  diagnostics: AdapterDiagnostic[]
}

export class ClaudeCodeAdapter {
  normalize(sessionId: string, raw: unknown): AdapterResult {
    const envelope = asRecord(raw)
    if (!envelope) return malformed('Claude hook payload must be an object')
    const data = asRecord(envelope.data) ?? envelope
    const eventName = stringValue(envelope.event) ?? stringValue(data.hook_event_name)
    const occurredAt = numberValue(envelope.timestamp) ?? numberValue(data.timestamp)
    if (!eventName || occurredAt === undefined) {
      return malformed('Claude hook event name and timestamp are required')
    }

    if (eventName === 'PreToolUse' || eventName === 'PostToolUse') {
      const toolUseId = stringValue(data.tool_use_id)
      const toolName = stringValue(data.tool_name)
      if (!toolUseId || !toolName) return malformed(`${eventName} requires tool_use_id and tool_name`)
      const toolInput = asRecord(data.tool_input) ?? {}
      const events: AgentSemanticEvent[] = [
        semanticEvent({
          provider: 'claude-code',
          sessionId,
          providerEventId: toolUseId,
          source: 'structured',
          confidence: 'high',
          kind: eventName === 'PreToolUse' ? 'agent.tool-started' : 'agent.tool-finished',
          payload: {
            toolName,
            input: toolInput,
            ...(eventName === 'PostToolUse' ? { response: data.tool_response } : {})
          },
          occurredAt
        })
      ]
      if (eventName === 'PreToolUse' && toolName === 'TodoWrite' && Array.isArray(toolInput.todos)) {
        toolInput.todos.forEach((todo, index) => {
          if (!asRecord(todo)) return
          events.push(
            semanticEvent({
              provider: 'claude-code',
              sessionId,
              providerEventId: `${toolUseId}:todo:${index}`,
              source: 'structured',
              confidence: 'high',
              kind: 'agent.todo',
              payload: todo,
              occurredAt
            })
          )
        })
      }
      if (eventName === 'PostToolUse' && ['Write', 'Edit', 'MultiEdit'].includes(toolName)) {
        const path = stringValue(toolInput.file_path) ?? stringValue(toolInput.path)
        if (path) {
          events.push(
            semanticEvent({
              provider: 'claude-code',
              sessionId,
              providerEventId: `${toolUseId}:file:${path}`,
              source: 'structured',
              confidence: 'high',
              kind: 'file.changed',
              payload: { path, toolName },
              occurredAt
            })
          )
        }
      }
      return { events, diagnostics: [] }
    }

    if (eventName === 'Stop' || eventName === 'SessionEnd') {
      const assistant = asRecord(envelope.latestAssistantTurn) ?? asRecord(data.latestAssistantTurn)
      const id = assistant && stringValue(assistant.id)
      const text = assistant && (stringValue(assistant.text) ?? extractText(assistant.content))
      if (!id || !text) return malformed(`${eventName} requires a persisted assistant turn id and text`)
      return success(
        semanticEvent({
          provider: 'claude-code',
          sessionId,
          providerEventId: id,
          source: 'transcript',
          confidence: 'medium',
          kind: 'agent.message',
          payload: { text, terminalState: eventName === 'Stop' ? 'waiting' : 'exited' },
          occurredAt
        })
      )
    }

    if (eventName === 'Notification') {
      const notificationType = stringValue(data.notification_type)
      if (notificationType === 'permission_prompt') {
        const id = stringValue(data.id) ?? fallbackProviderId(data)
        return success(
          semanticEvent({
            provider: 'claude-code',
            sessionId,
            providerEventId: id,
            source: 'structured',
            confidence: 'high',
            kind: 'agent.permission-requested',
            payload: data,
            occurredAt
          })
        )
      }
    }

    return unsupported(`Claude hook event ${eventName} has no semantic mapping`)
  }
}

export class CodexAdapter {
  normalize(sessionId: string, raw: unknown): AdapterResult {
    const event = asRecord(raw)
    const item = event && asRecord(event.item)
    const type = event && stringValue(event.type)
    const occurredAt = event && numberValue(event.timestamp)
    const itemId = item && stringValue(item.id)
    const itemType = item && stringValue(item.type)
    if (!event || !item || !type || occurredAt === undefined || !itemId || !itemType) {
      return malformed('Codex event requires type, timestamp, and item identity')
    }

    let kind: AgentSemanticKind | undefined
    if (itemType === 'agent_message' && type === 'item.completed') kind = 'agent.message'
    else if (type === 'item.started') kind = 'agent.tool-started'
    else if (type === 'item.completed') kind = 'agent.tool-finished'
    if (!kind) return unsupported(`Codex event ${type}/${itemType} has no semantic mapping`)

    return success(
      semanticEvent({
        provider: 'codex',
        sessionId,
        providerEventId: itemId,
        source: 'structured',
        confidence: 'high',
        kind,
        payload: item,
        occurredAt
      })
    )
  }
}

export class GenericShellAdapter {
  normalizeMarker(sessionId: string, raw: unknown): AdapterResult {
    const marker = asRecord(raw)
    const id = marker && stringValue(marker.id)
    const kind = marker && stringValue(marker.kind)
    const occurredAt = marker && numberValue(marker.timestamp)
    if (!marker || !id || !isSemanticKind(kind) || occurredAt === undefined) {
      return malformed('semantic marker requires id, supported kind, and timestamp')
    }
    return success(
      semanticEvent({
        provider: 'generic',
        sessionId,
        providerEventId: id,
        source: 'terminal-marker',
        confidence: 'high',
        kind,
        payload: marker.payload,
        occurredAt
      })
    )
  }

  normalizeParsedText(sessionId: string, text: string, occurredAt: number): AdapterResult {
    const normalized = text.trim()
    if (!normalized) return unsupported('empty terminal text has no semantic meaning')
    const kind: AgentSemanticKind = /(?:tests?|checks?).*(?:pass|✓)|✓.*(?:tests?|checks?)/i.test(normalized)
      ? 'validation.status-changed'
      : 'agent.message'
    return success(
      semanticEvent({
        provider: 'generic',
        sessionId,
        providerEventId: `terminal:${fallbackProviderId(normalized)}`,
        source: 'terminal-parse',
        confidence: 'low',
        kind,
        payload: { text: normalized },
        occurredAt
      })
    )
  }
}

function semanticEvent(input: Omit<AgentSemanticEvent, 'eventId' | 'sourceRef'> & {
  providerEventId: string
  source: AgentSemanticEvent['sourceRef']['source']
}): AgentSemanticEvent {
  return {
    eventId: createHash('sha256')
      .update(`${input.provider}\0${input.sessionId}\0${input.providerEventId}\0${input.kind}`)
      .digest('hex'),
    sessionId: input.sessionId,
    kind: input.kind,
    provider: input.provider,
    sourceRef: { providerEventId: input.providerEventId, source: input.source },
    confidence: input.confidence,
    payload: input.payload,
    occurredAt: input.occurredAt
  }
}

function success(event: AgentSemanticEvent): AdapterResult {
  return { events: [event], diagnostics: [] }
}
function malformed(message: string): AdapterResult {
  return { events: [], diagnostics: [{ code: 'MALFORMED_PROVIDER_EVENT', message }] }
}
function unsupported(message: string): AdapterResult {
  return { events: [], diagnostics: [{ code: 'UNSUPPORTED_PROVIDER_EVENT', message }] }
}
function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined
}
function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined
}
function numberValue(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}
function extractText(content: unknown): string | undefined {
  if (typeof content === 'string') return stringValue(content)
  if (!Array.isArray(content)) return undefined
  const parts = content
    .map(asRecord)
    .filter((part): part is Record<string, unknown> => part !== undefined)
    .filter((part) => part.type === 'text')
    .map((part) => stringValue(part.text))
    .filter((part): part is string => part !== undefined)
  return parts.length ? parts.join('\n') : undefined
}
function fallbackProviderId(value: unknown): string {
  return createHash('sha256').update(canonicalJson(value)).digest('hex')
}
function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`
  if (value && typeof value === 'object') {
    return `{${Object.entries(value as Record<string, unknown>)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, child]) => `${JSON.stringify(key)}:${canonicalJson(child)}`)
      .join(',')}}`
  }
  return JSON.stringify(value) ?? 'undefined'
}
function isSemanticKind(value: string | undefined): value is AgentSemanticKind {
  return value !== undefined && [
    'agent.message', 'agent.todo', 'agent.tool-started', 'agent.tool-finished',
    'agent.permission-requested', 'file.changed', 'artifact.observed',
    'validation.status-changed'
  ].includes(value)
}
