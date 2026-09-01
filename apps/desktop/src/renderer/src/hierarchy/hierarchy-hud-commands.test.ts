import { describe, expect, it, vi } from 'vitest'

import { createHierarchyCommands } from './hierarchy-commands'

describe('PRD 02 HUD commands', () => {
  it('sends permission and model changes through the authoritative Runtime', async () => {
    const request = vi.fn().mockResolvedValue({})
    const commands = createHierarchyCommands({ request } as never, 'window-1')

    await commands.setPermissionMode('session-1', 'plan', false)
    await commands.setPermissionMode('session-1', 'bypassPermissions', true)
    await commands.setModel('session-1', 'claude-sonnet-4-6')

    expect(request.mock.calls.map(([method, payload]) => [method, payload.input])).toEqual([
      ['session.set-permission-mode', expect.objectContaining({
        sessionId: 'session-1', provider: 'claude-code', permissionMode: 'plan', respawn: false
      })],
      ['session.set-permission-mode', expect.objectContaining({
        sessionId: 'session-1', provider: 'claude-code', permissionMode: 'bypassPermissions', respawn: true
      })],
      ['session.set-model', expect.objectContaining({
        sessionId: 'session-1', modelStrategy: 'claude-sonnet-4-6'
      })]
    ])
  })

  it('keeps the owning canvas when retrying or removing a failed Fork card', async () => {
    const request = vi.fn().mockResolvedValue({})
    const commands = createHierarchyCommands({ request } as never, 'window-1')

    await commands.retryFork('scene-1', 'failed-session')
    await commands.removeFailedFork('scene-1', 'failed-session')

    expect(request.mock.calls.map(([method, payload]) => [method, payload.input])).toEqual([
      ['hierarchy.retry-fork', expect.objectContaining({
        sceneId: 'scene-1', sessionId: 'failed-session'
      })],
      ['hierarchy.remove-failed-fork', expect.objectContaining({
        sceneId: 'scene-1', sessionId: 'failed-session'
      })]
    ])
  })

  it('reasserts focus on a newly created Shell after its authoritative mutation', async () => {
    const request = vi.fn()
      .mockResolvedValueOnce({ session: { id: 'new-shell' } })
      .mockResolvedValueOnce({})
    const commands = createHierarchyCommands({ request } as never, 'window-1')

    await commands.createShellSibling('scene-1', 'source-shell')

    expect(request.mock.calls.map(([method, payload]) => [method, payload.input])).toEqual([
      ['hierarchy.create-shell-sibling', expect.objectContaining({
        sceneId: 'scene-1', sourceSessionId: 'source-shell'
      })],
      ['hierarchy.set-focused-session', expect.objectContaining({
        sceneId: 'scene-1', sessionId: 'new-shell'
      })]
    ])
  })

  it('keeps environment operations authoritative and activates an already-owned Worktree Session', async () => {
    const request = vi.fn()
      .mockResolvedValueOnce({ sessionId: 'session-1', kind: 'local', path: '/tmp/local' })
      .mockResolvedValueOnce({ kind: 'switch-session', sessionId: 'owner-session' })
      .mockResolvedValueOnce({})
      .mockResolvedValueOnce({
        kind: 'environment', sessionId: 'session-1', activeTarget: 'local',
        state: 'ready', path: '/tmp/local', restartRequired: true
      })
    const afterMutation = vi.fn()
    const commands = createHierarchyCommands(
      { request } as never, 'window-1', afterMutation
    )

    await expect(commands.openSessionEnvironment('session-1')).resolves.toEqual({
      sessionId: 'session-1', kind: 'local', path: '/tmp/local'
    })
    await expect(commands.locateSessionEnvironment('session-1', '/tmp/owned')).resolves.toEqual({
      kind: 'switch-session', sessionId: 'owner-session'
    })
    await commands.handoffSessionEnvironment('session-1', 'local')

    expect(request.mock.calls.map(([method, payload]) => [method, payload.input ?? payload])).toEqual([
      ['session.environment-open', { sessionId: 'session-1' }],
      ['session.environment-locate', expect.objectContaining({
        sessionId: 'session-1', path: '/tmp/owned'
      })],
      ['hierarchy.activate-session', expect.objectContaining({ sessionId: 'owner-session' })],
      ['session.environment-handoff', expect.objectContaining({
        sessionId: 'session-1', target: 'local'
      })]
    ])
    expect(afterMutation).toHaveBeenCalledTimes(3)
  })
})
