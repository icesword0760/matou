import { describe, expect, it } from 'vitest'

import { ClaudeCodeAdapter, CodexAdapter, GenericShellAdapter } from './agent-adapters'

describe('ClaudeCodeAdapter', () => {
  const adapter = new ClaudeCodeAdapter()

  it('normalizes tool lifecycle using stable tool_use_id identity', () => {
    const raw = {
      hook_event_name: 'PostToolUse',
      session_id: 'provider-session',
      tool_use_id: 'tool-use-1',
      tool_name: 'Read',
      tool_input: { file_path: '/tmp/a.ts' },
      tool_response: { ok: true },
      timestamp: 100
    }

    const first = adapter.normalize('session-1', raw)
    const second = adapter.normalize('session-1', raw)

    expect(first.diagnostics).toEqual([])
    expect(first.events).toEqual(second.events)
    expect(first.events[0]).toMatchObject({
      sessionId: 'session-1',
      kind: 'agent.tool-finished',
      provider: 'claude-code',
      sourceRef: { providerEventId: 'tool-use-1', source: 'structured' },
      confidence: 'high'
    })
  })

  it('expands TodoWrite into stable semantic todo events', () => {
    const result = adapter.normalize('session-1', {
      hook_event_name: 'PreToolUse',
      tool_use_id: 'todo-call-1',
      tool_name: 'TodoWrite',
      tool_input: {
        todos: [
          { content: 'Implement storage', status: 'in_progress' },
          { content: 'Run tests', status: 'pending' }
        ]
      },
      timestamp: 101
    })

    expect(result.events.map(({ kind }) => kind)).toEqual([
      'agent.tool-started', 'agent.todo', 'agent.todo'
    ])
    expect(new Set(result.events.map(({ eventId }) => eventId)).size).toBe(3)
  })

  it('normalizes Stop transcript output but reports malformed payloads in isolation', () => {
    expect(adapter.normalize('session-1', {
      hook_event_name: 'Stop',
      latestAssistantTurn: { id: 'message-1', text: 'Finished the task' },
      timestamp: 102
    }).events[0]).toMatchObject({
      kind: 'agent.message',
      sourceRef: { providerEventId: 'message-1', source: 'transcript' },
      confidence: 'medium'
    })

    const malformed = adapter.normalize('session-1', { hook_event_name: 'PostToolUse' })
    expect(malformed.events).toEqual([])
    expect(malformed.diagnostics[0]?.code).toBe('MALFORMED_PROVIDER_EVENT')
  })
})

describe('CodexAdapter', () => {
  it('normalizes structured item lifecycle and messages', () => {
    const adapter = new CodexAdapter()
    const started = adapter.normalize('session-1', {
      type: 'item.started',
      item: { id: 'item-1', type: 'command_execution', command: 'pnpm test' },
      timestamp: 200
    })
    const message = adapter.normalize('session-1', {
      type: 'item.completed',
      item: { id: 'message-2', type: 'agent_message', text: 'Tests pass' },
      timestamp: 201
    })

    expect(started.events[0]).toMatchObject({ kind: 'agent.tool-started', provider: 'codex' })
    expect(message.events[0]).toMatchObject({ kind: 'agent.message', provider: 'codex' })
  })
})

describe('GenericShellAdapter', () => {
  it('accepts explicit semantic markers and labels terminal parsing as low confidence', () => {
    const adapter = new GenericShellAdapter()
    expect(adapter.normalizeMarker('session-1', {
      id: 'marker-1', kind: 'artifact.observed', payload: { path: 'dist/app.js' }, timestamp: 300
    }).events[0]).toMatchObject({ confidence: 'high', sourceRef: { source: 'terminal-marker' } })

    expect(adapter.normalizeParsedText('session-1', '✓ tests passed', 301).events[0]).toMatchObject({
      kind: 'validation.status-changed', confidence: 'low', sourceRef: { source: 'terminal-parse' }
    })
  })
})
