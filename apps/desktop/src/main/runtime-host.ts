import { randomUUID } from 'node:crypto'

import { MessageChannelMain, utilityProcess, type UtilityProcess, type WebContents } from 'electron'

import {
  PROTOCOL_VERSION,
  parseRuntimeLifecycleEvent,
  type RuntimeConnectRequest,
  type RuntimeLifecycleEvent,
  type RuntimeRecoveryCommand
} from '@matou/contracts'
import {
  DESKTOP_CHANNELS,
  type RuntimeConnectionState,
  type RuntimeLifecyclePresentation,
  type RuntimeRecoveryCommandResult,
  type RuntimeRecoveryDetails
} from '../shared/desktop-api'

const RESTART_DELAYS = [100, 500, 1_000, 2_000, 5_000] as const

type RuntimeChildMessage = RuntimeLifecycleEvent | {
  type: 'runtime.recovery-details'
  recovery: RuntimeRecoveryDetails
} | {
  type: 'runtime.recovery-result'
  requestId: string
  ok: boolean
  value?: RuntimeRecoveryCommandResult
  error?: string
} | {
  type: 'runtime.scale-metrics-result'
  requestId: string
  runtimePid: number
  ptyCount: number
  ptyPids: number[]
  statementCount: number
}

interface PendingRecoveryCommand {
  resolve(value: RuntimeRecoveryCommandResult): void
  reject(error: Error): void
}

export interface RuntimeScaleMetrics {
  runtimePid: number
  ptyCount: number
  ptyPids: number[]
  statementCount: number
}

interface PendingScaleMetrics {
  resolve(value: RuntimeScaleMetrics): void
  reject(error: Error): void
}

export class RuntimeHost {
  readonly #runtimeEntry: string
  readonly #renderers = new Set<WebContents>()
  readonly #connectedRenderers = new Set<WebContents>()
  readonly #pendingRecoveryCommands = new Map<string, PendingRecoveryCommand>()
  readonly #pendingScaleMetrics = new Map<string, PendingScaleMetrics>()
  #child: UtilityProcess | undefined
  #restartTimer: ReturnType<typeof setTimeout> | undefined
  #restartAttempt = 0
  #stopping = false
  #stopPromise: Promise<void> | undefined
  #connectionState: RuntimeConnectionState = 'reconnecting'
  #lifecycle: RuntimeLifecyclePresentation = {
    snapshot: {
      recoveryId: `desktop-${randomUUID()}`,
      revision: 0,
      mode: 'normal',
      stage: 'opening-database',
      completed: 0,
      total: 1,
      failures: []
    }
  }

  constructor(runtimeEntry: string) {
    this.#runtimeEntry = runtimeEntry
  }

  async start(): Promise<void> {
    if (this.#child) return
    this.#stopping = false
    try {
      await this.#launch()
    } catch (error) {
      console.error('Matou Runtime failed to start', error)
      this.#markReconnecting()
      this.#scheduleRestart()
    }
  }

  getLifecycle(): RuntimeLifecyclePresentation {
    return this.#lifecycle
  }

  recover(command: RuntimeRecoveryCommand): Promise<RuntimeRecoveryCommandResult> {
    const child = this.#child
    if (!child) return Promise.reject(new Error('Runtime is not running'))
    if (
      command.action !== 'export-recovery-bundle' &&
      command.expectedRecoveryId !== this.#lifecycle.recovery?.recoveryId
    ) {
      return Promise.reject(new Error('数据库恢复周期已更新，本次操作已停止'))
    }
    if (this.#pendingRecoveryCommands.size > 0) {
      return Promise.reject(new Error('Recovery command is already running'))
    }
    this.#lifecycle = {
      ...this.#lifecycle,
      operation: { requestId: command.requestId, action: command.action, pending: true }
    }
    this.#publishLifecycle()
    const result = new Promise<RuntimeRecoveryCommandResult>((resolve, reject) => {
      this.#pendingRecoveryCommands.set(command.requestId, { resolve, reject })
    })
    child.postMessage(command)
    return result
  }

  getScaleMetrics(options: { resetStatementCount?: boolean } = {}): Promise<RuntimeScaleMetrics> {
    const child = this.#child
    if (!child) return Promise.reject(new Error('Runtime is not running'))
    const requestId = randomUUID()
    const result = new Promise<RuntimeScaleMetrics>((resolve, reject) => {
      this.#pendingScaleMetrics.set(requestId, { resolve, reject })
    })
    child.postMessage({
      type: 'runtime.scale-metrics-request',
      requestId,
      resetStatementCount: options.resetStatementCount ?? false
    })
    return result
  }

  async #launch(): Promise<void> {
    const child = utilityProcess.fork(this.#runtimeEntry, [], {
      serviceName: 'Matou Terminal Runtime',
      stdio: 'pipe',
      env: { ...process.env }
    })
    child.stdout?.pipe(process.stdout)
    child.stderr?.pipe(process.stderr)
    this.#child = child
    this.#connectedRenderers.clear()
    child.on('message', (message: unknown) => this.#receive(message))

    await new Promise<void>((resolve, reject) => {
      child.once('spawn', resolve)
      child.once('error', (error) => {
        if (this.#child === child) this.#child = undefined
        reject(new Error(`Runtime failed to start: ${error}`))
      })
      child.once('exit', (code) => {
        if (this.#child !== child) return
        this.#child = undefined
        this.#connectedRenderers.clear()
        if (code !== 0) console.error(`Matou Runtime exited with code ${code}`)
        this.#rejectPending(
          new Error('数据库恢复操作未完成：Runtime 在恢复操作期间退出'),
          true
        )
        this.#rejectScaleMetrics(new Error('Runtime exited during scale measurement'))
        if (!this.#stopping) {
          this.#markReconnecting()
          this.#scheduleRestart()
        }
      })
    })
  }

  connect(webContents: WebContents): void {
    this.#renderers.add(webContents)
    // did-finish-load is emitted again after a Renderer reload. The previously
    // transferred port belongs to the old document and is no longer reachable,
    // so this load must receive a fresh direct Runtime channel.
    this.#connectedRenderers.delete(webContents)
    this.#sendConnectionState(webContents)
    this.#sendLifecycle(webContents)
    if (this.#isRuntimeReady()) this.#connectRenderer(webContents)
  }

  #connectRenderer(webContents: WebContents): void {
    const child = this.#child
    if (!child || webContents.isDestroyed() || this.#connectedRenderers.has(webContents)) return
    const { port1, port2 } = new MessageChannelMain()
    const request: RuntimeConnectRequest = {
      type: 'runtime.connect',
      protocolVersion: PROTOCOL_VERSION
    }
    child.postMessage(request, [port1])
    webContents.postMessage('matou:terminal-port', { protocolVersion: PROTOCOL_VERSION }, [port2])
    this.#connectedRenderers.add(webContents)
  }

  stop(): Promise<void> {
    if (this.#stopPromise) return this.#stopPromise
    this.#stopping = true
    if (this.#restartTimer) clearTimeout(this.#restartTimer)
    this.#restartTimer = undefined
    this.#rejectPending(new Error('Runtime stopped during database recovery'))
    this.#rejectScaleMetrics(new Error('Runtime stopped during scale measurement'))
    const child = this.#child
    if (!child) return Promise.resolve()
    this.#stopPromise = new Promise<void>((resolve) => {
      child.once('exit', () => resolve())
      child.kill()
    }).finally(() => {
      if (this.#child === child) this.#child = undefined
    })
    return this.#stopPromise
  }

  #receive(message: unknown): void {
    if (!message || typeof message !== 'object' || !('type' in message)) return
    const candidate = message as RuntimeChildMessage
    if (candidate.type === 'runtime.lifecycle') {
      let event: RuntimeLifecycleEvent
      try {
        event = parseRuntimeLifecycleEvent(candidate, this.#lifecycle.snapshot)
      } catch {
        return
      }
      if (event.snapshot.mode !== 'recovery-required' && event.snapshot.stage === 'ready') {
        const { recovery: _recovery, operation: _operation, ...current } = this.#lifecycle
        this.#lifecycle = { ...current, snapshot: event.snapshot }
      } else {
        this.#lifecycle = { ...this.#lifecycle, snapshot: event.snapshot }
      }
      this.#publishLifecycle()
      if (event.snapshot.stage === 'ready') {
        this.#restartAttempt = 0
        this.#setConnectionState('ready')
        for (const renderer of this.#liveRenderers()) this.#connectRenderer(renderer)
      }
      return
    }
    if (candidate.type === 'runtime.recovery-details') {
      this.#lifecycle = { ...this.#lifecycle, recovery: candidate.recovery }
      this.#publishLifecycle()
      return
    }
    if (candidate.type === 'runtime.scale-metrics-result') {
      const pending = this.#pendingScaleMetrics.get(candidate.requestId)
      if (!pending) return
      this.#pendingScaleMetrics.delete(candidate.requestId)
      pending.resolve({
        runtimePid: candidate.runtimePid,
        ptyCount: candidate.ptyCount,
        ptyPids: candidate.ptyPids,
        statementCount: candidate.statementCount
      })
      return
    }
    if (candidate.type !== 'runtime.recovery-result') return
    const pending = this.#pendingRecoveryCommands.get(candidate.requestId)
    if (!pending) return
    this.#pendingRecoveryCommands.delete(candidate.requestId)
    if (candidate.ok) {
      const value = candidate.value ?? {}
      const { operation: _operation, ...current } = this.#lifecycle
      this.#lifecycle = current
      this.#publishLifecycle()
      pending.resolve(value)
    } else {
      const error = candidate.error || '数据库恢复操作失败'
      this.#lifecycle = {
        ...this.#lifecycle,
        operation: { ...this.#lifecycle.operation!, pending: false, error }
      }
      this.#publishLifecycle()
      pending.reject(new Error(error))
    }
  }

  #scheduleRestart(): void {
    if (this.#restartTimer || this.#stopping) return
    const delay = RESTART_DELAYS[Math.min(this.#restartAttempt, RESTART_DELAYS.length - 1)]!
    this.#restartAttempt += 1
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = undefined
      void this.#restart()
    }, delay)
  }

  async #restart(): Promise<void> {
    try {
      await this.#launch()
    } catch (error) {
      console.error('Matou Runtime restart failed', error)
      this.#markReconnecting()
      this.#scheduleRestart()
    }
  }

  #markReconnecting(): void {
    this.#setConnectionState('reconnecting')
    this.#lifecycle = {
      ...this.#lifecycle,
      snapshot: {
        recoveryId: `desktop-${randomUUID()}`,
        revision: 0,
        mode: 'normal',
        stage: 'opening-database',
        completed: 0,
        total: 1,
        failures: []
      }
    }
    this.#publishLifecycle()
  }

  #isRuntimeReady(): boolean {
    return Boolean(this.#child && this.#lifecycle.snapshot.stage === 'ready')
  }

  #setConnectionState(state: RuntimeConnectionState): void {
    this.#connectionState = state
    for (const renderer of this.#liveRenderers()) this.#sendConnectionState(renderer)
  }

  #publishLifecycle(): void {
    for (const renderer of this.#liveRenderers()) this.#sendLifecycle(renderer)
  }

  #sendConnectionState(webContents: WebContents): void {
    if (!webContents.isDestroyed()) {
      webContents.send(DESKTOP_CHANNELS.runtimeConnectionState, this.#connectionState)
    }
  }

  #sendLifecycle(webContents: WebContents): void {
    if (!webContents.isDestroyed()) {
      webContents.send(DESKTOP_CHANNELS.runtimeLifecycle, this.#lifecycle)
    }
  }

  *#liveRenderers(): Generator<WebContents> {
    for (const renderer of this.#renderers) {
      if (renderer.isDestroyed()) {
        this.#renderers.delete(renderer)
        this.#connectedRenderers.delete(renderer)
      } else {
        yield renderer
      }
    }
  }

  #rejectPending(error: Error, retainFailure = false): void {
    if (retainFailure && this.#lifecycle.recovery && this.#lifecycle.operation?.pending) {
      this.#lifecycle = {
        ...this.#lifecycle,
        operation: { ...this.#lifecycle.operation, pending: false, error: error.message }
      }
    }
    for (const pending of this.#pendingRecoveryCommands.values()) pending.reject(error)
    this.#pendingRecoveryCommands.clear()
  }

  #rejectScaleMetrics(error: Error): void {
    for (const pending of this.#pendingScaleMetrics.values()) pending.reject(error)
    this.#pendingScaleMetrics.clear()
  }
}
