import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'
import type { ClaudeAccountRuntimeAction, ClaudeAccountSession } from '@matou/contracts'
import type { AccountError, ClaudeAccountCommand, ClaudeAccountsState, ClaudeAccountView } from '../../shared/claude-accounts'
import { AccountFault, type AccountEngine, type EngineState } from './engine'

export interface AccountRuntime {
  request(action: ClaudeAccountRuntimeAction, sessionId?: string, runId?: string): Promise<ClaudeAccountSession[]>
}
export interface AccountPreferences { names: Record<string, string>; rememberScope: boolean }
export interface PreferenceStore { load(): Promise<AccountPreferences>; save(value: AccountPreferences): Promise<void> }
/** Presentation preferences only. All credential storage belongs to claude-swap. */
export class AccountPreferenceStore implements PreferenceStore {
  constructor(private readonly path: string) {}
  async load(): Promise<AccountPreferences> {
    try {
      const v = JSON.parse(await readFile(this.path, 'utf8')) as AccountPreferences
      if (!v.names || typeof v.names !== 'object' || Array.isArray(v.names) || Object.values(v.names).some(name => typeof name !== 'string' || !name.trim() || [...name].length > 12) || typeof v.rememberScope !== 'boolean') throw new Error('schema')
      return v
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === 'ENOENT') return { names: {}, rememberScope: false }
      throw new AccountFault('storage')
    }
  }
  async save(value: AccountPreferences) {
    try {
      await mkdir(dirname(this.path), { recursive: true, mode: 0o700 })
      const temp = `${this.path}.tmp`
      await writeFile(temp, JSON.stringify(value), { mode: 0o600 })
      await rename(temp, this.path)
    } catch { throw new AccountFault('storage') }
  }
}
const fault = (e: unknown): AccountError => e instanceof AccountFault ? e.code : 'engine-error'
export class ClaudeAccountsManager {
  private prefs: AccountPreferences = { names: {}, rememberScope: false }
  private load: Promise<void> | undefined
  private loaded = false
  private value: EngineState = { accounts: [] }
  private operation: ClaudeAccountsState['operation'] = 'idle'
  private revision = 0
  private error: AccountError | undefined
  private draft: ClaudeAccountView | undefined
  private nonce: string | undefined
  private loginAbort: AbortController | undefined
  private queuedId: string | undefined
  private timer: ReturnType<typeof setTimeout> | undefined
  private queueFlight: Promise<void> | undefined
  private cleaned = false
  private sessions: ClaudeAccountSession[] = []
  private pending = new Map<string, string>()
  private switchedAt: number | undefined
  private disposed = false
  private inFlight = new Set<Promise<ClaudeAccountsState>>()
  private closing: Promise<void> | undefined
  constructor(private readonly engine: AccountEngine, private readonly store: PreferenceStore,
    private readonly runtime: AccountRuntime, private readonly publish: (state: ClaudeAccountsState) => void) {}
  private named(a: ClaudeAccountView) { return { ...a, name: this.prefs.names[a.id] || a.name } }
  state(): ClaudeAccountsState {
    return { ...this.value, accounts: this.value.accounts.map(a => this.named(a)),
      ...(this.value.detected ? { detected: this.named(this.value.detected) } : {}),
      revision: this.revision, rememberScope: this.prefs.rememberScope,
      operation: this.operation, sessions: this.sessions, pendingSessionIds: [...this.pending.keys()],
      ...(this.draft ? { draft: this.named(this.draft) } : {}), ...(this.error ? { error: this.error } : {}),
      ...(this.queuedId ? { queuedId: this.queuedId } : {}), ...(this.switchedAt ? { switchedAt: this.switchedAt } : {}) }
  }
  private emit() { this.revision++; if (!this.disposed) this.publish(this.state()) }
  async snapshot() {
    this.load ??= this.store.load().then(v => { this.prefs = v; this.loaded = true }).catch(() => { this.error = 'storage'; this.load = undefined })
    await this.load; return this.state()
  }
  private async savePrefs(next: AccountPreferences) { await this.store.save(next); this.prefs = next }
  private async refresh(action: 'snapshot' | 'refresh' = 'snapshot') {
    if (!this.cleaned && !this.nonce) { await this.engine.run({ action: 'cleanup-logins' }); this.cleaned = true }
    this.value = await this.engine.run({ action })
    this.sessions = await this.runtime.request('snapshot')
    for (const [id, run] of this.pending) if (!this.sessions.some(s => s.sessionId === id && s.runId === run)) this.pending.delete(id)
  }
  command(command: ClaudeAccountCommand): Promise<ClaudeAccountsState> {
    const result = this.execute(command)
    this.inFlight.add(result)
    void result.then(() => this.inFlight.delete(result), () => this.inFlight.delete(result))
    return result
  }
  private async execute(command: ClaudeAccountCommand): Promise<ClaudeAccountsState> {
    await this.snapshot()
    if (!this.loaded || this.disposed) return this.state()
    if (command.type === 'cancel-login') {
      this.loginAbort?.abort()
      if (!this.loginAbort && this.nonce) {
        await this.engine.run({ action: 'cancel-login', nonce: this.nonce }).catch(() => {})
        this.nonce = undefined; this.draft = undefined
      }
      this.emit(); return this.state()
    }
    if (command.type === 'cancel-switch') { if (this.operation === 'switching') return this.state(); this.cancelQueue(); this.emit(); return this.state() }
    if (command.type === 'dismiss-error') { this.error = undefined; this.emit(); return this.state() }
    if (this.operation !== 'idle') return this.state()
    this.error = undefined
    this.operation = command.type === 'login' ? 'login' : command.type === 'switch' ? 'switching' : command.type === 'resume' ? 'resuming' : 'loading'
    this.emit()
    try {
      switch (command.type) {
        case 'inspect': case 'recover': await this.refresh(); break
        case 'refresh-usage': await this.refresh('refresh'); break
        case 'login': {
          if (this.nonce) await this.engine.run({ action: 'cancel-login', nonce: this.nonce })
          const nonce = randomUUID(), abort = new AbortController()
          this.nonce = nonce; this.draft = undefined; this.loginAbort = abort
          this.emit() // Clear the previous identity before a repeated sign-in completes.
          try {
            const result = await this.engine.run({ action: 'login', nonce, replaceId: command.replaceId }, abort.signal)
            if (!abort.signal.aborted) this.draft = result.draft
          } catch (e) {
            await this.engine.run({ action: 'cancel-login', nonce }).catch(() => {})
            this.nonce = undefined
            if (!abort.signal.aborted) throw e
          } finally { this.loginAbort = undefined }
          break
        }
        case 'save-detected': case 'save-draft': {
          const draft = command.type === 'save-draft' ? this.draft : this.value.detected
          if (!draft) throw new AccountFault('conflict')
          this.value = await this.engine.run({ action: command.type === 'save-draft' ? 'save-draft' : 'save-current', id: draft.id, nonce: this.nonce })
          if (command.type === 'save-draft') { this.nonce = undefined; this.draft = undefined }
          await this.savePrefs({ ...this.prefs, names: { ...this.prefs.names, [draft.id]: command.name.trim() } })
          break
        }
        case 'rename': {
          if (!this.value.accounts.some(a => a.id === command.id)) throw new AccountFault('conflict')
          await this.savePrefs({ ...this.prefs, names: { ...this.prefs.names, [command.id]: command.name.trim() } }); break
        }
        case 'remove': {
          this.value = await this.engine.run({ action: 'remove', id: command.id })
          const { [command.id]: _, ...names } = this.prefs.names; await this.savePrefs({ ...this.prefs, names })
          if (this.queuedId === command.id) this.cancelQueue()
          break
        }
        case 'switch': {
          this.sessions = await this.runtime.request('snapshot')
          if (this.sessions.some(s => s.busy)) {
            if (!command.waitForIdle) throw new AccountFault('busy')
            this.queuedId = command.id; this.schedule()
          } else await this.switchTo(command.id)
          await this.savePrefs({ ...this.prefs, rememberScope: command.rememberScope })
          break
        }
        case 'resume': {
          if (this.pending.get(command.sessionId) !== command.runId) throw new AccountFault('conflict')
          const check = await this.engine.run({ action: 'snapshot' })
          if (check.currentId !== this.value.currentId) { this.value = check; throw new AccountFault('conflict') }
          try { this.sessions = await this.runtime.request('resume', command.sessionId, command.runId) }
          catch { throw new AccountFault('resume-failed') }
          this.pending.delete(command.sessionId); break
        }
      }
    } catch (e) { this.error = fault(e) }
    finally { this.operation = 'idle'; this.emit() }
    return this.state()
  }
  private async switchTo(id: string) {
    let held = false
    try {
      this.sessions = await this.runtime.request('hold'); held = true
      if (this.sessions.some(s => s.busy)) throw new AccountFault('busy')
      this.value = await this.engine.run({ action: 'switch', id })
      if (this.value.currentId !== id) throw new AccountFault('switch-failed')
      this.pending = new Map(this.sessions.filter(s => s.defaultLogin).map(s => [s.sessionId, s.runId]))
      this.switchedAt = Date.now(); this.cancelQueue()
    } finally { if (held) await this.runtime.request('release').catch(() => {}) }
  }
  private schedule() {
    if (this.timer) clearTimeout(this.timer)
    this.timer = setTimeout(() => { this.queueFlight = this.tick() }, 2_000)
  }
  private async tick() {
    if (!this.queuedId || this.disposed) return
    if (this.operation !== 'idle') { this.schedule(); return }
    this.operation = 'loading'
    const target = this.queuedId
    try {
      this.sessions = await this.runtime.request('snapshot')
      if (!this.disposed && this.queuedId === target && !this.sessions.some(s => s.busy)) { this.operation = 'switching'; this.emit(); await this.switchTo(target) }
    } catch (e) { this.error = fault(e); this.cancelQueue() }
    finally { this.operation = 'idle'; this.emit(); if (this.queuedId) this.schedule() }
  }
  private cancelQueue() { this.queuedId = undefined; if (this.timer) clearTimeout(this.timer) }
  dispose(): Promise<void> {
    this.closing ??= (async () => {
      this.disposed = true; this.cancelQueue(); this.loginAbort?.abort()
      await Promise.allSettled([...this.inFlight, this.queueFlight])
      if (this.nonce) await this.engine.run({ action: 'cancel-login', nonce: this.nonce }).catch(() => {})
    })()
    return this.closing
  }
}
