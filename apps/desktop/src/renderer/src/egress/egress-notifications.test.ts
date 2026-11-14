// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { EgressStatusWire } from '@matou/contracts'
import { AgentNotificationStore } from '../notifications/AgentNotificationStore'
import type { RuntimeClient } from '../runtime/RuntimeClient'
import { egressNotification, useEgressNotifications } from './egress-notifications'

let mockClient: RuntimeClient | null = null

vi.mock('../runtime/RuntimeProvider', () => ({
  useRuntimeClient: () => mockClient
}))

afterEach(() => {
  cleanup()
  mockClient = null
})

function makeStatus(
  lastTransition?: EgressStatusWire['lastTransition']
): EgressStatusWire {
  return {
    state: lastTransition?.kind === 'frozen' ? 'abnormal' : 'ok',
    protectionEnabled: true,
    ruleMode: 'country',
    checkedAt: 1_700_000_000_000,
    families: [],
    checks: [],
    sessionFamilies: {},
    ...(lastTransition ? { lastTransition } : {})
  }
}

describe('egressNotification', () => {
  it('ignores historical transitions on initial status and dedupes by transition id', () => {
    const initial = makeStatus({
      id: 't-old',
      kind: 'frozen',
      family: 'anthropic',
      sessionCount: 2,
      at: 1_700_000_000_000,
      reason: 'mainland'
    })

    expect(egressNotification(undefined, initial, true)).toBeUndefined()
    expect(egressNotification(initial, initial, true)).toBeUndefined()
  })

  it('produces exact frozen (mainland & rule) and resumed notifications in zh and en', () => {
    const base = makeStatus()

    const frozenMainland = makeStatus({
      id: 't-1',
      kind: 'frozen',
      family: 'anthropic',
      sessionCount: 3,
      at: 1_700_000_010_000,
      reason: 'mainland'
    })
    expect(egressNotification(base, frozenMainland, true)).toEqual({
      eventId: 'egress:t-1',
      eventType: 'attention',
      replacementKey: 'egress',
      title: '出口异常，已暂停 3 个会话',
      body: '检测到中国大陆 IP。网络恢复后自动继续。'
    })
    expect(egressNotification(base, frozenMainland, false)).toEqual({
      eventId: 'egress:t-1',
      eventType: 'attention',
      replacementKey: 'egress',
      title: 'Exit problem: 3 session(s) paused',
      body: 'Mainland China IP detected. Resumes automatically when the network recovers.'
    })

    const frozenRule = makeStatus({
      id: 't-2',
      kind: 'frozen',
      family: 'openai',
      sessionCount: 1,
      at: 1_700_000_020_000,
      reason: 'rule'
    })
    expect(egressNotification(frozenMainland, frozenRule, true)).toEqual({
      eventId: 'egress:t-2',
      eventType: 'attention',
      replacementKey: 'egress',
      title: '出口异常，已暂停 1 个会话',
      body: '出口与规则不符。网络恢复后自动继续。'
    })
    expect(egressNotification(frozenMainland, frozenRule, false)).toEqual({
      eventId: 'egress:t-2',
      eventType: 'attention',
      replacementKey: 'egress',
      title: 'Exit problem: 1 session(s) paused',
      body: "Exit doesn't match your rule. Resumes automatically when the network recovers."
    })

    const resumed = makeStatus({
      id: 't-3',
      kind: 'resumed',
      family: 'openai',
      sessionCount: 1,
      at: 1_700_000_030_000
    })
    expect(egressNotification(frozenRule, resumed, true)).toEqual({
      eventId: 'egress:t-3',
      eventType: 'attention',
      replacementKey: 'egress',
      title: '出口已恢复，1 个会话已继续'
    })
    expect(egressNotification(frozenRule, resumed, false)).toEqual({
      eventId: 'egress:t-3',
      eventType: 'attention',
      replacementKey: 'egress',
      title: 'Exit restored: 1 session(s) resumed'
    })
  })
})

describe('useEgressNotifications', () => {
  it('skips initial historical transition, pushes subsequent transitions once, and replaces prior egress notice', async () => {
    const initialWithOldTransition = makeStatus({
      id: 't-history',
      kind: 'frozen',
      family: 'anthropic',
      sessionCount: 1,
      at: 1_700_000_000_000,
      reason: 'mainland'
    })
    const frozenNew = makeStatus({
      id: 't-freeze-new',
      kind: 'frozen',
      family: 'anthropic',
      sessionCount: 2,
      at: 1_700_000_050_000,
      reason: 'mainland'
    })
    const resumedNew = makeStatus({
      id: 't-resume-new',
      kind: 'resumed',
      family: 'anthropic',
      sessionCount: 2,
      at: 1_700_000_090_000
    })

    let pushStatus: ((s: EgressStatusWire) => void) | undefined
    mockClient = {
      request: vi.fn().mockResolvedValue(initialWithOldTransition),
      subscribeEgressStatus: vi.fn((listener: (s: EgressStatusWire) => void) => {
        pushStatus = listener
        return () => { pushStatus = undefined }
      })
    } as unknown as RuntimeClient

    const store = new AgentNotificationStore()
    renderHook(() => useEgressNotifications(store, true))

    await waitFor(() => {
      expect(mockClient?.request).toHaveBeenCalledWith('egress.status', undefined)
    })
    expect(store.snapshot().notifications).toHaveLength(0)

    act(() => {
      pushStatus?.(frozenNew)
    })
    expect(store.snapshot().notifications).toHaveLength(1)
    expect(store.snapshot().notifications[0]!.title).toBe('出口异常，已暂停 2 个会话')

    // Re-delivering the exact same transition id must not notify again.
    act(() => {
      pushStatus?.({ ...frozenNew, checkedAt: 1_700_000_055_000 })
    })
    expect(store.snapshot().notifications).toHaveLength(1)

    // Subsequent transition notifies and replaces by replacementKey: 'egress'.
    act(() => {
      pushStatus?.(resumedNew)
    })
    expect(store.snapshot().notifications).toHaveLength(1)
    expect(store.snapshot().notifications[0]!.title).toBe('出口已恢复，2 个会话已继续')
  })
})
