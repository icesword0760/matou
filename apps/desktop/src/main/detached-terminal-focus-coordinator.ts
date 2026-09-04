import type {
  DetachedTerminalFocusRequest,
  DetachedTerminalFocusResult
} from '../shared/desktop-api'

export interface DetachedTerminalFocusTarget {
  windowId: string
  mainWindowId: string
  sessionId: string
  webContentsId: number
  showAndFocus(): void
  send(request: DetachedTerminalFocusRequest): void
  isFocused(): boolean
}

interface DetachedTerminalFocusCoordinatorOptions {
  resolveTarget(windowId: string): DetachedTerminalFocusTarget | undefined
  now?: () => number
}

interface PendingFocusAttempt {
  request: DetachedTerminalFocusRequest
  target: DetachedTerminalFocusTarget
  promise: Promise<boolean>
  resolve(value: boolean): void
  timer: ReturnType<typeof setTimeout>
}

/**
 * Correlates a navigation attempt with proof from the exact detached Renderer.
 * A Renderer claim is accepted only while its native BrowserWindow is focused.
 */
export class DetachedTerminalFocusCoordinator {
  readonly #resolveTarget: DetachedTerminalFocusCoordinatorOptions['resolveTarget']
  readonly #now: () => number
  readonly #pending = new Map<string, PendingFocusAttempt>()

  constructor(options: DetachedTerminalFocusCoordinatorOptions) {
    this.#resolveTarget = options.resolveTarget
    this.#now = options.now ?? Date.now
  }

  request(request: DetachedTerminalFocusRequest): Promise<boolean> {
    const key = focusAttemptKey(request)
    const existing = this.#pending.get(key)
    if (existing) return existing.promise
    const target = this.#resolveTarget(request.targetWindowId)
    if (
      !target || request.deadlineAt <= this.#now() ||
      target.windowId !== request.targetWindowId ||
      target.mainWindowId !== request.routeWindowId ||
      target.sessionId !== request.sessionId
    ) return Promise.resolve(false)

    let settle!: (value: boolean) => void
    const promise = new Promise<boolean>((resolve) => { settle = resolve })
    const timer = setTimeout(() => this.#settle(key, false), Math.max(0, request.deadlineAt - this.#now()))
    const pending: PendingFocusAttempt = {
      request, target, promise, resolve: settle, timer
    }
    this.#pending.set(key, pending)
    try {
      target.showAndFocus()
      target.send(request)
    } catch {
      this.#settle(key, false)
    }
    return promise
  }

  acknowledge(result: DetachedTerminalFocusResult, senderWebContentsId: number): boolean {
    const key = focusAttemptKey(result)
    const pending = this.#pending.get(key)
    if (!pending) return false
    const { request, target } = pending
    if (
      senderWebContentsId !== target.webContentsId ||
      result.routeWindowId !== request.routeWindowId ||
      result.targetWindowId !== request.targetWindowId ||
      result.sessionId !== request.sessionId ||
      result.requestId !== request.requestId ||
      result.attemptId !== request.attemptId ||
      this.#now() >= request.deadlineAt
    ) return false
    if (!result.focused) {
      this.#settle(key, false)
      return true
    }
    if (!target.isFocused()) return false
    this.#settle(key, true)
    return true
  }

  cancelWindow(windowId: string): void {
    for (const [key, pending] of this.#pending) {
      if (pending.target.windowId === windowId) this.#settle(key, false)
    }
  }

  #settle(key: string, result: boolean): void {
    const pending = this.#pending.get(key)
    if (!pending) return
    this.#pending.delete(key)
    clearTimeout(pending.timer)
    pending.resolve(result)
  }
}

function focusAttemptKey(value: Pick<DetachedTerminalFocusRequest, 'requestId' | 'attemptId'>): string {
  return `${value.requestId}\u0000${value.attemptId}`
}
