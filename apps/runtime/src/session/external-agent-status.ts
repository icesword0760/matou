import { watch, readdirSync, readFileSync, type FSWatcher } from 'node:fs'
import { join } from 'node:path'
import type { HudAgentActivity } from '@matou/contracts'

export const STALE_AGENT_DETAIL = '状态已停止更新'
export const INTERRUPTED_AGENT_DETAIL = '监工进程已退出，任务中断'
export function sortAgents(agents: HudAgentActivity[]): HudAgentActivity[] {
  const rank = { error: 0, warning: 1, waiting: 2, running: 3 }
  return agents.sort((a, b) => rank[a.state] - rank[b.state] ||
    (a.startedAt ?? Infinity) - (b.startedAt ?? Infinity) || a.id.localeCompare(b.id))
}
interface ExternalStatus {
  activity: HudAgentActivity
  state: HudAgentActivity['state'] | 'completed' | 'stopped'
  updatedAt: number
  pid?: number
}
const line = (value: string, limit: number) => value.split(/\r?\n/)[0]!.trim().slice(0, limit)
function time(value: unknown): number | undefined {
  const parsed = typeof value === 'number' ? value : typeof value === 'string' ? Date.parse(value) : NaN
  return Number.isFinite(parsed) ? parsed : undefined
}
function parse(filename: string, value: unknown): ExternalStatus | undefined {
  if (!value || typeof value !== 'object') return
  const data = value as Record<string, unknown>
  const startedAt = time(data.startedAt)
  const updatedAt = time(data.updatedAt)
  if (data.version !== 1 || typeof data.label !== 'string' || !line(data.label, 60) ||
    typeof data.state !== 'string' || !['running', 'waiting', 'warning', 'error', 'completed', 'stopped'].includes(data.state) ||
    startedAt === undefined || updatedAt === undefined || (data.detail !== undefined && typeof data.detail !== 'string')) return
  return {
    activity: { id: `external:${filename.slice(0, -5)}`, label: line(data.label, 60), source: 'external',
      state: data.state as HudAgentActivity['state'], startedAt,
      ...(typeof data.detail === 'string' ? { detail: line(data.detail, 120) } : {}) },
    state: data.state as ExternalStatus['state'], updatedAt,
    ...(typeof data.pid === 'number' && Number.isInteger(data.pid) && data.pid > 0 ? { pid: data.pid } : {})
  }
}

function isProcessAlive(pid: number): boolean {
  try { process.kill(pid, 0); return true }
  catch (error) { return (error as NodeJS.ErrnoException).code === 'EPERM' }
}

/** A directory reader retains the last valid value only while its file still exists. */
export class ExternalAgentStatusReader {
  readonly #valid = new Map<string, ExternalStatus>()
  readonly #interrupted = new Map<string, { pid: number; detectedAt: number }>()
  constructor(readonly directory: string, readonly now: () => number = Date.now,
    readonly alive: (pid: number) => boolean = isProcessAlive) {}
  read(): HudAgentActivity[] {
    let files: string[]
    try { files = readdirSync(this.directory).filter(name => /^[A-Za-z0-9._-]{1,80}\.json$/.test(name)) }
    catch { return this.#snapshot() }
    for (const name of this.#valid.keys()) if (!files.includes(name)) {
      this.#valid.delete(name)
      this.#interrupted.delete(name)
    }
    for (const name of files.sort()) {
      try {
        const valid = parse(name, JSON.parse(readFileSync(join(this.directory, name), 'utf8')))
        if (valid) this.#valid.set(name, valid)
      } catch { /* Keep the previous valid heartbeat during incomplete writes. */ }
    }
    return this.#snapshot()
  }
  #snapshot(): HudAgentActivity[] {
    return sortAgents([...this.#valid.entries()].flatMap(([name, { activity, state, updatedAt, pid }]) => {
      if (this.#interrupted.get(name)?.pid !== pid) this.#interrupted.delete(name)
      if (pid !== undefined && (state === 'running' || state === 'waiting' || state === 'warning') && !this.alive(pid)) {
        const interrupted = this.#interrupted.get(name) ?? { pid, detectedAt: this.now() }
        this.#interrupted.set(name, interrupted)
        if (this.now() - interrupted.detectedAt >= 300_000) return []
        return [{ ...activity, state: 'error' as const, detail: INTERRUPTED_AGENT_DETAIL }]
      }
      this.#interrupted.delete(name)
      const age = this.now() - updatedAt
      if (state === 'completed' || state === 'stopped' || (state === 'error' && age >= 300_000)) return []
      return [{ ...activity, ...(state !== 'error' && age > 60_000 ? { state: 'error' as const, detail: STALE_AGENT_DETAIL } : {}) }]
    }))
  }
}

export class ExternalAgentStatusMonitor {
  readonly #reader: ExternalAgentStatusReader
  #watcher: FSWatcher | undefined
  #timer: ReturnType<typeof setInterval> | undefined
  #last = ''
  constructor(directory: string, readonly changed: (agents: HudAgentActivity[]) => void,
    now: () => number = Date.now, readonly tick: () => void = () => {}) {
    this.#reader = new ExternalAgentStatusReader(directory, now)
  }
  start(): void {
    if (this.#timer) return
    this.#timer = setInterval(() => this.refresh(), 2_000)
    this.#timer.unref()
    this.refresh()
  }
  refresh(): void {
    if (!this.#watcher) {
      try {
        this.#watcher = watch(this.#reader.directory, () => { if (this.#timer) this.refresh() })
        this.#watcher.on('error', () => { this.#watcher?.close(); this.#watcher = undefined })
      } catch { /* Polling retries when the directory becomes available. */ }
    }
    const agents = this.#reader.read()
    const next = JSON.stringify(agents)
    if (next !== this.#last) { this.#last = next; this.changed(agents) }
    this.tick()
  }
  stop(): void {
    clearInterval(this.#timer)
    this.#timer = undefined
    this.#watcher?.close()
    this.#watcher = undefined
  }
}
