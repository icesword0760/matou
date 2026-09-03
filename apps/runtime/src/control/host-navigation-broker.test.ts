import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { HostNavigationPath } from '@matou/contracts'

import {
  HostNavigationBroker,
  type HostNavigationRequestInput,
  type HostNavigationSender
} from './host-navigation-broker'

let broker: HostNavigationBroker

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(1_000)
  broker = new HostNavigationBroker()
})

afterEach(() => {
  broker.close()
  vi.useRealTimers()
})

describe('HostNavigationBroker', () => {
  it('routes one navigation request to the target window and resolves its bound acknowledgement', async () => {
    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)

    const pending = broker.navigate(navigationInput())

    expect(client.sent).toEqual([expect.objectContaining({
      type: 'host.navigation-request',
      protocolVersion: 1,
      requestId: 'nav-1',
      windowId: 'main-window-2',
      focusTerminal: true,
      deadlineAt: 5_000
    })])
    expect(broker.acknowledge({
      type: 'host.navigation-result',
      protocolVersion: 1,
      requestId: 'nav-1',
      windowId: 'main-window-2',
      ok: true,
      finalPath: pathFixture()
    }, registration)).toBe(true)

    await expect(pending).resolves.toEqual({ finalPath: pathFixture() })
  })

  it('binds equal request IDs to their window and exact sender generation', async () => {
    const first = fakeClient()
    const second = fakeClient()
    const firstRegistration = broker.registerWindow('main-window-1', first.send)
    const secondRegistration = broker.registerWindow('main-window-2', second.send)
    const firstPending = broker.navigate(navigationInput({
      requestId: 'same-request', windowId: 'main-window-1'
    }))
    const secondPending = broker.navigate(navigationInput({ requestId: 'same-request' }))

    expect(broker.acknowledge(successResult({
      requestId: 'same-request', windowId: 'main-window-1',
      finalPath: pathFixture({ windowId: 'main-window-1' })
    }), secondRegistration)).toBe(false)
    expect(broker.acknowledge(successResult({
      requestId: 'same-request', windowId: 'main-window-1',
      finalPath: pathFixture({ windowId: 'main-window-1' })
    }), firstRegistration)).toBe(true)
    await expect(firstPending).resolves.toEqual({
      finalPath: pathFixture({ windowId: 'main-window-1' })
    })

    expect(broker.acknowledge(successResult({ requestId: 'same-request' }), secondRegistration))
      .toBe(true)
    await expect(secondPending).resolves.toEqual({ finalPath: pathFixture() })
    expect(broker.acknowledge(successResult({ requestId: 'same-request' }), secondRegistration))
      .toBe(false)
  })

  it('keeps a replacement sender registered when the old sender later closes', async () => {
    const first = fakeClient()
    const second = fakeClient()
    const firstRegistration = broker.registerWindow('main-window-2', first.send)
    const displaced = broker.navigate(navigationInput({ requestId: 'displaced' }))
    const displacedResult = expect(displaced).rejects.toMatchObject({ code: 'TARGET_NOT_READY' })

    const secondRegistration = broker.registerWindow('main-window-2', second.send)
    await displacedResult
    expect(broker.unregisterWindow('main-window-2', firstRegistration)).toBe(false)

    const current = broker.navigate(navigationInput({ requestId: 'current' }))
    expect(first.sent).toHaveLength(1)
    expect(second.sent).toEqual([expect.objectContaining({ requestId: 'current' })])
    expect(broker.acknowledge(successResult({ requestId: 'current' }), firstRegistration)).toBe(false)
    expect(broker.acknowledge(successResult({ requestId: 'current' }), secondRegistration)).toBe(true)
    await expect(current).resolves.toEqual({ finalPath: pathFixture() })
  })

  it('rejects an offline or disconnected window and clears its pending request', async () => {
    await expect(broker.navigate(navigationInput({ requestId: 'offline' })))
      .rejects.toMatchObject({ code: 'TARGET_NOT_READY' })

    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const pending = broker.navigate(navigationInput({ requestId: 'disconnect' }))
    const disconnected = expect(pending).rejects.toMatchObject({ code: 'TARGET_NOT_READY' })
    expect(broker.unregisterWindow('main-window-2', registration)).toBe(true)
    await disconnected

    const replacement = fakeClient()
    const replacementRegistration = broker.registerWindow('main-window-2', replacement.send)
    const reused = broker.navigate(navigationInput({ requestId: 'disconnect' }))
    expect(broker.acknowledge(successResult({ requestId: 'disconnect' }), replacementRegistration))
      .toBe(true)
    await expect(reused).resolves.toEqual({ finalPath: pathFixture() })
  })

  it('uses the absolute deadline, ignores late acknowledgements, and releases request identity', async () => {
    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const expired = broker.navigate(navigationInput({
      requestId: 'already-expired', deadlineAt: 999
    }))
    await expect(expired).rejects.toMatchObject({ code: 'NAVIGATION_TIMEOUT' })
    expect(client.sent).toEqual([])

    const pending = broker.navigate(navigationInput({ requestId: 'timed', deadlineAt: 1_200 }))
    const timedOut = expect(pending).rejects.toMatchObject({ code: 'NAVIGATION_TIMEOUT' })
    await vi.advanceTimersByTimeAsync(200)
    await timedOut
    expect(broker.acknowledge(successResult({ requestId: 'timed' }), registration)).toBe(false)

    const reused = broker.navigate(navigationInput({ requestId: 'timed', deadlineAt: 2_000 }))
    expect(broker.acknowledge(successResult({ requestId: 'timed' }), registration)).toBe(true)
    await expect(reused).resolves.toEqual({ finalPath: pathFixture() })
  })

  it('clears a request after send failure and maps Renderer refusal to a retryable target state', async () => {
    const brokenSend: HostNavigationSender = () => { throw new Error('port closed') }
    broker.registerWindow('main-window-2', brokenSend)
    await expect(broker.navigate(navigationInput({ requestId: 'send-failed' })))
      .rejects.toMatchObject({ code: 'TARGET_NOT_READY', message: expect.stringContaining('port closed') })

    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const refused = broker.navigate(navigationInput({ requestId: 'send-failed' }))
    expect(broker.acknowledge({
      type: 'host.navigation-result', protocolVersion: 1,
      requestId: 'send-failed', windowId: 'main-window-2', ok: false,
      error: 'target disappeared'
    }, registration)).toBe(true)
    await expect(refused).rejects.toMatchObject({
      code: 'TARGET_NOT_READY', message: 'target disappeared'
    })
  })

  it('settles every pending request when the Runtime navigation bridge closes', async () => {
    const first = fakeClient()
    const second = fakeClient()
    broker.registerWindow('main-window-1', first.send)
    broker.registerWindow('main-window-2', second.send)
    const one = broker.navigate(navigationInput({ requestId: 'shutdown-1', windowId: 'main-window-1' }))
    const two = broker.navigate(navigationInput({ requestId: 'shutdown-2' }))
    const oneResult = expect(one).rejects.toMatchObject({ code: 'TARGET_NOT_READY' })
    const twoResult = expect(two).rejects.toMatchObject({ code: 'TARGET_NOT_READY' })

    broker.close()

    await oneResult
    await twoResult
    await expect(broker.navigate(navigationInput({ requestId: 'after-close' })))
      .rejects.toMatchObject({ code: 'TARGET_NOT_READY' })
  })
})

function fakeClient(): { sent: Parameters<HostNavigationSender>[0][]; send: HostNavigationSender } {
  const sent: Parameters<HostNavigationSender>[0][] = []
  return { sent, send: (message) => { sent.push(message) } }
}

function navigationInput(
  overrides: Partial<HostNavigationRequestInput> = {}
): HostNavigationRequestInput {
  return {
    requestId: 'nav-1',
    windowId: 'main-window-2',
    workspaceId: 'workspace-2',
    taskId: 'task-2',
    sceneId: 'scene-2',
    sessionId: 'session-2',
    focusTerminal: true,
    deadlineAt: 5_000,
    ...overrides
  }
}

function pathFixture(overrides: Partial<HostNavigationPath> = {}): HostNavigationPath {
  return {
    windowId: 'main-window-2',
    workspaceId: 'workspace-2',
    taskId: 'task-2',
    sceneId: 'scene-2',
    sessionId: 'session-2',
    ...overrides
  }
}

function successResult(overrides: Partial<{
  requestId: string
  windowId: string
  finalPath: HostNavigationPath
}> = {}) {
  return {
    type: 'host.navigation-result' as const,
    protocolVersion: 1 as const,
    requestId: 'nav-1',
    windowId: 'main-window-2',
    ok: true as const,
    finalPath: pathFixture(),
    ...overrides
  }
}
