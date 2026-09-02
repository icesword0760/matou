import { mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { SessionHudRegistry } from './session-hud-registry'

describe('PRD 02 authoritative Session HUD state', () => {
  it('publishes an already validated restored identity as immediately forkable', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({
      sessionId: 'restored-agent', profile: 'claude-code',
      cwd: '/tmp/project', resumable: true
    })

    expect(registry.snapshot('restored-agent')).toMatchObject({
      mode: 'agent', resumable: true
    })
  })

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

  it('parses Claude statusline fields without exposing Fork before durable identity confirmation', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp/project', startedAt: 1 })
    registry.ingestProvider('agent-1', {
      session_id: 'provider-1', cwd: '/tmp/new-project',
      model: { display_name: 'Claude Sonnet 4.6' },
      context_window: { used_percentage: 108, context_window_size: 1_000_000 }
    })

    expect(registry.snapshot('agent-1')).toMatchObject({
      mode: 'agent', cwd: '/tmp/new-project', model: 'Claude Sonnet 4.6',
      modelStrategy: 'opusplan', contextPercent: 108, contextWindowSize: 1_000_000, resumable: false
    })

    registry.markResumable('agent-1')
    expect(registry.snapshot('agent-1')).toMatchObject({ resumable: true })
  })

  it('keeps the provider auto permission mode instead of displaying the default mode', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp/project', startedAt: 1 })
    registry.ingestProvider('agent-1', { permission_mode: 'auto' })

    expect(registry.snapshot('agent-1')).toMatchObject({ permissionMode: 'auto' })
  })

  it('keeps a selected permission visible while a stale provider status arrives', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({
      sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp/project', permissionMode: 'auto'
    })

    registry.updatePermission('agent-1', 'bypassPermissions')
    registry.ingestProvider('agent-1', { permission_mode: 'auto' })
    expect(registry.snapshot('agent-1')).toMatchObject({ permissionMode: 'bypassPermissions' })

    registry.ingestProvider('agent-1', { permission_mode: 'bypassPermissions' })
    registry.ingestProvider('agent-1', { permission_mode: 'plan' })
    expect(registry.snapshot('agent-1')).toMatchObject({ permissionMode: 'plan' })
  })

  it('hydrates restored Agent HUD history from its transcript', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-hud-transcript-'))
    const transcriptPath = join(root, 'provider-session.jsonl')
    await writeFile(transcriptPath, [
      { type: 'user', timestamp: '2026-09-02T10:00:00.000Z', slug: 'nested-squishing-map', permissionMode: 'auto', message: { role: 'user', content: '检查项目' } },
      { type: 'assistant', timestamp: '2026-09-02T10:01:00.000Z', message: { role: 'assistant', model: 'claude-fable-5', content: [{ type: 'tool_use', id: 'read-1', name: 'Read', input: { file_path: '/tmp/project/README.md' } }] } },
      { type: 'user', timestamp: '2026-09-02T10:01:01.000Z', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'read-1', content: 'ok' }] } },
      { type: 'assistant', timestamp: '2026-09-02T10:02:00.000Z', message: { role: 'assistant', model: 'claude-fable-5', content: [{ type: 'tool_use', id: 'todo-1', name: 'TodoWrite', input: { todos: [{ content: '补齐 HUD', status: 'in_progress' }, { content: '回归', status: 'pending' }] } }] } },
      { type: 'user', timestamp: '2026-09-02T10:02:01.000Z', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'todo-1', content: 'ok' }] } },
      { type: 'assistant', timestamp: '2026-09-02T10:03:00.000Z', message: { role: 'assistant', model: 'claude-fable-5', content: [{ type: 'tool_use', id: 'mcp-1', name: 'mcp__browser_bridge__open', input: {} }] } },
      { type: 'user', timestamp: '2026-09-02T10:03:01.000Z', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: 'mcp-1', is_error: true, content: 'failed' }] } }
    ].map((row) => JSON.stringify(row)).join('\n'))
    const registry = new SessionHudRegistry(() => Date.parse('2026-09-02T12:00:00.000Z'))
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp/project' })
    const refreshTranscript = (registry as unknown as {
      refreshTranscript?(sessionId: string, transcriptPath: string): Promise<boolean>
    }).refreshTranscript

    expect(typeof refreshTranscript).toBe('function')
    if (!refreshTranscript) return
    await refreshTranscript.call(registry, 'agent-1', transcriptPath)
    expect(registry.snapshot('agent-1')).toMatchObject({
      sessionName: 'nested-squishing-map', permissionMode: 'auto', model: 'claude-fable-5',
      startedAt: Date.parse('2026-09-02T10:00:00.000Z'),
      runningTools: [],
      toolCounts: [
        { name: 'Read', count: 1 },
        { name: 'TodoWrite', count: 1 },
        { name: 'mcp__browser_bridge__open', count: 1 }
      ],
      lastTool: { name: 'mcp__browser_bridge__open', status: 'error' },
      mcpErrors: ['browser_bridge'],
      todos: [
        { content: '补齐 HUD', status: 'in_progress' },
        { content: '回归', status: 'pending' }
      ]
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

  it('projects usage, completed tool counts and MCP failures into the HUD', () => {
    const registry = new SessionHudRegistry(() => 20_000)
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp', startedAt: 1 })
    registry.ingestProvider('agent-1', {
      cost: { total_duration_ms: 3_600_000 },
      rate_limits: {
        five_hour: { used_percentage: 24, resets_at: 1_800 },
        seven_day: { used_percentage: 8, resets_at: 90_000 },
        model_scoped: [{ display_name: 'Opus', utilization: 12, resets_at: '1970-01-02T00:00:00Z' }]
      }
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PreToolUse', tool_name: 'Read', tool_use_id: 'read-1',
      tool_input: { file_path: '/tmp/a.ts' }
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PostToolUse', tool_name: 'Read', tool_use_id: 'read-1'
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PreToolUse', tool_name: 'mcp__browser_bridge__open', tool_use_id: 'mcp-1'
    })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PostToolUseFailure', tool_name: 'mcp__browser_bridge__open', tool_use_id: 'mcp-1'
    })

    expect(registry.snapshot('agent-1')).toMatchObject({
      startedAt: -3_580_000,
      usageWindows: [
        { label: '5h', percent: 24, resetsAt: 1_800_000 },
        { label: 'Weekly', percent: 8, resetsAt: 90_000_000 },
        { label: 'Opus', percent: 12, resetsAt: 86_400_000 }
      ],
      toolCounts: [
        { name: 'Read', count: 1 },
        { name: 'mcp__browser_bridge__open', count: 1 }
      ],
      mcpErrors: ['browser_bridge'],
      lastTool: { name: 'mcp__browser_bridge__open', status: 'error' }
    })
  })

  it('updates the provider-generated session name shown in the HUD', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp', startedAt: 1 })
    expect(typeof (registry as unknown as { updateSessionName?: unknown }).updateSessionName).toBe('function')
    ;(registry as unknown as { updateSessionName(sessionId: string, name: string): void })
      .updateSessionName('agent-1', 'adaptive-painting-hoare')
    expect(registry.snapshot('agent-1')).toMatchObject({ sessionName: 'adaptive-painting-hoare' })
  })

  it('counts user and project instructions, MCP servers and hooks for the HUD', async () => {
    const module = await import('./session-hud-registry')
    expect(typeof (module as Record<string, unknown>).inspectProviderConfig).toBe('function')
    const root = await mkdtemp(join(tmpdir(), 'matou-hud-config-'))
    const configDir = join(root, 'config')
    const cwd = join(root, 'project')
    await mkdir(join(cwd, '.claude'), { recursive: true })
    await mkdir(configDir, { recursive: true })
    await writeFile(join(configDir, 'CLAUDE.md'), 'user instructions')
    await writeFile(join(configDir, 'settings.json'), JSON.stringify({
      mcpServers: { user_bridge: {} }, hooks: { Stop: [], Notification: [] }
    }))
    await writeFile(join(cwd, 'CLAUDE.local.md'), 'local instructions')
    await writeFile(join(cwd, '.claude', 'CLAUDE.md'), 'project instructions')
    await writeFile(join(cwd, '.mcp.json'), JSON.stringify({
      mcpServers: { project_bridge: {}, disabled_bridge: {} }
    }))
    await writeFile(join(cwd, '.claude', 'settings.local.json'), JSON.stringify({
      mcpServers: { local_bridge: {} }, disabledMcpjsonServers: ['disabled_bridge'],
      hooks: { PreToolUse: [] }
    }))

    const inspectProviderConfig = (module as unknown as {
      inspectProviderConfig(cwd: string, configDir: string): {
        instructionFiles: number; mcpServers: number; hooks: number
      }
    }).inspectProviderConfig
    expect(inspectProviderConfig(cwd, configDir)).toEqual({
      instructionFiles: 3, mcpServers: 3, hooks: 3
    })
  })

  it('shows an explicit provider permission prompt as waiting for user input', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'agent-1', profile: 'claude-code', cwd: '/tmp', startedAt: 1 })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PermissionRequest', tool_name: 'Write',
      tool_input: { file_path: '/tmp/file.ts' }
    })
    expect(registry.snapshot('agent-1')).toMatchObject({ taskStatus: 'needs-input' })
    registry.ingestProvider('agent-1', {
      hook_event_name: 'PreToolUse', tool_name: 'Write', tool_use_id: 'write-1'
    })
    expect(registry.snapshot('agent-1')).toMatchObject({ taskStatus: 'running' })
  })

  it('re-reads provider configuration while an agent session is otherwise idle', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-hud-live-config-'))
    const cwd = join(root, 'project')
    const configDir = join(root, '.claude')
    await mkdir(cwd, { recursive: true })
    await mkdir(configDir, { recursive: true })
    const registry = new SessionHudRegistry(Date.now, configDir)
    registry.spawn({ sessionId: 'agent-live', profile: 'claude-code', cwd })
    expect(registry.snapshot('agent-live')?.configCounts).toEqual({
      instructionFiles: 0, mcpServers: 0, hooks: 0
    })

    await writeFile(join(configDir, 'settings.json'), JSON.stringify({
      mcpServers: { live_bridge: {} }, hooks: { Stop: [] }
    }))

    expect(registry.refreshConfig('agent-live')).toBe(true)
    expect(registry.snapshot('agent-live')?.configCounts).toEqual({
      instructionFiles: 0, mcpServers: 1, hooks: 1
    })
    expect(registry.configWatchTargets('agent-live')).toEqual(expect.arrayContaining([
      expect.objectContaining({ directory: configDir, names: expect.arrayContaining(['settings.json']) }),
      expect.objectContaining({ directory: cwd, names: expect.arrayContaining(['.mcp.json']) })
    ]))
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
