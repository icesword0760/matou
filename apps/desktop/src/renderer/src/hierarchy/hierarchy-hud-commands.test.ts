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
})
