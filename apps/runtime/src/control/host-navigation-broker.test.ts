import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type {
  HostNavigationPath,
  HostNavigationRequestWire,
  HostNavigationResultWire
} from '@matou/contracts'

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
  it('routes through the owning main window and resolves an exact target acknowledgement', async () => {
    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)

    const pending = broker.navigate(navigationInput())
    const request = onlyRequest(client)

    expect(request).toMatchObject({
      type: 'host.navigation-request',
      protocolVersion: 1,
      requestId: 'nav-1',
      attemptId: expect.any(String),
      routeWindowId: 'main-window-2',
      targetWindowId: 'main-window-2',
      focusTerminal: true,
      deadlineAt: 5_000
    })
    expect(broker.acknowledge(successResult(request), registration)).toBe(true)
    await expect(pending).resolves.toEqual({ finalPath: pathFor(request) })
  })

  it('routes a detached target through its owning main window rather than the native key', async () => {
    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const pending = broker.navigate(navigationInput({
      requestId: 'detached-focus',
      routeWindowId: 'main-window-2',
      targetWindowId: 'native-detached-window'
    }))
    const request = onlyRequest(client)

    expect(request).toMatchObject({
      routeWindowId: 'main-window-2', targetWindowId: 'native-detached-window'
    })
    await expect(broker.navigate(navigationInput({
      requestId: 'native-is-not-route', routeWindowId: 'native-detached-window',
      targetWindowId: 'native-detached-window'
    }))).rejects.toMatchObject({ code: 'TARGET_NOT_READY' })

    expect(broker.acknowledge(successResult(request), registration)).toBe(true)
    await expect(pending).resolves.toEqual({ finalPath: pathFor(request) })
  })

  it('binds equal request IDs to their route window and exact sender generation', async () => {
    const first = fakeClient()
    const second = fakeClient()
    const firstRegistration = broker.registerWindow('main-window-1', first.send)
    const secondRegistration = broker.registerWindow('main-window-2', second.send)
    const firstPending = broker.navigate(navigationInput({
      requestId: 'same-request', routeWindowId: 'main-window-1', targetWindowId: 'main-window-1'
    }))
    const secondPending = broker.navigate(navigationInput({ requestId: 'same-request' }))
    const firstRequest = onlyRequest(first)
    const secondRequest = onlyRequest(second)

    expect(broker.acknowledge(successResult(firstRequest), secondRegistration)).toBe(false)
    expect(broker.acknowledge(successResult(firstRequest), firstRegistration)).toBe(true)
    await expect(firstPending).resolves.toEqual({ finalPath: pathFor(firstRequest) })

    expect(broker.acknowledge(successResult(secondRequest), secondRegistration)).toBe(true)
    await expect(secondPending).resolves.toEqual({ finalPath: pathFor(secondRequest) })
    expect(broker.acknowledge(successResult(secondRequest), secondRegistration)).toBe(false)
  })

  it('rejects and clears a current-sender acknowledgement with a false route or target path', async () => {
    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const invalidResults: Array<[
      string,
      (request: HostNavigationRequestWire) => HostNavigationResultWire
    ]> = [
      ['cross-route', (request) => ({ ...successResult(request), routeWindowId: 'main-window-9' })],
      ['cross-target', (request) => ({ ...successResult(request), targetWindowId: 'main-window-9' })],
      ['path-route', (request) => ({
        ...successResult(request), finalPath: { ...pathFor(request), routeWindowId: 'main-window-9' }
      })],
      ['path-target', (request) => ({
        ...successResult(request), finalPath: { ...pathFor(request), targetWindowId: 'main-window-9' }
      })],
      ['workspace', (request) => ({
        ...successResult(request), finalPath: { ...pathFor(request), workspaceId: 'workspace-wrong' }
      })],
      ['task', (request) => ({
        ...successResult(request), finalPath: { ...pathFor(request), taskId: 'task-wrong' }
      })],
      ['scene', (request) => ({
        ...successResult(request), finalPath: { ...pathFor(request), sceneId: 'scene-wrong' }
      })],
      ['session', (request) => ({
        ...successResult(request), finalPath: { ...pathFor(request), sessionId: 'session-wrong' }
      })],
      ['missing-session', (request) => {
        const { sessionId: _sessionId, ...withoutSession } = pathFor(request)
        return { ...successResult(request), finalPath: withoutSession }
      }]
    ]

    for (const [name, invalidResult] of invalidResults) {
      const pending = broker.navigate(navigationInput({ requestId: `invalid-${name}` }))
      const request = client.sent.at(-1)!
      expect(broker.acknowledge(invalidResult(request), registration), name).toBe(true)
      const error = await rejection(pending)
      expect(error, name).toMatchObject({ code: 'TARGET_NOT_READY' })
      expect(error.message, name).not.toMatch(/main-window-9|workspace-wrong|task-wrong|scene-wrong|session-wrong/)

      const retried = broker.navigate(navigationInput({ requestId: `invalid-${name}` }))
      const retryRequest = client.sent.at(-1)!
      expect(broker.acknowledge(successResult(retryRequest), registration), name).toBe(true)
      await expect(retried).resolves.toEqual({ finalPath: pathFor(retryRequest) })
    }
  })

  it('rejects an extra acknowledgement Session when the request has no Session', async () => {
    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const { sessionId: _sessionId, ...withoutSession } = navigationInput()
    const pending = broker.navigate({ ...withoutSession, requestId: 'unexpected-session' })
    const request = onlyRequest(client)
    const result = successResult(request)
    result.finalPath = { ...pathFor(request), sessionId: 'session-from-another-scene' }

    expect(broker.acknowledge(result, registration)).toBe(true)
    await expect(pending).rejects.toMatchObject({ code: 'TARGET_NOT_READY' })
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
    const request = onlyRequest(second)
    expect(first.sent).toHaveLength(1)
    expect(broker.acknowledge(successResult(request), firstRegistration)).toBe(false)
    expect(broker.acknowledge(successResult(request), secondRegistration)).toBe(true)
    await expect(current).resolves.toEqual({ finalPath: pathFor(request) })
  })

  it('rejects an offline or disconnected route and clears its pending request', async () => {
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
    const request = onlyRequest(replacement)
    expect(broker.acknowledge(successResult(request), replacementRegistration)).toBe(true)
    await expect(reused).resolves.toEqual({ finalPath: pathFor(request) })
  })

  it('binds a reused request ID to a new attempt so the old late acknowledgement is inert', async () => {
    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const oldPending = broker.navigate(navigationInput({ requestId: 'aba-request', deadlineAt: 1_200 }))
    const oldRequest = onlyRequest(client)
    const oldResult = expect(oldPending).rejects.toMatchObject({ code: 'NAVIGATION_TIMEOUT' })
    await vi.advanceTimersByTimeAsync(200)
    await oldResult

    const newPending = broker.navigate(navigationInput({ requestId: 'aba-request', deadlineAt: 2_000 }))
    const newRequest = client.sent.at(-1)!
    expect(newRequest.attemptId).not.toBe(oldRequest.attemptId)
    expect(broker.acknowledge(successResult(oldRequest), registration)).toBe(false)
    await expectPending(newPending)

    expect(broker.acknowledge(successResult(newRequest), registration)).toBe(true)
    await expect(newPending).resolves.toEqual({ finalPath: pathFor(newRequest) })
  })

  it('does not send after the second absolute-deadline check observes expiry', async () => {
    broker.close()
    const clock = [1_000, 1_200]
    broker = new HostNavigationBroker({ now: () => clock.shift() ?? 1_200 })
    const client = fakeClient()
    broker.registerWindow('main-window-2', client.send)

    const pending = broker.navigate(navigationInput({ requestId: 'arm-race', deadlineAt: 1_100 }))

    await expect(pending).rejects.toMatchObject({ code: 'NAVIGATION_TIMEOUT' })
    expect(client.sent).toEqual([])
  })

  it('uses bounded public errors without request, window, native, or Renderer diagnostics', async () => {
    const offline = await rejection(broker.navigate(navigationInput({
      requestId: 'secret-request-offline', routeWindowId: 'secret-main-window',
      targetWindowId: 'secret-native-window'
    })))
    expectPublicNavigationError(offline, ['secret-request-offline', 'secret-main-window', 'secret-native-window'])

    const client = fakeClient()
    const registration = broker.registerWindow('main-window-2', client.send)
    const active = broker.navigate(navigationInput({ requestId: 'secret-request-duplicate' }))
    const duplicate = await rejection(broker.navigate(navigationInput({ requestId: 'secret-request-duplicate' })))
    expectPublicNavigationError(duplicate, ['secret-request-duplicate'])
    const activeRequest = onlyRequest(client)
    broker.acknowledge(successResult(activeRequest), registration)
    await active

    const timeoutPending = broker.navigate(navigationInput({
      requestId: 'secret-request-timeout', deadlineAt: 1_100
    }))
    const timeoutResult = rejection(timeoutPending)
    await vi.advanceTimersByTimeAsync(100)
    expectPublicNavigationError(await timeoutResult, ['secret-request-timeout'])

    broker.unregisterWindow('main-window-2', registration)
    broker.registerWindow('main-window-2', () => { throw new Error('secret-native-send-diagnostic') })
    const sendFailure = await rejection(broker.navigate(navigationInput({ requestId: 'send-failure' })))
    expectPublicNavigationError(sendFailure, ['send-failure', 'secret-native-send-diagnostic'])

    const renderer = fakeClient()
    const rendererRegistration = broker.registerWindow('main-window-2', renderer.send)
    const refusedPending = broker.navigate(navigationInput({ requestId: 'renderer-refusal' }))
    const refusedResult = rejection(refusedPending)
    const rendererRequest = onlyRequest(renderer)
    broker.acknowledge({
      type: 'host.navigation-result', protocolVersion: 1,
      requestId: rendererRequest.requestId, attemptId: rendererRequest.attemptId,
      routeWindowId: rendererRequest.routeWindowId,
      targetWindowId: rendererRequest.targetWindowId,
      ok: false, error: 'secret-renderer-native-diagnostic'
    }, rendererRegistration)
    expectPublicNavigationError(await refusedResult, [
      'renderer-refusal', 'secret-renderer-native-diagnostic'
    ])
  })

  it('settles every pending request when the Runtime navigation bridge closes', async () => {
    const first = fakeClient()
    const second = fakeClient()
    broker.registerWindow('main-window-1', first.send)
    broker.registerWindow('main-window-2', second.send)
    const one = broker.navigate(navigationInput({
      requestId: 'shutdown-1', routeWindowId: 'main-window-1', targetWindowId: 'main-window-1'
    }))
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

function fakeClient(): { sent: HostNavigationRequestWire[]; send: HostNavigationSender } {
  const sent: HostNavigationRequestWire[] = []
  return { sent, send: (message) => { sent.push(message) } }
}

function onlyRequest(client: { sent: HostNavigationRequestWire[] }): HostNavigationRequestWire {
  expect(client.sent).toHaveLength(1)
  return client.sent[0]!
}

function navigationInput(
  overrides: Partial<HostNavigationRequestInput> = {}
): HostNavigationRequestInput {
  return {
    requestId: 'nav-1',
    routeWindowId: 'main-window-2',
    targetWindowId: 'main-window-2',
    workspaceId: 'workspace-2',
    taskId: 'task-2',
    sceneId: 'scene-2',
    sessionId: 'session-2',
    focusTerminal: true,
    deadlineAt: 5_000,
    ...overrides
  }
}

function pathFor(request: HostNavigationRequestWire): HostNavigationPath {
  return {
    routeWindowId: request.routeWindowId,
    targetWindowId: request.targetWindowId,
    workspaceId: request.workspaceId,
    taskId: request.taskId,
    sceneId: request.sceneId,
    ...(request.sessionId === undefined ? {} : { sessionId: request.sessionId })
  }
}

function successResult(request: HostNavigationRequestWire): HostNavigationResultWire {
  return {
    type: 'host.navigation-result',
    protocolVersion: 1,
    requestId: request.requestId,
    attemptId: request.attemptId,
    routeWindowId: request.routeWindowId,
    targetWindowId: request.targetWindowId,
    ok: true,
    finalPath: pathFor(request)
  }
}

async function rejection(promise: Promise<unknown>): Promise<Error & { code?: string }> {
  try {
    await promise
  } catch (error) {
    return error as Error & { code?: string }
  }
  throw new Error('expected rejection')
}

async function expectPending(promise: Promise<unknown>): Promise<void> {
  const settled = vi.fn()
  void promise.then(settled, settled)
  await Promise.resolve()
  expect(settled).not.toHaveBeenCalled()
}

function expectPublicNavigationError(error: Error & { code?: string }, secrets: string[]): void {
  expect(['TARGET_NOT_READY', 'NAVIGATION_TIMEOUT']).toContain(error.code)
  for (const secret of secrets) expect(error.message).not.toContain(secret)
}
