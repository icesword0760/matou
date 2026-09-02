import { describe, expect, it, vi } from 'vitest'

import { createHierarchyCommands } from './hierarchy-commands'

describe('PRD 02 HUD commands', () => {
  it('replays an idempotent Workspace creation once when the first Runtime response is lost', async () => {
    const request = vi.fn()
      .mockRejectedValueOnce(new Error('Runtime channel replaced before the request completed'))
      .mockResolvedValueOnce({ workspace: { id: 'workspace-new' } })
    const afterMutation = vi.fn()
    const commands = createHierarchyCommands({ request } as never, 'window-1', afterMutation)

    await commands.createWorkspace('/Users/demo/new-workspace')

    expect(request).toHaveBeenCalledTimes(2)
    expect(request.mock.calls[1]?.[0]).toBe('hierarchy.create-workspace')
    expect(request.mock.calls[1]?.[1]).toEqual(request.mock.calls[0]?.[1])
    expect(afterMutation).toHaveBeenCalledTimes(1)
  })

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

  it('routes manual rename and Claude title restore through authoritative session commands', async () => {
    const request = vi.fn().mockResolvedValue({})
    const commands = createHierarchyCommands({ request } as never, 'window-1')

    await commands.renameSession?.('session-1', '发布问题排查')
    await commands.restoreSessionAutoTitle?.('session-1')

    expect(request.mock.calls.map(([method, payload]) => [method, payload.input])).toEqual([
      ['hierarchy.rename-session', expect.objectContaining({
        sessionId: 'session-1', title: '发布问题排查'
      })],
      ['hierarchy.restore-session-auto-title', expect.objectContaining({
        sessionId: 'session-1'
      })]
    ])
  })
})
