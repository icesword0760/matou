import { randomUUID } from 'node:crypto'
import { createServer } from 'node:net'
import { join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { resolveAntigravityClientUserAgent } from './antigravity-client'
import { resolveCodexClientIdentity, type CodexClientIdentity } from './codex-client'
import { prepareRouterDirectory, ROUTER_VERSION, writeRouterConfig, type RouterKeys, type RouterModelAliases } from './config'
import type { RouterProcessControl } from './process-owner'
import { appendRouterLifecycleLog } from './lifecycle-log'
import { resolveRouterProxy, type RouterProxyResolution } from './network-proxy'

export interface RouterConnection {
  baseUrl: string
  clientKey: string
  managementKey: string
  pid: number
  codexClient?: CodexClientIdentity
  antigravityUserAgent?: string
}
export interface RouterLease { connection: RouterConnection; release(): void }
export interface RouterCredentialTarget { name: string; provider: 'codex' | 'antigravity'; authIndex?: string }
export interface AdoptedRouterCredential { name: string; provider: 'codex' | 'antigravity'; authIndex: string; status: 'active' }
export type RouterRuntimeState = 'running' | 'restarting' | 'failed'
interface ManagerOptions { dataRoot: string; control: RouterProcessControl; idleMs?: number; resolveProxy?(): Promise<RouterProxyResolution> }
/** One instance per runtime, shared by all windows and sessions. No quota polling. */
export class ModelRouterManager {
  readonly #options: ManagerOptions
  readonly #lifetime = new AbortController()
  readonly #modelAliases: RouterModelAliases = {}
  readonly #stateListeners = new Set<(state: RouterRuntimeState) => void>()
  readonly #leases = new Map<string, string>()
  #starting: Promise<RouterConnection> | undefined
  #stopping: Promise<void> | undefined
  #connection: RouterConnection | undefined
  #pid: number | undefined
  #lastPort: number | undefined
  #restarts = 0
  #logPath: string | undefined
  #logWrites = Promise.resolve()
  #failed = false
  #disposed = false
  #idle: ReturnType<typeof setTimeout> | undefined
  readonly #unsubscribe: () => void
  constructor(options: ManagerOptions) {
    this.#options = options
    this.#unsubscribe = options.control.onEvent(event => {
      if (event.state === 'spawned') {
        this.#pid = event.pid
        this.#restarts = event.restarts
        if (this.#connection) this.#connection = { ...this.#connection, pid: event.pid }
        if (event.restarts > 0) {
          this.#log('restart', { pid: event.pid, restarts: event.restarts, reason: event.reason })
          this.#emitState('restarting')
          const connection = this.#connection
          if (connection) {
            const port = Number(new URL(connection.baseUrl).port)
            void this.#waitReady(port, { client: connection.clientKey, management: connection.managementKey }).then(() => {
              if (!this.#disposed && this.#pid === event.pid) this.#emitState('running')
            }, reason => {
              if (this.#disposed || this.#pid !== event.pid) return
              this.#log('failed', { reason: failureMessage(reason), port })
              this.#emitState('failed')
            })
          }
        }
      } else {
        this.#log('failed', { reason: event.reason, pid: this.#pid, restarts: this.#restarts })
        this.#failed = true; this.#pid = undefined; this.#connection = undefined
        this.#emitState('failed')
      }
    })
  }
  onStateChange(listener: (state: RouterRuntimeState) => void): () => void {
    this.#stateListeners.add(listener)
    return () => this.#stateListeners.delete(listener)
  }
  #emitState(state: RouterRuntimeState): void { for (const listener of this.#stateListeners) listener(state) }
  #log(event: string, details: object = {}): void {
    const path = this.#logPath
    if (path) this.#logWrites = this.#logWrites.then(() => appendRouterLifecycleLog(path, { event, ...details })).catch(() => {})
  }
  get status(): { state: 'stopped' | 'starting' | 'running' | 'stopping' | 'failed'; leases: number; pid?: number } {
    return { state: this.#stopping ? 'stopping' : this.#failed ? 'failed' : this.#starting ? 'starting' : this.#connection ? 'running' : 'stopped', leases: this.#leases.size, ...(this.#pid ? { pid: this.#pid } : {}) }
  }
  /** UI binding is deliberately deferred. RPC close calls releaseConnection. */
  openSettings(connectionId: string): Promise<RouterLease> { return this.acquire(`settings:${connectionId}`) }
  /** Rebuilt from committed provider data on runtime startup; no extra storage. */
  rememberModelAliases(provider: 'codex' | 'antigravity', aliases: NonNullable<RouterModelAliases['codex']>): void {
    this.#modelAliases[provider] = aliases.map(alias => ({ ...alias }))
  }
  bindSettingsConnection(port: { once(event: 'close', listener: () => void): unknown }): { open(): Promise<RouterLease>; close(): void } {
    const id = randomUUID()
    let closed = false
    port.once('close', () => { closed = true; this.releaseConnection(id) })
    return {
      open: () => closed ? Promise.reject(new Error('Settings connection is closed')) : this.openSettings(id),
      close: () => this.releaseConnection(id)
    }
  }
  releaseConnection(connectionId: string): void {
    for (const [id, scope] of this.#leases) if (scope === `settings:${connectionId}`) this.#leases.delete(id)
    this.#scheduleIdle()
  }
  async acquire(scope: string): Promise<RouterLease> {
    if (this.#disposed) throw new Error('Model router manager is closed')
    if (this.#idle) clearTimeout(this.#idle)
    this.#idle = undefined
    const id = randomUUID()
    this.#leases.set(id, scope)
    try {
      await this.#stopping
      if (this.#disposed || !this.#leases.has(id)) throw new Error('Model router lease was released')
      const recovering = scope === 'request-recovery'
      if (!this.#connection || recovering) {
        this.#starting ??= (recovering ? this.#recover() : this.#start()).finally(() => { this.#starting = undefined })
      }
      const connection = this.#starting ? await this.#starting : this.#connection!
      if (this.#disposed || !this.#leases.has(id)) throw new Error('Model router lease was released')
      return { connection, release: () => { this.#leases.delete(id); this.#scheduleIdle() } }
    } catch (error) { this.#leases.delete(id); this.#scheduleIdle(); throw error }
  }
  async #recover(): Promise<RouterConnection> {
    const previousPort = this.#lastPort
    this.#log('recovery-started', { port: previousPort })
    try {
      const existing = this.#connection
      if (existing && previousPort !== undefined) {
        // A refused request can arrive during the supervisor's restart backoff,
        // while the cached connection still points to the right port.
        try {
          await this.#waitReady(previousPort, { client: existing.clientKey, management: existing.managementKey })
        } catch {
          if (this.#connection) {
            this.#connection = undefined
            await this.#options.control.stop()
          }
        }
      }
      if (this.#disposed) throw new Error('Model router manager is closed')
      const connection = this.#connection ?? await this.#start()
      // The previous port is preferred; when it is taken, gateways follow the
      // returned connection instead of failing every running session.
      this.#log('recovery-succeeded', { port: this.#lastPort, previousPort, pid: connection.pid })
      return connection
    } catch (error) {
      this.#log('recovery-failed', { port: previousPort, reason: failureMessage(error) })
      throw error
    }
  }
  async #start(): Promise<RouterConnection> {
    const { directory, keys } = await prepareRouterDirectory(this.#options.dataRoot)
    this.#logPath = join(directory, 'logs', 'lifecycle.log')
    const [{ proxyUrl }, codexClient, antigravityUserAgent] = await Promise.all([
      (this.#options.resolveProxy ?? resolveRouterProxy)(),
      resolveCodexClientIdentity(),
      resolveAntigravityClientUserAgent()
    ])
    for (let attempt = 0; attempt < 3 && !this.#disposed; attempt++) {
      let port = attempt === 0 ? this.#lastPort : undefined
      this.#pid = undefined
      this.#restarts = 0
      this.#failed = false
      let startRequested = false
      try {
        port = await availableLoopbackPort(port)
        await writeRouterConfig(directory, port, keys, proxyUrl, this.#modelAliases, codexClient.userAgent)
        if (this.#disposed) break
        startRequested = true
        await this.#options.control.start()
        await this.#waitReady(port, keys)
        const connection: RouterConnection = {
          baseUrl: `http://127.0.0.1:${port}`,
          clientKey: keys.client,
          managementKey: keys.management,
          pid: this.#pid!,
          codexClient,
          antigravityUserAgent
        }
        this.#lastPort = port
        this.#connection = connection
        this.#log('started', { pid: connection.pid, port, restarts: this.#restarts })
        this.#emitState('running')
        return connection
      } catch (error) {
        this.#log('failed', { port, reason: failureMessage(error), attempt: attempt + 1 })
        if (startRequested) await this.#options.control.stop()
      }
    }
    this.#failed = true
    throw new Error('Model router failed to start')
  }
  async #waitReady(port: number, keys: RouterKeys): Promise<void> {
    const until = Date.now() + 8_000
    while (Date.now() < until && !this.#disposed && !this.#failed) {
      const pid = this.#pid
      if (pid) {
        try {
          process.kill(pid, 0)
          const response = await fetch(`http://127.0.0.1:${port}/v8/management/credentials`, { headers: { Authorization: `Bearer ${keys.management}` }, signal: AbortSignal.timeout(500) })
          const version = response.headers.get('x-cpa-version')
          await response.body?.cancel()
          if (response.ok && version === ROUTER_VERSION && this.#pid === pid && !this.#disposed && !this.#failed) {
            process.kill(pid, 0)
            return
          }
        } catch { /* The router may be in backoff or still binding its socket. */ }
      }
      await delay(25)
    }
    throw new Error('Model router readiness failed')
  }
  /** HTTP readiness is not credential adoption. Login/save/switch callers must
   * wait for the specific credential in the runtime auth manager to be active.
   * This polls only the local management API, never provider quota endpoints.
   */
  async waitForCredentialAdoption(
    target: RouterCredentialTarget,
    options: { timeoutMs?: number; signal?: AbortSignal } = {}
  ): Promise<AdoptedRouterCredential> {
    const timeoutMs = options.timeoutMs ?? 8_000
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) throw new Error('Invalid credential adoption deadline')
    const signal = AbortSignal.any([this.#lifetime.signal, ...(options.signal ? [options.signal] : [])])
    signal.throwIfAborted()
    const lease = await this.acquire('credential-adoption')
    const deadline = Date.now() + timeoutMs
    try {
      const query = new URLSearchParams({ name: target.name, ...(target.authIndex ? { auth_index: target.authIndex } : {}) })
      while (Date.now() < deadline && !this.#failed) {
        signal.throwIfAborted()
        try {
          const response = await fetch(`${lease.connection.baseUrl}/v8/management/credentials?${query}`, {
            headers: { Authorization: `Bearer ${lease.connection.managementKey}` },
            signal: AbortSignal.any([signal, AbortSignal.timeout(Math.min(500, Math.max(1, deadline - Date.now())))])
          })
          if (response.ok) {
            const body = await response.json() as { files?: Array<Record<string, unknown>> }
            const file = body.files?.find(value => value.name === target.name && value.provider === target.provider &&
              (!target.authIndex || value.auth_index === target.authIndex))
            if (file?.status === 'active' && file.disabled !== true && file.requires_sign_in !== true &&
                typeof file.auth_index === 'string' && file.auth_index.length > 0) {
              return { name: target.name, provider: target.provider, authIndex: file.auth_index, status: 'active' }
            }
          } else await response.body?.cancel()
        } catch { /* Watcher adoption/restart can temporarily leave no active entry. */ }
        signal.throwIfAborted()
        await delay(Math.min(50, Math.max(1, deadline - Date.now())), undefined, { signal })
      }
      throw new Error('Model router credential was not adopted before the deadline')
    } finally { lease.release() }
  }
  #scheduleIdle(): void {
    if (this.#leases.size || this.#disposed || this.#idle) return
    this.#idle = setTimeout(() => { this.#idle = undefined; void this.#stop().catch(() => { this.#failed = true }) }, this.#options.idleMs ?? 1_000)
    this.#idle.unref()
  }
  #stop(): Promise<void> {
    if (this.#stopping) return this.#stopping
    this.#stopping = (async () => {
      await this.#starting?.catch(() => undefined)
      this.#connection = undefined
      await this.#options.control.stop()
      this.#pid = undefined
    })().finally(() => { this.#stopping = undefined })
    return this.#stopping
  }
  async close(): Promise<void> {
    this.#disposed = true
    this.#lifetime.abort(new Error('Model router manager is closed'))
    if (this.#idle) clearTimeout(this.#idle)
    this.#leases.clear()
    await this.#stop()
    this.#unsubscribe()
    this.#stateListeners.clear()
  }
}

function failureMessage(error: unknown): string {
  return error instanceof Error ? error.message.slice(0, 1024) : 'Unknown model router error'
}

async function availableLoopbackPort(port = 0): Promise<number> {
  const server = createServer()
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(port, '127.0.0.1', resolve) })
  const address = server.address()
  const boundPort = typeof address === 'object' && address ? address.port : 0
  await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()))
  if (!boundPort) throw new Error('Cannot allocate model router port')
  return boundPort
}
