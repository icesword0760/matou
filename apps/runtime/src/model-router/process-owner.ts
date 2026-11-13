import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process'
import { access, readFile, realpath } from 'node:fs/promises'
import { isAbsolute, join, relative, sep } from 'node:path'
import { createInterface } from 'node:readline'

export type RouterProcessEvent = { state: 'spawned'; pid: number; restarts: number } | { state: 'failed'; reason: string }
export interface RouterProcessControl {
  start(): Promise<void>
  stop(): Promise<void>
  onEvent(listener: (event: RouterProcessEvent) => void): () => void
}

/** Main-process owner. Resource paths come from main, never from renderer/runtime messages. */
export class RouterProcessOwner implements RouterProcessControl {
  readonly #options: { executable: string; supervisor: string; binary: string; dataRoot: string }
  readonly #listeners = new Set<(event: RouterProcessEvent) => void>()
  #child: ChildProcessWithoutNullStreams | undefined
  #start: Promise<void> | undefined
  #stop: Promise<void> | undefined
  #canonicalDataRoot: string | undefined
  constructor(options: { executable: string; supervisor: string; binary: string; dataRoot: string }) { this.#options = options }
  onEvent(listener: (event: RouterProcessEvent) => void): () => void { this.#listeners.add(listener); return () => this.#listeners.delete(listener) }
  start(): Promise<void> {
    if (this.#stop) return this.#stop.then(() => this.start())
    if (this.#start) return this.#start
    this.#start = this.#launch().finally(() => { this.#start = undefined })
    return this.#start
  }
  async #launch(): Promise<void> {
    if (this.#child) return
    const { executable, supervisor, binary } = this.#options
    const dataRoot = this.#canonicalDataRoot ??= await realpath(this.#options.dataRoot)
    const insideRoot = async (path: string) => {
      const canonical = await realpath(path), child = relative(dataRoot, canonical)
      if (!child || child === '..' || child.startsWith(`..${sep}`) || isAbsolute(child)) throw new Error('Invalid model router directory')
      return canonical
    }
    const directory = await insideRoot(join(dataRoot, 'model-router'))
    const config = await insideRoot(join(directory, 'config.json'))
    const credentials = await insideRoot(join(directory, 'credentials'))
    const value = JSON.parse(await readFile(config, 'utf8')) as { server?: { host?: string; port?: number }; oauth?: { 'auth-dir'?: string } }
    if (value.server?.host !== '127.0.0.1' || !Number.isInteger(value.server.port) || value.server.port! < 1 || value.server.port! > 65535 || typeof value.oauth?.['auth-dir'] !== 'string' || await realpath(value.oauth['auth-dir']) !== credentials) throw new Error('Invalid model router configuration')
    await Promise.all([access(binary), access(supervisor)])
    const child = spawn(executable, [supervisor, binary, config, directory], {
      cwd: directory, stdio: 'pipe', env: { ELECTRON_RUN_AS_NODE: '1', PATH: process.env.PATH ?? '/usr/bin:/bin', LANG: process.env.LANG ?? 'en_US.UTF-8' }
    })
    this.#child = child
    child.stdin.on('error', () => { /* An exited supervisor already released the lease. */ })
    // Raw upstream output is deliberately not forwarded. Its bounded error logs
    // stay in model-router/logs, with mode 600 inherited from supervisor umask.
    child.stderr.resume()
    const lines = createInterface({ input: child.stdout })
    lines.on('line', line => {
      if (line.length > 1024) return
      try {
        const event = JSON.parse(line) as RouterProcessEvent
        if ((event.state === 'spawned' && Number.isSafeInteger(event.pid) && event.pid > 0) || event.state === 'failed') this.#emit(event)
      } catch { /* Ignore malformed child output, readiness remains authenticated. */ }
    })
    child.once('exit', () => {
      lines.close()
      if (this.#child === child) this.#child = undefined
      if (!this.#stop) this.#emit({ state: 'failed', reason: 'supervisor-exited' })
    })
    await new Promise<void>((resolve, reject) => { child.once('spawn', resolve); child.once('error', () => { this.#child = undefined; reject(new Error('Model router supervisor failed to start')) }) })
  }
  stop(): Promise<void> {
    if (this.#stop) return this.#stop
    this.#stop = this.#close().finally(() => { this.#stop = undefined })
    return this.#stop
  }
  async #close(): Promise<void> {
    await this.#start?.catch(() => undefined)
    const child = this.#child
    if (!child) return
    await new Promise<void>(resolve => {
      const timer = setTimeout(() => child.kill('SIGTERM'), 2_000)
      const deadline = setTimeout(() => { child.kill('SIGKILL'); resolve() }, 4_000)
      child.once('exit', () => { clearTimeout(timer); clearTimeout(deadline); resolve() })
      child.stdin.end()
    })
  }
  #emit(event: RouterProcessEvent): void { for (const listener of this.#listeners) listener(event) }
}
