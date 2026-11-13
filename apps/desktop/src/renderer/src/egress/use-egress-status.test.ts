// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { EgressStatusWire } from '@matou/contracts'
import type { RuntimeClient } from '../runtime/RuntimeClient'
import { useEgressStatus } from './use-egress-status'

afterEach(cleanup)

function makeStatus(overrides: Partial<EgressStatusWire> = {}): EgressStatusWire {
  return {
    state: 'ok',
    protectionEnabled: true,
    ruleMode: 'country',
    checkedAt: 1_700_000_000_000,
    families: [
      {
        family: 'anthropic',
        state: 'ok',
        exit: { ip: '169.128.10.5', country: 'US' },
        hosts: [],
        overridden: false,
        frozenSessionIds: [],
        recovering: false
      }
    ],
    checks: [],
    sessionFamilies: { 'session-1': 'anthropic' },
    ...overrides
  }
}

describe('useEgressStatus', () => {
  it('fetches initial status, follows pushes, and re-fetches on reconnect', async () => {
    const initial = makeStatus({ checkedAt: 1_700_000_000_000 })
    const pushed = makeStatus({
      checkedAt: 1_700_000_060_000,
      families: [
        {
          family: 'anthropic',
          state: 'unknown',
          hosts: [],
          overridden: false,
          frozenSessionIds: [],
          recovering: false
        }
      ]
    })
    const reconnected = makeStatus({ checkedAt: 1_700_000_120_000 })

    let pushListener: ((status: EgressStatusWire) => void) | undefined
    let reconnectListener: (() => void) | undefined

    const request = vi
      .fn()
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(reconnected)

    const mockClient = {
      request,
      subscribeEgressStatus: vi.fn(
        (listener: (status: EgressStatusWire) => void, onReconnect?: () => void) => {
          pushListener = listener
          reconnectListener = onReconnect
          return () => {
            pushListener = undefined
            reconnectListener = undefined
          }
        }
      )
    } as unknown as RuntimeClient

    const { result } = renderHook(() => useEgressStatus(mockClient))

    await waitFor(() => {
      expect(result.current.status).toEqual(initial)
    })
    expect(request).toHaveBeenCalledWith('egress.status', undefined)

    act(() => {
      pushListener?.(pushed)
    })
    expect(result.current.status).toEqual(pushed)

    act(() => {
      reconnectListener?.()
    })
    await waitFor(() => {
      expect(result.current.status).toEqual(reconnected)
    })
    expect(request).toHaveBeenCalledTimes(2)
  })

  it('returns undefined without throwing when egress.status RPC fails', async () => {
    const mockClient = {
      request: vi.fn().mockRejectedValue(new Error('Method not registered')),
      subscribeEgressStatus: vi.fn(() => () => {})
    } as unknown as RuntimeClient

    const { result } = renderHook(() => useEgressStatus(mockClient))

    await waitFor(() => {
      expect(mockClient.request).toHaveBeenCalledWith('egress.status', undefined)
    })
    expect(result.current.status).toBeUndefined()
  })
})
