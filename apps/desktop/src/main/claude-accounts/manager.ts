import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'
import type { ClaudeAccountRuntimeAction, ClaudeAccountSession } from '@matou/contracts'
import { validLoginCode } from '../../shared/claude-accounts'
import type { AccountError, ClaudeAccountCommand, ClaudeAccountsState, ClaudeAccountView } from '../../shared/claude-accounts'
import { AccountFault, type AccountEngine, type EngineState } from './engine'

export interface AccountRuntime {
  request(action: ClaudeAccountRuntimeAction, sessionId?: string, runId?: string): Promise<ClaudeAccountSession[]>
}
export interface AccountPreferences { names: Record<string, string>; rememberScope: boolean; renewedIds?: string[] }
export interface PreferenceStore { load(): Promise<AccountPreferences>; save(value: AccountPreferences): Promise<void> }
/** Presentation preferences only. All credential storage belongs to claude-swap. */
export class AccountPreferenceStore implements PreferenceStore {
  constructor(private readonly path: string) {}
  async load(): Promise<AccountPreferences> {
    try {
      const v = JSON.parse(await readFile(this.path, 'utf8')) as AccountPreferences
      if (!v.names || typeof v.names !== 'object' || Array.isArray(v.names) || Object.values(v.names).some(name => typeof name !== 'string' || !name.trim() || [...name].length > 12) || typeof v.rememberScope !== 'boolean') throw new Error('schema')
      if (v.renewedIds !== undefined && (!Array.isArray(v.renewedIds) || v.renewedIds.some(id => typeof id !== 'string' || !id || id.length > 200))) throw new Error('schema')
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
  private initialized = false
  private restartResult: ClaudeAccountsState['restartResult']
  private usageOwners = new Set<number>()
  private usageTimer: ReturnType<typeof setTimeout> | undefined
  private usageFlight: Promise<ClaudeAccountsState> | undefined
  private usageAbort: AbortController | undefined
  private usageLastAttempt = 0
  private usageRefreshFailed = false
  private usageFailures = 0
  private usageDue = false
  private usageSuspended = false
  private prefs: AccountPreferences = { names: {}, rememberScope: false }
  private load: Promise<void> | undefined
  private loaded = false
  private value: EngineState = { accounts: [] }
  private operation: ClaudeAccountsState['operation'] = 'idle'
  private revision = 0
  private error: AccountError | undefined
  private draft: ClaudeAccountView | undefined
  private reauthId: string | undefined
  private appliedReauthId: string | undefined
  private nonce: string | undefined
  private loginCodeSubmitted = false
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
    return { ...this.value, ...(this.restartResult ? { restartResult: { ...this.restartResult } } : {}), usageRefreshing: Boolean(this.usageFlight), usageRefreshFailed: this.usageRefreshFailed, accounts: this.value.accounts.map(a => this.named(a)),
      ...(this.value.detected ? { detected: this.named(this.value.detected) } : {}),
      revision: this.revision, rememberScope: this.prefs.rememberScope,
      ...(this.reauthId ? { reauthId: this.reauthId } : {}), renewedIds: this.prefs.renewedIds ?? [],
      ...(this.appliedReauthId ? { appliedReauthId: this.appliedReauthId } : {}),
      ...(this.operation === 'login' && this.nonce && this.loginAbort && !this.loginAbort.signal.aborted ? { loginAttemptId: this.nonce, loginCodeSubmitted: this.loginCodeSubmitted } : {}),
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
    this.value = await this.engine.run({ action }); this.initialized = true
    this.sessions = await this.runtime.request('snapshot')
    for (const [id, run] of this.pending) if (!this.sessions.some(s => s.sessionId === id && s.runId === run)) this.pending.delete(id)
  }
  /** One scheduler for all windows. Upstream owns network TTL/backoff and credentials. */
  setUsageVisible(owner: number, visible: boolean, recheck = false) {
    if (visible) this.usageOwners.add(owner); else this.usageOwners.delete(owner)
    if (visible && recheck) this.recheckUsage()
    this.scheduleUsage()
    return this.state()
  }
  private recheckUsage() {
    // A restored connection may recover sooner than local retry backoff. Upstream
    // still enforces its own Retry-After. Coalesce bursts of focus/online events.
    if (Date.now() - this.usageLastAttempt >= 180_000 && this.value.accounts.some(a => !a.needsLogin && (!a.usage || Date.now() - a.usage.checkedAt >= 180_000))) this.usageDue = true
  }
  suspendUsage(suspended: boolean) { this.usageSuspended = suspended; if (!suspended) this.recheckUsage(); this.scheduleUsage() }
  private scheduleUsage() {
    if (this.usageTimer) clearTimeout(this.usageTimer)
    if (this.disposed || this.usageSuspended || !this.usageOwners.size || !this.value.accounts.length || this.operation !== 'idle' || this.usageFlight) return
    const now = Date.now()
    const interval = Math.min(1_800_000, 180_000 * 2 ** this.usageFailures)
    let due = this.usageDue ? now : this.usageLastAttempt + interval
    for (const account of this.value.accounts) {
      if (account.needsLogin) continue
      for (const window of [account.usage?.fiveHour, account.usage?.weekly]) {
        const reset = Date.parse(window?.resetsAt ?? '') + 15_000
        if (reset > this.usageLastAttempt) due = Math.min(due, reset)
      }
    }
    this.usageTimer = setTimeout(() => { void this.refreshUsage() }, Math.max(0, due - now))
  }
  private refreshUsage(): Promise<ClaudeAccountsState> {
    if (this.usageFlight) return this.usageFlight
    if (this.disposed || this.operation !== 'idle' || !this.value.accounts.length) return Promise.resolve(this.state())
    this.usageLastAttempt = Date.now(); this.usageDue = false
    const abort = new AbortController(); this.usageAbort = abort
    const timeout = setTimeout(() => abort.abort(), 30_000)
    this.usageFlight = (async () => {
      // Start on the next microtask so the in-flight flag is already observable.
      await Promise.resolve(); this.emit()
      try {
        const next = await this.engine.run({ action: 'refresh' }, abort.signal)
        if (!abort.signal.aborted && !this.disposed && this.operation === 'idle') {
          this.value = next; this.usageRefreshFailed = false; this.usageFailures = 0
        }
      } catch {
        if (!this.disposed && this.operation === 'idle') {
          this.usageRefreshFailed = true; this.usageFailures = Math.min(4, this.usageFailures + 1)
        }
      } finally {
        clearTimeout(timeout); this.usageFlight = undefined; this.usageAbort = undefined
        this.emit(); this.scheduleUsage()
      }
      return this.state()
    })()
    return this.usageFlight
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
    if (command.type === 'ensure-usage' && this.initialized) return this.state()
    if (command.type === 'usage-visibility') return this.state() // Owner identity is supplied by main IPC.
    if (command.type === 'refresh-usage') return this.refreshUsage()
    if (command.type === 'refresh-sessions') {
      if (this.operation === 'idle') {
        try { this.sessions = await this.runtime.request('snapshot'); this.emit() } catch {}
      }
      return this.state()
    }
    if (command.type === 'submit-login-code') {
      if (this.operation !== 'login' || !this.loginAbort || this.loginAbort.signal.aborted || this.nonce !== command.attemptId) {
        this.error = 'login-ended'; this.emit(); return this.state()
      }
      if (this.loginCodeSubmitted) return this.state()
      if (!validLoginCode(command.code)) { this.error = 'login-code-invalid'; this.emit(); return this.state() }
      this.loginCodeSubmitted = true; this.error = undefined; this.emit()
      try {
        if (!this.engine.submitLoginCode) throw new AccountFault('login-ended')
        await this.engine.submitLoginCode(command.attemptId, command.code)
      } catch (e) {
        if (this.nonce === command.attemptId && this.loginAbort && !this.loginAbort.signal.aborted) {
          this.loginCodeSubmitted = false; this.error = fault(e); this.emit()
        }
      }
      return this.state()
    }
    if (command.type === 'cancel-login') {
      this.loginAbort?.abort()
      if (!this.loginAbort && this.nonce) {
        await this.engine.run({ action: 'cancel-login', nonce: this.nonce }).catch(() => {})
        this.nonce = undefined; this.draft = undefined
      }
      this.reauthId = undefined
      this.emit(); return this.state()
    }
    if (command.type === 'cancel-switch') { if (this.operation === 'switching') return this.state(); this.cancelQueue(); this.emit(); return this.state() }
    if (command.type === 'dismiss-error') { this.error = undefined; this.emit(); return this.state() }
    if (this.operation !== 'idle') return this.state()
    this.error = undefined
    this.operation = command.type === 'login' ? 'login' : (command.type === 'switch' || command.type === 'apply-reauth') ? 'switching' : (command.type === 'resume' || command.type === 'restart-all') ? 'resuming' : 'loading'
    this.emit()
    try {
      // Foreground actions preempt quota collection; never merge an old roster into a new login.
      this.usageAbort?.abort()
      await this.usageFlight
      switch (command.type) {
        case 'ensure-usage': case 'inspect': case 'recover': await this.refresh(); break
        case 'login': {
          if (command.replaceId && !this.value.accounts.some(a => a.id === command.replaceId)) throw new AccountFault('conflict')
          this.reauthId = command.replaceId
          this.cancelQueue() // A repair never competes with a queued account change.
          if (this.nonce) await this.engine.run({ action: 'cancel-login', nonce: this.nonce })
          const nonce = randomUUID(), abort = new AbortController()
          this.nonce = nonce; this.loginCodeSubmitted = false; this.error = undefined; this.draft = undefined; this.loginAbort = abort
          this.emit() // Clear the previous identity before a repeated sign-in completes.
          try {
            const result = await this.engine.run({ action: 'login', nonce, replaceId: command.replaceId }, abort.signal)
            if (!abort.signal.aborted) {
              if (this.reauthId && result.draft?.id !== this.reauthId) throw new AccountFault('identity')
              this.draft = result.draft; this.error = undefined
            }
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
          const repairing = command.type === 'save-draft' && this.reauthId
          if (repairing && draft.id !== repairing) throw new AccountFault('identity')
          this.value = await this.engine.run({ action: command.type === 'save-draft' ? 'save-draft' : 'save-current', id: draft.id, nonce: this.nonce })
          if (command.type === 'save-draft') { this.nonce = undefined; this.draft = undefined }
          await this.savePrefs({ ...this.prefs, names: { ...this.prefs.names, [draft.id]: repairing ? this.named(draft).name : command.name.trim() },
            ...(repairing ? { renewedIds: [...new Set([...(this.prefs.renewedIds ?? []), draft.id])] } : {}) })
          this.reauthId = undefined
          break
        }
        case 'rename': {
          if (!this.value.accounts.some(a => a.id === command.id)) throw new AccountFault('conflict')
          await this.savePrefs({ ...this.prefs, names: { ...this.prefs.names, [command.id]: command.name.trim() } }); break
        }
        case 'remove': {
          this.value = await this.engine.run({ action: 'remove', id: command.id })
          const { [command.id]: _, ...names } = this.prefs.names; await this.savePrefs({ ...this.prefs, names, ...(this.prefs.renewedIds ? { renewedIds: this.prefs.renewedIds.filter(id => id !== command.id) } : {}) })
          if (this.queuedId === command.id) this.cancelQueue()
          break
        }
        case 'apply-reauth': {
          if (!this.prefs.renewedIds?.includes(command.id)) throw new AccountFault('conflict')
          await this.switchTo(command.id, true)
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
        case 'restart-all': {
          this.cancelQueue()
          this.sessions = await this.runtime.request('snapshot')
          const targets = [...new Map(this.sessions.filter(s => s.runId).map(s => [s.sessionId, s])).values()]
          this.restartResult = { total: targets.length, restarted: 0, failed: 0, completed: false }; this.emit()
          // Bounded parallelism keeps many windows responsive without a launch storm.
          const queue = [...targets]
          const worker = async () => {
            while (queue.length && !this.disposed) {
              const target = queue.shift()!
              try {
                await this.runtime.request('restart', target.sessionId, target.runId)
                this.restartResult!.restarted++; this.pending.delete(target.sessionId)
              } catch { this.restartResult!.failed++ }
              this.emit()
            }
          }
          await Promise.all(Array.from({ length: Math.min(3, targets.length) }, worker))
          this.restartResult.completed = true
          this.sessions = await this.runtime.request('snapshot').catch(() => this.sessions)
          this.usageDue = true
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
    finally {
      this.operation = 'idle'; this.emit()
      if (!this.error && ['save-detected', 'save-draft', 'switch', 'apply-reauth'].includes(command.type)) this.usageDue = true
      this.scheduleUsage()
    }
    return this.state()
  }
  private async switchTo(id: string, repair = false) {
    let held = false
    try {
      this.sessions = await this.runtime.request('hold'); held = true
      if (this.sessions.some(s => s.busy)) throw new AccountFault('busy')
      this.value = await this.engine.run({ action: repair ? 'reauth' : 'switch', id })
      if (this.value.currentId !== id) throw new AccountFault('switch-failed')
      this.pending = new Map(this.sessions.filter(s => s.defaultLogin).map(s => [s.sessionId, s.runId]))
      this.appliedReauthId = repair ? id : undefined
      this.restartResult = undefined
      this.switchedAt = Date.now(); this.usageDue = true; this.cancelQueue()
      if (this.prefs.renewedIds?.includes(id)) await this.savePrefs({ ...this.prefs, renewedIds: this.prefs.renewedIds.filter(value => value !== id) })
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
      this.usageAbort?.abort(); await this.usageFlight
      this.sessions = await this.runtime.request('snapshot')
      if (!this.disposed && this.queuedId === target && !this.sessions.some(s => s.busy)) { this.operation = 'switching'; this.emit(); await this.switchTo(target) }
    } catch (e) { this.error = fault(e); this.cancelQueue() }
    finally { this.operation = 'idle'; this.emit(); if (this.queuedId) this.schedule(); this.scheduleUsage() }
  }
  private cancelQueue() { this.queuedId = undefined; if (this.timer) clearTimeout(this.timer) }
  dispose(): Promise<void> {
    this.closing ??= (async () => {
      this.disposed = true; this.cancelQueue(); this.loginAbort?.abort(); this.usageAbort?.abort()
      if (this.usageTimer) clearTimeout(this.usageTimer)
      this.usageOwners.clear()
      await Promise.allSettled([...this.inFlight, this.queueFlight, this.usageFlight])
      if (this.nonce) await this.engine.run({ action: 'cancel-login', nonce: this.nonce }).catch(() => {})
    })()
    return this.closing
  }
}
