import { describe, expect, it } from 'vitest'

import { SessionHudRegistry } from './session-hud-registry'

describe('PRD 02 authoritative Session HUD state', () => {
  it('keeps independent state per Session and demotes an ended Agent to Shell without stale fields', () => {
    const registry = new SessionHudRegistry(() => 20_000)
    registry.spawn({
      sessionId: 'shell-1', profile: 'shell', shell: 'zsh', cwd: '/tmp/one', startedAt: 1_000
    })
    registry.spawn({
      sessionId: 'agent-1', profile: 'claude-code', shell: 'zsh', cwd: '/tmp/two', startedAt: 2_000,
      permissionMode: 'acceptEdits'
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PreToolUse', tool_name: 'Read', tool_use_id: 'tool-1',
      tool_input: { file_path: '/tmp/two/README.md' }
    })

    expect(registry.snapshot('shell-1')).toMatchObject({ mode: 'shell', cwd: '/tmp/one' })
    expect(registry.snapshot('agent-1')).toMatchObject({
      mode: 'agent', permissionMode: 'acceptEdits', taskStatus: 'running',
      runningTools: [{ name: 'Read', target: '/tmp/two/README.md' }]
    })

    registry.exit('agent-1', { fallbackToShell: true })
    expect(registry.snapshot('agent-1')).toMatchObject({
      mode: 'shell', cwd: '/tmp/two', startedAt: 2_000
    })
    expect(registry.snapshot('agent-1')).not.toHaveProperty('contextPercent')
    expect(registry.snapshot('agent-1')).not.toHaveProperty('runningTools')
  })

  it('parses Claude statusline fields while clamping only ring geometry at the UI boundary', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp/project', startedAt: 1 })
    registry.ingestProvider('agent-1', {
      session_id: 'provider-1', cwd: '/tmp/new-project',
      model: { display_name: 'Claude Sonnet 4.6' },
      context_window: { used_percentage: 108 }
    })

    expect(registry.snapshot('agent-1')).toMatchObject({
      mode: 'agent', cwd: '/tmp/new-project', model: 'Claude Sonnet 4.6',
      modelStrategy: 'opusplan', contextPercent: 108, resumable: true
    })
  })

  it('tracks running tools, last-two display data, todos, task state and subagents from hooks', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp', startedAt: 1 })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PreToolUse', tool_name: 'TodoWrite', tool_use_id: 'todo-tool',
      tool_input: { todos: [
        { content: '第一项', status: 'completed' },
        { content: '第二项', status: 'in_progress' }
      ] }
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PreToolUse', tool_name: 'Agent', tool_use_id: 'agent-tool', tool_input: {}
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PreToolUse', tool_name: 'Read', tool_use_id: 'read-tool',
      tool_input: { file_path: '/tmp/a.ts' }
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PostToolUse', tool_name: 'Read', tool_use_id: 'read-tool'
    })
    registry.ingestProvider('agent-1', { hook_event_name: 'Stop' })

    expect(registry.snapshot('agent-1')).toMatchObject({
      taskStatus: 'idle', subagentCount: 1,
      todos: [
        { content: '第一项', status: 'completed' },
        { content: '第二项', status: 'in_progress' }
      ]
    })
    expect(registry.snapshot('agent-1')?.runningTools).toEqual([])
  })

  it('refreshes Git branch and dirty state and removes Git when cwd is outside a repository', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'shell-1', profile: 'shell', cwd: '/tmp', startedAt: 1 })
    registry.updateEnvironment('shell-1', {
      cwd: '/repo', gitBranch: 'feature/hud', gitDirty: true
    })
    expect(registry.snapshot('shell-1')).toMatchObject({
      cwd: '/repo', gitBranch: 'feature/hud', gitDirty: true
    })
    registry.updateEnvironment('shell-1', { cwd: '/tmp' })
    expect(registry.snapshot('shell-1')).not.toHaveProperty('gitBranch')
  })
})
