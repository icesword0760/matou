import {
  PROTOCOL_VERSION,
  type RpcMethod,
  type RuntimeMessage
} from '@matou/contracts'

export interface RuntimeClientPort {
  onmessage: ((event: MessageEvent<RuntimeMessage>) => void) | null
  postMessage(message: unknown): void
  start(): void
  close(): void
}

export interface TerminalAttachment {
  sessionId: string
  executionContextId: string
  profile: 'shell' | 'claude-code' | 'codex'
  cols: number
  rows: number
}

interface PendingRequest {
  resolve(value: unknown): void
  reject(error: Error): void
  timeout: ReturnType<typeof setTimeout>
}

interface TerminalConsumer {
  config: TerminalAttachment
  listeners: Set<(message: RuntimeMessage) => void>
}

export class RuntimeClient {
  readonly #clientId: string
  readonly #requestTimeoutMs: number
  readonly #requests = new Map<string, PendingRequest>()
  readonly #terminals = new Map<string, TerminalConsumer>()
  readonly #projectionListeners = new Set<(message: RuntimeMessage) => void>()
  readonly #readyWaiters = new Set<() => void>()
  #port: RuntimeClientPort
  #ready = false
  #projectionAfterSequence: number | undefined

  constructor(
    port: RuntimeClientPort,
    options: { clientId?: string; requestTimeoutMs?: number } = {}
  ) {
    this.#port = port
    this.#clientId = options.clientId ?? crypto.randomUUID()
    this.#requestTimeoutMs = options.requestTimeoutMs ?? 10_000
    this.#bindPort(port)
  }

  replacePort(port: RuntimeClientPort): void {
    this.#port.close()
    this.#port = port
    this.#ready = false
    for (const [requestId, pending] of this.#requests) {
      clearTimeout(pending.timeout)
      pending.reject(new Error('Runtime channel replaced before the request completed'))
      this.#requests.delete(requestId)
    }
    this.#bindPort(port)
  }

  async request<T = unknown>(method: RpcMethod, payload: unknown): Promise<T> {
    await this.whenReady()
    const requestId = crypto.randomUUID()
    return new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.#requests.delete(requestId)
        this.#post({
          type: 'rpc.cancel', protocolVersion: PROTOCOL_VERSION, requestId
        })
        reject(new Error(`Runtime request ${method} timed out`))
      }, this.#requestTimeoutMs)
      this.#requests.set(requestId, {
        resolve: (value) => resolve(value as T), reject, timeout
      })
      this.#post({
        type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
        requestId, method, capability: 'renderer',
        deadlineAt: Date.now() + this.#requestTimeoutMs, payload
      })
    })
  }

  whenReady(): Promise<void> {
    if (this.#ready) return Promise.resolve()
    return new Promise((resolve) => this.#readyWaiters.add(resolve))
  }

  subscribeProjection(listener: (message: RuntimeMessage) => void): () => void {
    this.#projectionListeners.add(listener)
    return () => this.#projectionListeners.delete(listener)
  }

  startProjection(afterSequence: number): void {
    this.#projectionAfterSequence = afterSequence
    if (this.#ready) this.#subscribeEvents(afterSequence)
  }

  attachTerminal(
    config: TerminalAttachment,
    listener: (message: RuntimeMessage) => void
  ): () => void {
    const consumer = this.#terminals.get(config.sessionId) ?? {
      config,
      listeners: new Set<(message: RuntimeMessage) => void>()
    }
    consumer.config = config
    consumer.listeners.add(listener)
    this.#terminals.set(config.sessionId, consumer)
    if (this.#ready && consumer.listeners.size === 1) this.#spawn(config)
    return () => {
      consumer.listeners.delete(listener)
      if (consumer.listeners.size === 0) this.#terminals.delete(config.sessionId)
    }
  }

  sendTerminalInput(sessionId: string, data: string): void {
    this.#post({ type: 'terminal.input', protocolVersion: PROTOCOL_VERSION, sessionId, data })
  }

  recordTerminalInteraction(
    sessionId: string,
    interactionKind: 'submit' | 'control' | 'provider-action'
  ): void {
    this.#post({
      type: 'terminal.user-interaction', protocolVersion: PROTOCOL_VERSION,
      sessionId, interactionKind
    })
  }

  resizeTerminal(sessionId: string, cols: number, rows: number): void {
    this.#post({ type: 'terminal.resize', protocolVersion: PROTOCOL_VERSION, sessionId, cols, rows })
  }

  acknowledgeTerminal(sessionId: string, throughSequence: number): void {
    this.#post({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId, throughSequence
    })
  }

  requestTerminalReplay(sessionId: string, fromSequence = 0): void {
    this.#post({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId, fromSequence
    })
  }

  disposeDeletedTerminal(sessionId: string): void {
    this.#terminals.delete(sessionId)
    this.#post({ type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId })
  }

  #bindPort(port: RuntimeClientPort): void {
    port.onmessage = (event) => this.#receive(event.data)
    port.start()
    port.postMessage({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: this.#clientId
    })
  }

  #receive(message: RuntimeMessage): void {
    if (message.type === 'protocol.ready') {
      this.#ready = true
      for (const resolve of this.#readyWaiters) resolve()
      this.#readyWaiters.clear()
      for (const { config } of this.#terminals.values()) this.#spawn(config)
      if (this.#projectionAfterSequence !== undefined) {
        this.#subscribeEvents(this.#projectionAfterSequence)
      }
      return
    }
    if (message.type === 'rpc.response' || message.type === 'rpc.error') {
      const pending = this.#requests.get(message.requestId)
      if (!pending) return
      clearTimeout(pending.timeout)
      this.#requests.delete(message.requestId)
      if (message.type === 'rpc.response') pending.resolve(message.result)
      else pending.reject(Object.assign(new Error(message.message), { code: message.code }))
      return
    }
    if ('sessionId' in message && typeof message.sessionId === 'string') {
      for (const listener of this.#terminals.get(message.sessionId)?.listeners ?? []) {
        listener(message)
      }
    }
    if (message.type === 'events.batch' || message.type === 'terminal.hud') {
      for (const listener of this.#projectionListeners) listener(message)
    }
  }

  #spawn(config: TerminalAttachment): void {
    this.#post({ type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, ...config })
  }

  #subscribeEvents(afterSequence: number): void {
    this.#post({
      type: 'events.subscribe', protocolVersion: PROTOCOL_VERSION,
      consumerId: `${this.#clientId}-projection`, afterSequence, batchSize: 250
    })
  }

  #post(message: unknown): void {
    this.#port.postMessage(message)
  }
}
