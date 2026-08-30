import { MessageChannelMain, utilityProcess, type UtilityProcess, type WebContents } from 'electron'

import { PROTOCOL_VERSION, type RuntimeConnectRequest } from '@matou/contracts'
import { DESKTOP_CHANNELS, type RuntimeConnectionState } from '../shared/desktop-api'

export class RuntimeHost {
  readonly #runtimeEntry: string
  readonly #renderers = new Set<WebContents>()
  #child: UtilityProcess | undefined
  #restartTimer: ReturnType<typeof setTimeout> | undefined
  #recoveryReadyTimer: ReturnType<typeof setTimeout> | undefined
  #stopping = false
  #stopPromise: Promise<void> | undefined
  #connectionState: RuntimeConnectionState = 'ready'

  constructor(runtimeEntry: string) {
    this.#runtimeEntry = runtimeEntry
  }

  async start(): Promise<void> {
    if (this.#child) {
      return
    }
    this.#stopping = false
    await this.#launch()
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

    await new Promise<void>((resolve, reject) => {
      child.once('spawn', resolve)
      child.once('error', (error) => {
        if (this.#child === child) this.#child = undefined
        reject(new Error(`Runtime failed to start: ${error}`))
      })
      child.once('exit', (code) => {
        if (this.#child === child) {
          this.#child = undefined
        }
        if (code !== 0) {
          console.error(`Matou Runtime exited with code ${code}`)
        }
        if (!this.#stopping) {
          this.#markReconnecting()
          this.#scheduleRestart()
        }
      })
    })
  }

  connect(webContents: WebContents): void {
    this.#renderers.add(webContents)
    this.#sendConnectionState(webContents)
    if (!this.#child) return
    this.#connectRenderer(webContents)
  }

  #connectRenderer(webContents: WebContents): void {
    const child = this.#child
    if (!child) {
      throw new Error('Runtime is not running')
    }

    const { port1, port2 } = new MessageChannelMain()
    const request: RuntimeConnectRequest = {
      type: 'runtime.connect',
      protocolVersion: PROTOCOL_VERSION
    }
    child.postMessage(request, [port1])
    webContents.postMessage('matou:terminal-port', { protocolVersion: PROTOCOL_VERSION }, [port2])
  }

  stop(): Promise<void> {
    if (this.#stopPromise) return this.#stopPromise
    this.#stopping = true
    if (this.#restartTimer) clearTimeout(this.#restartTimer)
    if (this.#recoveryReadyTimer) clearTimeout(this.#recoveryReadyTimer)
    this.#restartTimer = undefined
    this.#recoveryReadyTimer = undefined
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

  #scheduleRestart(): void {
    if (this.#restartTimer || this.#stopping) return
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = undefined
      void this.#restartAndReconnect()
    }, 100)
  }

  async #restartAndReconnect(): Promise<void> {
    try {
      await this.#launch()
      for (const renderer of this.#renderers) {
        if (renderer.isDestroyed()) {
          this.#renderers.delete(renderer)
          continue
        }
        this.#connectRenderer(renderer)
      }
      this.#recoveryReadyTimer = setTimeout(() => {
        this.#recoveryReadyTimer = undefined
        this.#setConnectionState('ready')
      }, 1_000)
    } catch (error) {
      console.error('Matou Runtime restart failed', error)
      this.#scheduleRestart()
    }
  }

  #markReconnecting(): void {
    if (this.#recoveryReadyTimer) clearTimeout(this.#recoveryReadyTimer)
    this.#recoveryReadyTimer = undefined
    this.#setConnectionState('reconnecting')
  }

  #setConnectionState(state: RuntimeConnectionState): void {
    this.#connectionState = state
    for (const renderer of this.#renderers) this.#sendConnectionState(renderer)
  }

  #sendConnectionState(webContents: WebContents): void {
    if (webContents.isDestroyed()) return
    webContents.send(DESKTOP_CHANNELS.runtimeConnectionState, this.#connectionState)
  }
}
