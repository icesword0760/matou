import { randomUUID } from 'node:crypto'

import {
  PROTOCOL_VERSION,
  type HostNavigationPath,
  type HostNavigationRequestWire,
  type HostNavigationResultWire
} from '@matou/contracts'

export type HostNavigationBrokerErrorCode = 'NAVIGATION_TIMEOUT' | 'TARGET_NOT_READY'

export class HostNavigationBrokerError extends Error {
  readonly code: HostNavigationBrokerErrorCode

  constructor(
    code: HostNavigationBrokerErrorCode,
    message: string,
    options: { cause?: unknown } = {}
  ) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause })
    this.name = 'HostNavigationBrokerError'
    this.code = code
  }
}

export type HostNavigationRequestInput = Omit<
  HostNavigationRequestWire,
  'type' | 'protocolVersion' | 'attemptId'
>

export type HostNavigationSender = (message: HostNavigationRequestWire) => void

export interface HostNavigationRegistration {
  readonly windowId: string
  readonly generation: number
  readonly sender: HostNavigationSender
}

export interface HostNavigationAcknowledgement {
  finalPath: HostNavigationPath
}

export interface HostNavigationBrokerOptions {
  now?: () => number
}

interface PendingNavigation {
  readonly key: string
  readonly request: HostNavigationRequestWire
  readonly registration: HostNavigationRegistration
  readonly resolve: (result: HostNavigationAcknowledgement) => void
  readonly reject: (error: HostNavigationBrokerError) => void
  timer: ReturnType<typeof setTimeout> | undefined
}

const TARGET_NOT_READY_MESSAGE = '目标窗口当前未就绪，请稍后重试'
const REQUEST_IN_PROGRESS_MESSAGE = '导航请求正在处理中，请稍后重试'
const SEND_FAILED_MESSAGE = '导航请求发送失败，请稍后重试'
const RENDERER_REJECTED_MESSAGE = '目标窗口未完成导航，请重试'
const INVALID_ACK_MESSAGE = '目标窗口返回的导航结果无效，请重试'
const TIMEOUT_MESSAGE = '目标窗口响应超时，请重试'
const CLOSED_MESSAGE = '导航服务已停止，请稍后重试'

/**
 * Process-scoped request broker between Host Control and authenticated main-window ports.
 * Pending acknowledgements retain the exact registration and per-attempt identity that received
 * each request. Detached/native destination keys are payload only and never registration keys.
 */
export class HostNavigationBroker {
  readonly #registrations = new Map<string, HostNavigationRegistration>()
  readonly #pending = new Map<string, PendingNavigation>()
  readonly #now: () => number
  #nextGeneration = 0
  #closed = false

  constructor(options: HostNavigationBrokerOptions = {}) {
    this.#now = options.now ?? Date.now
  }

  registerWindow(windowId: string, sender: HostNavigationSender): HostNavigationRegistration {
    if (this.#closed) {
      throw new HostNavigationBrokerError('TARGET_NOT_READY', CLOSED_MESSAGE)
    }
    const previous = this.#registrations.get(windowId)
    const registration = Object.freeze({
      windowId,
      generation: ++this.#nextGeneration,
      sender
    })
    this.#registrations.set(windowId, registration)
    if (previous) {
      this.#rejectRegistration(
        previous,
        new HostNavigationBrokerError('TARGET_NOT_READY', TARGET_NOT_READY_MESSAGE, {
          cause: diagnostic(`replaced route window ${windowId}`)
        })
      )
    }
    return registration
  }

  unregisterWindow(windowId: string, registration: HostNavigationRegistration): boolean {
    const current = this.#registrations.get(windowId)
    if (!current || current !== registration) return false
    this.#registrations.delete(windowId)
    this.#rejectRegistration(
      current,
      new HostNavigationBrokerError('TARGET_NOT_READY', TARGET_NOT_READY_MESSAGE, {
        cause: diagnostic(`disconnected route window ${windowId}`)
      })
    )
    return true
  }

  navigate(input: HostNavigationRequestInput): Promise<HostNavigationAcknowledgement> {
    if (this.#closed) {
      return Promise.reject(new HostNavigationBrokerError('TARGET_NOT_READY', CLOSED_MESSAGE))
    }
    if (input.deadlineAt <= this.#now()) {
      return Promise.reject(this.#timeoutError(input.requestId))
    }
    const registration = this.#registrations.get(input.routeWindowId)
    if (!registration) {
      return Promise.reject(new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        TARGET_NOT_READY_MESSAGE,
        { cause: diagnostic(`offline route=${input.routeWindowId} target=${input.targetWindowId}`) }
      ))
    }
    const key = pendingKey(input.routeWindowId, input.requestId)
    if (this.#pending.has(key)) {
      return Promise.reject(new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        REQUEST_IN_PROGRESS_MESSAGE,
        { cause: diagnostic(`duplicate navigation request ${input.requestId}`) }
      ))
    }

    const request: HostNavigationRequestWire = {
      type: 'host.navigation-request',
      protocolVersion: PROTOCOL_VERSION,
      attemptId: `navigation-attempt-${randomUUID()}`,
      ...input
    }
    let resolve!: PendingNavigation['resolve']
    let reject!: PendingNavigation['reject']
    const promise = new Promise<HostNavigationAcknowledgement>((done, fail) => {
      resolve = done
      reject = fail
    })
    const pending: PendingNavigation = {
      key, request, registration, resolve, reject, timer: undefined
    }
    this.#pending.set(key, pending)
    if (!this.#armDeadline(pending)) return promise

    try {
      registration.sender(request)
    } catch (error) {
      this.#reject(pending, new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        SEND_FAILED_MESSAGE,
        { cause: error }
      ))
    }
    return promise
  }

  acknowledge(
    result: HostNavigationResultWire,
    registration: HostNavigationRegistration
  ): boolean {
    const pending = this.#pendingForAcknowledgement(result, registration)
    if (!pending) return false
    const current = this.#registrations.get(pending.request.routeWindowId)
    if (
      current !== pending.registration ||
      pending.registration !== registration ||
      result.attemptId !== pending.request.attemptId
    ) {
      return false
    }
    if (pending.request.deadlineAt <= this.#now()) {
      this.#reject(pending, this.#timeoutError(pending.request.requestId))
      return true
    }
    if (
      result.routeWindowId !== pending.request.routeWindowId ||
      result.targetWindowId !== pending.request.targetWindowId
    ) {
      this.#rejectInvalidAcknowledgement(pending, result)
      return true
    }
    if (!result.ok) {
      this.#reject(pending, new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        RENDERER_REJECTED_MESSAGE,
        { cause: result.error === undefined ? undefined : diagnostic(result.error) }
      ))
      return true
    }
    if (!result.finalPath || !sameNavigationPath(result.finalPath, pending.request)) {
      this.#rejectInvalidAcknowledgement(pending, result)
      return true
    }

    this.#resolve(pending, { finalPath: copyNavigationPath(result.finalPath) })
    return true
  }

  close(): void {
    if (this.#closed) return
    this.#closed = true
    this.#registrations.clear()
    const error = new HostNavigationBrokerError('TARGET_NOT_READY', CLOSED_MESSAGE)
    for (const pending of [...this.#pending.values()]) this.#reject(pending, error)
  }

  #pendingForAcknowledgement(
    result: HostNavigationResultWire,
    registration: HostNavigationRegistration
  ): PendingNavigation | undefined {
    return [...this.#pending.values()].find((pending) =>
      pending.registration === registration &&
      pending.request.requestId === result.requestId &&
      pending.request.attemptId === result.attemptId
    )
  }

  #armDeadline(pending: PendingNavigation): boolean {
    const remaining = pending.request.deadlineAt - this.#now()
    if (remaining <= 0) {
      this.#reject(pending, this.#timeoutError(pending.request.requestId))
      return false
    }
    pending.timer = setTimeout(() => {
      pending.timer = undefined
      if (this.#pending.get(pending.key) !== pending) return
      if (pending.request.deadlineAt > this.#now()) {
        this.#armDeadline(pending)
        return
      }
      this.#reject(pending, this.#timeoutError(pending.request.requestId))
    }, Math.min(remaining, 2_147_483_647))
    return true
  }

  #resolve(pending: PendingNavigation, result: HostNavigationAcknowledgement): void {
    if (!this.#release(pending)) return
    pending.resolve(result)
  }

  #reject(pending: PendingNavigation, error: HostNavigationBrokerError): void {
    if (!this.#release(pending)) return
    pending.reject(error)
  }

  #release(pending: PendingNavigation): boolean {
    if (this.#pending.get(pending.key) !== pending) return false
    this.#pending.delete(pending.key)
    if (pending.timer !== undefined) clearTimeout(pending.timer)
    pending.timer = undefined
    return true
  }

  #rejectRegistration(
    registration: HostNavigationRegistration,
    error: HostNavigationBrokerError
  ): void {
    for (const pending of [...this.#pending.values()]) {
      if (pending.registration === registration) this.#reject(pending, error)
    }
  }

  #rejectInvalidAcknowledgement(
    pending: PendingNavigation,
    result: HostNavigationResultWire
  ): void {
    this.#reject(pending, new HostNavigationBrokerError(
      'TARGET_NOT_READY',
      INVALID_ACK_MESSAGE,
      { cause: diagnostic(`invalid navigation acknowledgement ${JSON.stringify(result)}`) }
    ))
  }

  #timeoutError(requestId: string): HostNavigationBrokerError {
    return new HostNavigationBrokerError(
      'NAVIGATION_TIMEOUT',
      TIMEOUT_MESSAGE,
      { cause: diagnostic(`navigation request ${requestId} exceeded its deadline`) }
    )
  }
}

function sameNavigationPath(
  path: NonNullable<HostNavigationResultWire['finalPath']>,
  request: HostNavigationRequestWire
): boolean {
  return path.routeWindowId === request.routeWindowId &&
    path.targetWindowId === request.targetWindowId &&
    path.workspaceId === request.workspaceId &&
    path.taskId === request.taskId &&
    path.sceneId === request.sceneId &&
    path.sessionId === request.sessionId
}

function copyNavigationPath(
  path: NonNullable<HostNavigationResultWire['finalPath']>
): HostNavigationPath {
  return {
    routeWindowId: path.routeWindowId,
    targetWindowId: path.targetWindowId,
    workspaceId: path.workspaceId,
    taskId: path.taskId,
    sceneId: path.sceneId,
    ...(path.sessionId === undefined ? {} : { sessionId: path.sessionId })
  }
}

function pendingKey(routeWindowId: string, requestId: string): string {
  return `${routeWindowId}\u0000${requestId}`
}

function diagnostic(message: string): Error {
  return new Error(message)
}
