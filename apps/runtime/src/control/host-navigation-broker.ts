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
  'type' | 'protocolVersion'
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

interface PendingNavigation {
  readonly key: string
  readonly input: HostNavigationRequestInput
  readonly registration: HostNavigationRegistration
  readonly resolve: (result: HostNavigationAcknowledgement) => void
  readonly reject: (error: HostNavigationBrokerError) => void
  timer: ReturnType<typeof setTimeout> | undefined
}

type RegistrationIdentity = HostNavigationRegistration | HostNavigationSender

/**
 * Process-scoped request broker between Host Control and authenticated main-window ports.
 * Pending acknowledgements retain the exact registration generation that received the request.
 */
export class HostNavigationBroker {
  readonly #registrations = new Map<string, HostNavigationRegistration>()
  readonly #pending = new Map<string, PendingNavigation>()
  #nextGeneration = 0
  #closed = false

  registerWindow(windowId: string, sender: HostNavigationSender): HostNavigationRegistration {
    if (this.#closed) {
      throw new HostNavigationBrokerError('TARGET_NOT_READY', 'Runtime 导航服务已关闭')
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
        new HostNavigationBrokerError('TARGET_NOT_READY', '目标窗口已重新连接，请重试导航')
      )
    }
    return registration
  }

  unregisterWindow(windowId: string, identity?: RegistrationIdentity): boolean {
    const current = this.#registrations.get(windowId)
    if (!current || (identity !== undefined && !sameRegistration(current, identity))) return false
    this.#registrations.delete(windowId)
    this.#rejectRegistration(
      current,
      new HostNavigationBrokerError('TARGET_NOT_READY', '目标窗口导航连接已断开')
    )
    return true
  }

  navigate(input: HostNavigationRequestInput): Promise<HostNavigationAcknowledgement> {
    if (this.#closed) {
      return Promise.reject(
        new HostNavigationBrokerError('TARGET_NOT_READY', 'Runtime 导航服务已关闭')
      )
    }
    if (input.deadlineAt <= Date.now()) {
      return Promise.reject(this.#timeoutError(input))
    }
    const registration = this.#registrations.get(input.windowId)
    if (!registration) {
      return Promise.reject(new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        `目标窗口 ${input.windowId} 当前离线`
      ))
    }
    const key = pendingKey(input.windowId, input.requestId)
    if (this.#pending.has(key)) {
      return Promise.reject(new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        `导航请求 ${input.requestId} 正在处理中`
      ))
    }

    let resolve!: PendingNavigation['resolve']
    let reject!: PendingNavigation['reject']
    const promise = new Promise<HostNavigationAcknowledgement>((done, fail) => {
      resolve = done
      reject = fail
    })
    const pending: PendingNavigation = {
      key, input, registration, resolve, reject, timer: undefined
    }
    this.#pending.set(key, pending)
    this.#armDeadline(pending)
    try {
      registration.sender({
        type: 'host.navigation-request',
        protocolVersion: PROTOCOL_VERSION,
        ...input
      })
    } catch (error) {
      this.#reject(pending, new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        `目标窗口发送导航请求失败: ${errorMessage(error)}`,
        { cause: error }
      ))
    }
    return promise
  }

  acknowledge(result: HostNavigationResultWire, identity?: RegistrationIdentity): boolean {
    const pending = this.#pending.get(pendingKey(result.windowId, result.requestId))
    if (!pending) return false
    const current = this.#registrations.get(result.windowId)
    if (
      current !== pending.registration ||
      (identity !== undefined && !sameRegistration(pending.registration, identity))
    ) {
      return false
    }
    if (pending.input.deadlineAt <= Date.now()) {
      this.#reject(pending, this.#timeoutError(pending.input))
      return false
    }
    if (!result.ok) {
      this.#reject(pending, new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        result.error ?? '目标窗口未完成导航'
      ))
      return true
    }
    if (!result.finalPath || result.finalPath.windowId !== pending.input.windowId) {
      this.#reject(pending, new HostNavigationBrokerError(
        'TARGET_NOT_READY',
        '目标窗口返回的导航路径无效'
      ))
      return true
    }
    this.#resolve(pending, {
      finalPath: {
        windowId: result.finalPath.windowId,
        workspaceId: result.finalPath.workspaceId,
        taskId: result.finalPath.taskId,
        sceneId: result.finalPath.sceneId,
        ...(result.finalPath.sessionId === undefined
          ? {}
          : { sessionId: result.finalPath.sessionId })
      }
    })
    return true
  }

  close(): void {
    if (this.#closed) return
    this.#closed = true
    this.#registrations.clear()
    const error = new HostNavigationBrokerError('TARGET_NOT_READY', 'Runtime 导航服务已关闭')
    for (const pending of [...this.#pending.values()]) this.#reject(pending, error)
  }

  #armDeadline(pending: PendingNavigation): void {
    const remaining = pending.input.deadlineAt - Date.now()
    if (remaining <= 0) {
      this.#reject(pending, this.#timeoutError(pending.input))
      return
    }
    pending.timer = setTimeout(() => {
      pending.timer = undefined
      if (this.#pending.get(pending.key) !== pending) return
      if (pending.input.deadlineAt > Date.now()) {
        this.#armDeadline(pending)
        return
      }
      this.#reject(pending, this.#timeoutError(pending.input))
    }, Math.min(remaining, 2_147_483_647))
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

  #timeoutError(input: HostNavigationRequestInput): HostNavigationBrokerError {
    return new HostNavigationBrokerError(
      'NAVIGATION_TIMEOUT',
      `导航请求 ${input.requestId} 已超过确认截止时间`
    )
  }
}

function sameRegistration(
  registration: HostNavigationRegistration,
  identity: RegistrationIdentity
): boolean {
  return typeof identity === 'function'
    ? registration.sender === identity
    : registration === identity
}

function pendingKey(windowId: string, requestId: string): string {
  return `${windowId}\u0000${requestId}`
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
