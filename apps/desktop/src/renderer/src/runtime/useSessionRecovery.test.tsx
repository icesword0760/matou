// @vitest-environment jsdom
import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { PROTOCOL_VERSION } from '@matou/contracts'

import type { SessionRecoveryStatus } from './RuntimeClient'
import { useSessionRecovery } from './useSessionRecovery'

afterEach(cleanup)

describe('useSessionRecovery', () => {
  it('projects authoritative per-card state and reprioritizes the current foreground list', () => {
    let listener: ((status: SessionRecoveryStatus) => void) | undefined
    const client = {
      subscribeSessionRecovery: vi.fn((next: (status: SessionRecoveryStatus) => void) => {
        listener = next
        return () => { listener = undefined }
      }),
      prioritizeSessionRecovery: vi.fn(),
      retrySessionRecovery: vi.fn()
    }
    const view = renderHook(
      ({ sceneId, sessionId }) => useSessionRecovery(client, sceneId, sessionId),
      { initialProps: { sceneId: 'scene-1', sessionId: 'session-1' } }
    )

    expect(client.prioritizeSessionRecovery).toHaveBeenCalledWith('scene-1', 'session-1')
    act(() => listener?.({
      type: 'session.recovery-status', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'session-2', sceneId: 'scene-1', priority: 'foreground-scene',
      state: 'restoring'
    }))
    expect(view.result.current.statusBySession.get('session-2')?.state).toBe('restoring')

    view.rerender({ sceneId: 'scene-2', sessionId: 'session-3' })
    expect(client.prioritizeSessionRecovery).toHaveBeenLastCalledWith('scene-2', 'session-3')
    view.result.current.retry('session-2')
    expect(client.retrySessionRecovery).toHaveBeenCalledWith('session-2')
  })
})
