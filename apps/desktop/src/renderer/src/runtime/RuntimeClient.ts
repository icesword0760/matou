import {
  PROTOCOL_VERSION,
  type HostNavigationRequestWire,
  type HostNavigationResultWire,
  type RpcMethod,
  type RuntimeMessage,
  type RuntimeMode,
  type TerminalHistoryCursor,
  type TerminalHistoryPage,
  type TerminalHistorySearchOptions,
  type TerminalHistorySearchResult
} from '@matou/contracts'

import { splitUtf8ForTransport } from '../terminal/terminal-input-chunker'

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
  spawnRevision?: number
  readOnly?: boolean
}

export type SessionRecoveryStatus = Extract<RuntimeMessage, { type: 'session.recovery-status' }>
export type HostNavigationResultInput = Omit<
  HostNavigationResultWire,
  'type' | 'protocolVersion'
>

export interface RuntimeClientOptions {
  clientId?: string
  requestTimeoutMs?: number
  windowId?: string
  windowKind?: 'main' | 'detached-terminal'
}

interface PendingRequest {
  resolve(value: unknown): void
  reject(error: Error): void
  timeout: ReturnType<typeof setTimeout>
}

interface TerminalConsumer {
  config: TerminalAttachment
  requestedReadOnly: boolean
  listeners: Set<(message: RuntimeMessage) => void>
}

interface TerminalCheckpoint {
  throughSequence: number
  screenEpoch: number
  snapshot: string
}

interface TerminalCheckpointQueue {
  inFlight?: TerminalCheckpoint
  pending?: TerminalCheckpoint
}

export class RuntimeClient {
  readonly #clientId: string
  readonly #requestTimeoutMs: number
  readonly #requests = new Map<string, PendingRequest>()
  readonly #terminals = new Map<string, TerminalConsumer>()
  #foregroundTerminalSessions = new Set<string>()
  readonly #terminalCheckpoints = new Map<string, TerminalCheckpointQueue>()
  readonly #terminalResizeIds = new Map<string, number>()
  readonly #projectionListeners = new Set<(message: RuntimeMessage) => void>()
  readonly #hostNavigationListeners = new Set<(request: HostNavigationRequestWire) => void>()
  readonly #hostNavigationPorts = new Map<string, RuntimeClientPort>()
  readonly #recoveryListeners = new Set<(status: SessionRecoveryStatus) => void>()
  readonly #recoveryResetListeners = new Set<() => void>()
  readonly #recoveryStatuses = new Map<string, SessionRecoveryStatus>()
  readonly #readyWaiters = new Set<() => void>()
  #port: RuntimeClientPort
  #ready = false
  #readOnly = false
  #projectionAfterSequence: number | undefined
  #requiresRecoverySnapshot = true
  readonly #windowIdentity: Pick<RuntimeClientOptions, 'windowId' | 'windowKind'>
  #disposed = false

  constructor(
    port: RuntimeClientPort,
    options: RuntimeClientOptions = {}
  ) {
    this.#port = port
    this.#clientId = options.clientId ?? crypto.randomUUID()
    this.#requestTimeoutMs = options.requestTimeoutMs ?? 10_000
    this.#windowIdentity = options.windowId !== undefined && options.windowKind !== undefined
      ? { windowId: options.windowId, windowKind: options.windowKind }
      : {}
    this.#bindPort(port)
  }

  replacePort(port: RuntimeClientPort): void {
    const previousPort = this.#port
    previousPort.onmessage = null
    previousPort.close()
    this.#port = port
    this.#ready = false
    this.#requiresRecoverySnapshot = true
    this.#resetRecoveryStatuses()
    this.#markTerminalsQueuedForRecovery()
    // An acknowledgement belongs to the port generation that carried the
    // write. A replacement Runtime may already have committed it or may never
    // have seen it, so discard transport state and let the next replay/output
    // snapshot establish a fresh authoritative checkpoint.
    this.#terminalCheckpoints.clear()
    this.#hostNavigationPorts.clear()
    for (const [requestId, pending] of this.#requests) {
      clearTimeout(pending.timeout)
      pending.reject(new Error('Runtime channel replaced before the request completed'))
      this.#requests.delete(requestId)
    }
    this.#bindPort(port)
  }

  dispose(): void {
    if (this.#disposed) return
    this.#disposed = true
    this.#port.onmessage = null
    this.#port.close()
    this.#hostNavigationPorts.clear()
    this.#hostNavigationListeners.clear()
    this.#projectionListeners.clear()
    this.#recoveryListeners.clear()
    this.#recoveryResetListeners.clear()
    for (const [requestId, pending] of this.#requests) {
      clearTimeout(pending.timeout)
      pending.reject(new Error('Runtime client was disposed before the request completed'))
      this.#requests.delete(requestId)
    }
  }

  setRuntimeMode(mode: RuntimeMode): void {
    this.#readOnly = mode === 'read-only'
    if (this.#readOnly) this.#terminalCheckpoints.clear()
    for (const consumer of this.#terminals.values()) {
      consumer.config = effectiveTerminalConfig(
        consumer.config,
        consumer.requestedReadOnly || this.#readOnly
      )
    }
  }

  async request<T = unknown>(
    method: RpcMethod,
    payload: unknown,
    options: { timeoutMs?: number } = {}
  ): Promise<T> {
    await this.whenReady()
    const requestId = crypto.randomUUID()
    const timeoutMs = options.timeoutMs ?? this.#requestTimeoutMs
    return new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.#requests.delete(requestId)
        this.#post({
          type: 'rpc.cancel', protocolVersion: PROTOCOL_VERSION, requestId
        })
        reject(new Error(`Runtime request ${method} timed out`))
      }, timeoutMs)
      this.#requests.set(requestId, {
        resolve: (value) => resolve(value as T), reject, timeout
      })
      this.#post({
        type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
        requestId, method, capability: 'renderer',
        deadlineAt: Date.now() + timeoutMs, payload
      })
    })
  }

  whenReady(): Promise<void> {
    if (this.#ready) return Promise.resolve()
    return new Promise((resolve) => this.#readyWaiters.add(resolve))
  }

  pageTerminalHistory(
    sessionId: string,
    before?: TerminalHistoryCursor,
    lineLimit = 1_000
  ): Promise<TerminalHistoryPage> {
    return this.request('terminal.history-page', {
      sessionId,
      lineLimit,
      ...(before ? { before } : {})
    })
  }

  historyAroundTerminalCursor(
    sessionId: string,
    around: TerminalHistoryCursor,
    contextLines = 250
  ): Promise<TerminalHistoryPage> {
    return this.request('terminal.history-page', {
      sessionId,
      around,
      beforeLines: contextLines,
      afterLines: contextLines
    })
  }

  searchTerminalHistory(
    sessionId: string,
    query: string,
    options: TerminalHistorySearchOptions,
    before?: TerminalHistoryCursor,
    limit = 1_000
  ): Promise<TerminalHistorySearchResult> {
    return this.request('terminal.history-search', {
      sessionId,
      query,
      options,
      limit,
      ...(before ? { before } : {})
    })
  }

  subscribeProjection(listener: (message: RuntimeMessage) => void): () => void {
    this.#projectionListeners.add(listener)
    return () => this.#projectionListeners.delete(listener)
  }

  subscribeHostNavigation(listener: (request: HostNavigationRequestWire) => void): () => void {
    this.#hostNavigationListeners.add(listener)
    return () => this.#hostNavigationListeners.delete(listener)
  }

  acknowledgeHostNavigation(result: HostNavigationResultInput): void {
    const sourcePort = this.#hostNavigationPorts.get(hostNavigationAttemptKey(result))
    if (this.#disposed || sourcePort === undefined || sourcePort !== this.#port) return
    sourcePort.postMessage({
      type: 'host.navigation-result', protocolVersion: PROTOCOL_VERSION, ...result
    } satisfies HostNavigationResultWire)
  }

  subscribeSessionRecovery(
    listener: (status: SessionRecoveryStatus) => void,
    onReset?: () => void
  ): () => void {
    this.#recoveryListeners.add(listener)
    if (onReset) this.#recoveryResetListeners.add(onReset)
    for (const status of this.#recoveryStatuses.values()) listener(status)
    return () => {
      this.#recoveryListeners.delete(listener)
      if (onReset) this.#recoveryResetListeners.delete(onReset)
    }
  }

  prioritizeSessionRecovery(
    sceneId: string,
    activeSessionId?: string,
    foregroundSessionIds?: readonly string[]
  ): void {
    if (this.#readOnly) return
    this.#post({
      type: 'session.recovery-prioritize',
      protocolVersion: PROTOCOL_VERSION,
      sceneId,
      ...(activeSessionId === undefined ? {} : { activeSessionId }),
      ...(foregroundSessionIds === undefined ? {} : {
        foregroundSessionIds: [...foregroundSessionIds]
      })
    })
  }

  retrySessionRecovery(sessionId: string): void {
    if (this.#readOnly) return
    this.#post({
      type: 'session.recovery-retry', protocolVersion: PROTOCOL_VERSION, sessionId
    })
  }

  startProjection(afterSequence: number): void {
    this.#projectionAfterSequence = afterSequence
    if (this.#ready) this.#subscribeEvents(afterSequence)
  }

  attachTerminal(
    config: TerminalAttachment,
    listener: (message: RuntimeMessage) => void
  ): () => void {
    const requestedReadOnly = config.readOnly === true
    const consumer = this.#terminals.get(config.sessionId) ?? {
      config: effectiveTerminalConfig(config, requestedReadOnly || this.#readOnly),
      requestedReadOnly,
      listeners: new Set<(message: RuntimeMessage) => void>()
    }
    consumer.requestedReadOnly = requestedReadOnly
    consumer.config = effectiveTerminalConfig(config, requestedReadOnly || this.#readOnly)
    consumer.listeners.add(listener)
    this.#terminals.set(config.sessionId, consumer)
    if (
      this.#ready && consumer.listeners.size === 1 &&
      !this.#requiresRecoverySnapshot && this.#recoveryAllowsAttach(config.sessionId)
    ) this.#spawn(consumer.config)
    return () => {
      consumer.listeners.delete(listener)
      if (consumer.listeners.size === 0 && !this.#foregroundTerminalSessions.has(config.sessionId)) {
        this.#releaseTerminalView(config.sessionId, consumer)
      }
    }
  }

  /**
   * Keeps Runtime view bindings for the current horizontal sibling list even
   * when carousel virtualization temporarily removes a card from the DOM.
   * This does not eagerly spawn never-viewed Sessions; it retains only
   * consumers whose terminal surface has already been attached.
   */
  setForegroundTerminalSessions(sessionIds: readonly string[]): void {
    const next = new Set(sessionIds)
    for (const sessionId of this.#foregroundTerminalSessions) {
      if (next.has(sessionId)) continue
      const consumer = this.#terminals.get(sessionId)
      if (consumer?.listeners.size === 0) this.#releaseTerminalView(sessionId, consumer)
    }
    this.#foregroundTerminalSessions = next
  }

  updateTerminalProfile(
    sessionId: string,
    profile: TerminalAttachment['profile']
  ): void {
    const consumer = this.#terminals.get(sessionId)
    if (!consumer || consumer.config.profile === profile) return
    consumer.config = { ...consumer.config, profile }
  }

  sendTerminalInput(sessionId: string, data: string): void {
    if (this.#readOnly) return
    for (const chunk of splitUtf8ForTransport(data)) {
      this.#post({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION, sessionId, data: chunk
      })
    }
  }

  retryLastTerminalInput(sessionId: string): void {
    if (this.#readOnly) return
    this.#post({
      type: 'terminal.retry-last-input', protocolVersion: PROTOCOL_VERSION, sessionId
    })
  }

  retryTerminalStorage(sessionId: string): void {
    if (this.#readOnly) return
    this.#post({
      type: 'terminal.storage-retry', protocolVersion: PROTOCOL_VERSION, sessionId
    })
  }

  endTerminalAfterStorageFault(sessionId: string): void {
    if (this.#readOnly) return
    this.#post({
      type: 'terminal.storage-end', protocolVersion: PROTOCOL_VERSION, sessionId
    })
  }

  recordTerminalInteraction(
    sessionId: string,
    interactionKind: 'submit' | 'control' | 'provider-action',
    deferOrdering = false
  ): void {
    if (this.#readOnly) return
    this.#post({
      type: 'terminal.user-interaction', protocolVersion: PROTOCOL_VERSION,
      sessionId, interactionKind, deferOrdering
    })
  }

  resizeTerminal(sessionId: string, cols: number, rows: number): void {
    if (this.#readOnly) return
    const resizeId = (this.#terminalResizeIds.get(sessionId) ?? 0) + 1
    this.#terminalResizeIds.set(sessionId, resizeId)
    this.#post({
      type: 'terminal.resize', protocolVersion: PROTOCOL_VERSION,
      sessionId, resizeId, cols, rows
    })
  }

  acknowledgeTerminal(sessionId: string, throughSequence: number): void {
    this.#post({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId, throughSequence
    })
  }

  requestTerminalReplay(
    sessionId: string,
    fromSequence = 0,
    preserveExistingModel = false
  ): void {
    this.#post({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId, fromSequence,
      ...(preserveExistingModel ? { preserveExistingModel: true } : {})
    })
  }

  storeTerminalCheckpoint(
    sessionId: string,
    throughSequence: number,
    screenEpoch: number,
    snapshot: string
  ): void {
    if (this.#readOnly) return
    const checkpoint = { throughSequence, screenEpoch, snapshot }
    const queue = this.#terminalCheckpoints.get(sessionId) ?? {}
    this.#terminalCheckpoints.set(sessionId, queue)
    if (queue.inFlight) {
      if (!queue.pending || throughSequence >= queue.pending.throughSequence) {
        queue.pending = checkpoint
      }
      return
    }
    this.#postTerminalCheckpoint(sessionId, queue, checkpoint)
  }

  refreshTerminalHud(sessionId: string): void {
    this.#post({ type: 'terminal.hud-refresh', protocolVersion: PROTOCOL_VERSION, sessionId })
  }

  disposeDeletedTerminal(sessionId: string): void {
    this.#foregroundTerminalSessions.delete(sessionId)
    this.#terminals.delete(sessionId)
    this.#terminalCheckpoints.delete(sessionId)
    this.#terminalResizeIds.delete(sessionId)
    if (this.#readOnly) return
    this.#post({ type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId })
  }

  #bindPort(port: RuntimeClientPort): void {
    port.onmessage = (event) => {
      if (this.#disposed || this.#port !== port) return
      this.#receive(event.data, port)
    }
    port.start()
    port.postMessage({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: this.#clientId,
      ...this.#windowIdentity
    })
  }

  #receive(message: RuntimeMessage, port: RuntimeClientPort): void {
    if (message.type === 'protocol.ready') {
      this.#ready = true
      for (const resolve of this.#readyWaiters) resolve()
      this.#readyWaiters.clear()
      if (!this.#requiresRecoverySnapshot) {
        for (const { config } of this.#terminals.values()) {
          if (this.#recoveryAllowsAttach(config.sessionId)) this.#spawn(config)
        }
      }
      if (this.#projectionAfterSequence !== undefined) {
        this.#subscribeEvents(this.#projectionAfterSequence)
      }
      return
    }
    if (message.type === 'host.navigation-request') {
      const key = hostNavigationAttemptKey(message)
      if (!this.#hostNavigationPorts.has(key) && this.#hostNavigationPorts.size >= 256) {
        const oldest = this.#hostNavigationPorts.keys().next().value as string | undefined
        if (oldest !== undefined) this.#hostNavigationPorts.delete(oldest)
      }
      this.#hostNavigationPorts.set(key, port)
      for (const listener of this.#hostNavigationListeners) listener(message)
      return
    }
    if (message.type === 'session.recovery-snapshot') {
      this.#resetRecoveryStatuses()
      for (const status of message.statuses) {
        const wireStatus: SessionRecoveryStatus = {
          type: 'session.recovery-status', protocolVersion: PROTOCOL_VERSION, ...status
        }
        this.#recoveryStatuses.set(wireStatus.sessionId, wireStatus)
        for (const listener of this.#recoveryListeners) listener(wireStatus)
      }
      this.#requiresRecoverySnapshot = false
      for (const { config } of this.#terminals.values()) {
        if (this.#recoveryAllowsAttach(config.sessionId)) this.#spawn(config)
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
    if (message.type === 'session.recovery-status') {
      const previous = this.#recoveryStatuses.get(message.sessionId)
      this.#recoveryStatuses.set(message.sessionId, message)
      for (const listener of this.#recoveryListeners) listener(message)
      if (
        !this.#requiresRecoverySnapshot && message.state === 'ready' &&
        previous?.state !== 'ready'
      ) {
        const consumer = this.#terminals.get(message.sessionId)
        if (consumer) this.#spawn(consumer.config)
      }
      return
    }
    if (
      message.type === 'terminal.checkpoint-stored' ||
      message.type === 'terminal.checkpoint-rejected'
    ) {
      const queue = this.#terminalCheckpoints.get(message.sessionId)
      if (queue?.inFlight && queue.inFlight.throughSequence <= message.throughSequence) {
        delete queue.inFlight
        const pending = queue.pending
        delete queue.pending
        if (pending) this.#postTerminalCheckpoint(message.sessionId, queue, pending)
        else if (!this.#terminals.has(message.sessionId)) {
          this.#terminalCheckpoints.delete(message.sessionId)
        }
      }
    }
    if ('sessionId' in message && typeof message.sessionId === 'string') {
      const consumer = this.#terminals.get(message.sessionId)
      for (const listener of consumer?.listeners ?? []) {
        listener(message)
      }
      // A virtualized foreground card has no mounted xterm writer. Runtime's
      // append-only Journal remains authoritative, so acknowledge the live
      // stream here to apply backpressure while the final xterm checkpoint and
      // later replay preserve the card's VT state.
      if (message.type === 'terminal.data' && consumer && consumer.listeners.size === 0 &&
        this.#foregroundTerminalSessions.has(message.sessionId)) {
        this.acknowledgeTerminal(message.sessionId, message.sequence)
      }
    }
    if (message.type === 'events.batch' || message.type === 'terminal.hud') {
      for (const listener of this.#projectionListeners) listener(message)
    }
  }

  #spawn(config: TerminalAttachment): void {
    if (config.readOnly) {
      this.requestTerminalReplay(config.sessionId)
      return
    }
    this.#post({ type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, ...config })
  }

  #releaseTerminalView(sessionId: string, consumer: TerminalConsumer): void {
    if (this.#terminals.get(sessionId) !== consumer) return
    this.#terminals.delete(sessionId)
    if (!this.#readOnly) {
      this.#post({
        type: 'terminal.view-detach', protocolVersion: PROTOCOL_VERSION, sessionId
      })
    }
    const queue = this.#terminalCheckpoints.get(sessionId)
    if (!queue?.inFlight && !queue?.pending) this.#terminalCheckpoints.delete(sessionId)
  }

  #recoveryAllowsAttach(sessionId: string): boolean {
    const state = this.#recoveryStatuses.get(sessionId)?.state
    return state === undefined || state === 'ready'
  }

  #resetRecoveryStatuses(): void {
    this.#recoveryStatuses.clear()
    for (const listener of this.#recoveryResetListeners) listener()
  }

  #markTerminalsQueuedForRecovery(): void {
    for (const sessionId of this.#terminals.keys()) {
      const status: SessionRecoveryStatus = {
        type: 'session.recovery-status', protocolVersion: PROTOCOL_VERSION,
        sessionId, sceneId: '',
        priority: this.#foregroundTerminalSessions.has(sessionId)
          ? 'foreground-scene'
          : 'background',
        state: 'queued'
      }
      this.#recoveryStatuses.set(sessionId, status)
      for (const listener of this.#recoveryListeners) listener(status)
    }
  }

  #subscribeEvents(afterSequence: number): void {
    this.#post({
      type: 'events.subscribe', protocolVersion: PROTOCOL_VERSION,
      consumerId: `${this.#clientId}-projection`, afterSequence, batchSize: 250
    })
  }

  #postTerminalCheckpoint(
    sessionId: string,
    queue: TerminalCheckpointQueue,
    checkpoint: TerminalCheckpoint
  ): void {
    queue.inFlight = checkpoint
    this.#post({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId, ...checkpoint
    })
  }

  #post(message: unknown): void {
    this.#port.postMessage(message)
  }
}

function hostNavigationAttemptKey(value: {
  requestId: string
  attemptId: string
}): string {
  return `${value.requestId}\u0000${value.attemptId}`
}

function effectiveTerminalConfig(
  config: TerminalAttachment,
  readOnly: boolean
): TerminalAttachment {
  const { readOnly: _readOnly, ...base } = config
  return readOnly ? { ...base, readOnly: true } : base
}
