// @vitest-environment jsdom
import { createElement, Fragment, useCallback } from 'react'
import { act, cleanup, render, renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { PROTOCOL_VERSION, type EgressStatusWire } from '@matou/contracts'
import { RuntimeClient, type RuntimeClientPort } from '../runtime/RuntimeClient'
import { chipCopy } from './egress-copy'
import { findEgressFamily, sessionEgressFamily, useEgressSelector, useEgressStatus } from './use-egress-status'

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

class MockPort implements RuntimeClientPort {
  readonly sent: Array<{ type?: string; method?: string; requestId?: string }> = []
  onmessage: ((event: MessageEvent) => void) | null = null
  postMessage(message: unknown): void {
    this.sent.push(message as { type?: string; method?: string; requestId?: string })
  }
  start(): void {}
  close(): void {}
  deliver(data: unknown): void {
    this.onmessage?.({ data } as MessageEvent)
  }
}

describe('useEgressStatus', () => {
  it('shares a single egress.status RPC across 20 mounted components and syncs cached snapshots without flashing undefined', async () => {
    const port = new MockPort()
    const client = new RuntimeClient(port, { clientId: 'renderer-test' })
    port.deliver({
      type: 'protocol.ready',
      protocolVersion: PROTOCOL_VERSION,
      runtimeId: 'runtime-1',
      capabilities: ['domain-rpc-v1']
    })

    const observed: Array<EgressStatusWire | undefined> = []
    function Consumer({ index }: { index: number }) {
      const { status } = useEgressStatus(client)
      observed[index] = status
      return null
    }

    render(
      createElement(
        Fragment,
        null,
        Array.from({ length: 20 }, (_, i) =>
          createElement(Consumer, { key: i, index: i })
        )
      )
    )

    await waitFor(() => {
      const rpcs = port.sent.filter(
        (m) => m.type === 'rpc.request' && m.method === 'egress.status'
      )
      expect(rpcs).toHaveLength(1)
    })

    const rpc = port.sent.find(
      (m) => m.type === 'rpc.request' && m.method === 'egress.status'
    )!
    const initial = makeStatus({ checkedAt: 1_700_000_000_000 })
    act(() => {
      port.deliver({
        type: 'rpc.response',
        protocolVersion: PROTOCOL_VERSION,
        requestId: rpc.requestId,
        ok: true,
        result: initial
      })
    })

    await waitFor(() => {
      expect(observed.every((s) => s === initial)).toBe(true)
    })

    // Newly mounted 21st component receives the cached status synchronously on its very first render.
    const lateRenders: Array<EgressStatusWire | undefined> = []
    function LateConsumer() {
      const { status } = useEgressStatus(client)
      lateRenders.push(status)
      return null
    }
    render(createElement(LateConsumer))
    expect(lateRenders[0]).toEqual(initial)
    expect(
      port.sent.filter((m) => m.type === 'rpc.request' && m.method === 'egress.status')
    ).toHaveLength(1)

    // Pushed updates reach all subscribers without extra RPCs.
    const pushed = makeStatus({ checkedAt: 1_700_000_060_000 })
    act(() => {
      port.deliver({
        type: 'egress.status',
        protocolVersion: PROTOCOL_VERSION,
        status: pushed
      })
    })
    expect(observed[0]).toEqual(pushed)
    expect(observed[19]).toEqual(pushed)

    // Reconnect clears cache and triggers 1 coalesced re-fetch for mounted subscribers.
    const port2 = new MockPort()
    act(() => {
      client.replacePort(port2)
    })
    expect(client.getEgressStatusSnapshot()).toBeUndefined()
    act(() => {
      port2.deliver({
        type: 'protocol.ready',
        protocolVersion: PROTOCOL_VERSION,
        runtimeId: 'runtime-2',
        capabilities: ['domain-rpc-v1']
      })
    })
    await waitFor(() => {
      expect(
        port2.sent.filter((m) => m.type === 'rpc.request' && m.method === 'egress.status')
      ).toHaveLength(1)
    })

    client.dispose()
  })

  it('returns undefined without throwing when egress.status RPC fails', async () => {
    const request = vi.fn().mockRejectedValue(new Error('Method not registered'))
    const mockClient = {
      request,
      subscribeEgressStatus: vi.fn(() => () => {})
    } as unknown as RuntimeClient

    const { result } = renderHook(() => useEgressStatus(mockClient))

    await waitFor(() => {
      expect(request).toHaveBeenCalledWith('egress.status', undefined)
    })
    expect(result.current.status).toBeUndefined()
  })

  it('recomputes selection immediately when selector changes for the same status object reference', () => {
    const port = new MockPort()
    const client = new RuntimeClient(port, { clientId: 'renderer-test' })
    const status = makeStatus({ sessionFamilies: { 'session-claude': 'anthropic' } })
    client.updateEgressStatus(status)

    const { result, rerender } = renderHook(
      ({ sessionId }: { sessionId: string }) => {
        const selector = useCallback(
          (current: EgressStatusWire | undefined) => {
            if (!current) return null
            const family = sessionEgressFamily(current, sessionId)
            if (!family) return null
            return chipCopy(
              current.state,
              family,
              findEgressFamily(current, family),
              current.checks,
              current.checkedAt,
              true
            )
          },
          [sessionId]
        )
        return useEgressSelector(selector, Object.is, client)
      },
      { initialProps: { sessionId: 'session-claude' } }
    )

    expect(result.current?.label).toBe('🇺🇸 美国 · 出口正常')

    rerender({ sessionId: 'session-shell' })
    expect(result.current).toBeNull()

    rerender({ sessionId: 'session-claude' })
    expect(result.current?.label).toBe('🇺🇸 美国 · 出口正常')

    client.dispose()
  })
})
