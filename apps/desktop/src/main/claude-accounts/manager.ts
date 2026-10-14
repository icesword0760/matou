import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname } from 'node:path'
import type { ClaudeAccountRuntimeAction, ClaudeAccountSession } from '@matou/contracts'
import { expiredUsageWindow, validAccountUsage } from '../../shared/account-usage'
import type { AccountUsage } from '../../shared/claude-accounts'
import { validLoginCode } from '../../shared/claude-accounts'
import type { AccountError, ClaudeAccountCommand, ClaudeAccountsState, ClaudeAccountView } from '../../shared/claude-accounts'
import { AccountFault, type AccountEngine, type EngineState } from './engine'

export interface AccountRuntime {
  request(action: ClaudeAccountRuntimeAction, sessionId?: string, runId?: string): Promise<ClaudeAccountSession[]>
}
export interface AccountPreferences { names: Record<string, string>; rememberScope: boolean; renewedIds?: string[]; usage?: Record<string, AccountUsage> }
export interface PreferenceStore { load(): Promise<AccountPreferences>; save(value: AccountPreferences): Promise<void> }
/** Presentation preferences and last official quota only; never credentials. */
export class AccountPreferenceStore implements PreferenceStore {
  constructor(private readonly path: string) {}
  async load(): Promise<AccountPreferences> {
    try {
      const v = JSON.parse(await readFile(this.path, 'utf8')) as AccountPreferences
      if (!v.names || typeof v.names !== 'object' || Array.isArray(v.names) || Object.values(v.names).some(name => typeof name !== 'string' || !name.trim() || [...name].length > 12) || typeof v.rememberScope !== 'boolean') throw new Error('schema')
      if (v.renewedIds !== undefined && (!Array.isArray(v.renewedIds) || v.renewedIds.some(id => typeof id !== 'string' || !id || id.length > 200))) throw new Error('schema')
      if (v.usage) v.usage = typeof v.usage === 'object' && !Array.isArray(v.usage)
        ? Object.fromEntries(Object.entries(v.usage).filter(([, u]) => validAccountUsage(u))) : {}
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
  private rosterCheckedAt = 0
  private restartResult: ClaudeAccountsState['restartResult']
  private usageTimer: ReturnType<typeof setTimeout> | undefined
  private usageFlight: Promise<ClaudeAccountsState> | undefined
  private usageAbort: AbortController | undefined
  private usageRefreshFailed = false
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
  private cleaned = false
  private sessions: ClaudeAccountSession[] = []
  private pending = new Map<string, string>()
  private switchedAt: number | undefined
  private disposed = false
  private inFlight = new Set<Promise<ClaudeAccountsState>>()
  private closing: Promise<void> | undefined
  constructor(private readonly engine: AccountEngine, private readonly store: PreferenceStore,
    private readonly runtime: AccountRuntime, private readonly publish: (state: ClaudeAccountsState) => void) {}
  private named(a: ClaudeAccountView) {
    const usage = this.prefs.usage?.[a.id] ?? a.usage
    const active = this.value.currentId ?? this.value.detected?.id
    return { ...a, name: this.prefs.names[a.id] || a.name, ...(usage ? { usage: { ...usage,
      stale: a.id !== active || usage.stale || Date.now() - usage.checkedAt > 180_000 ||
        expiredUsageWindow(usage.fiveHour) || expiredUsageWindow(usage.weekly)
    } } : {}) }
  }
  state(): ClaudeAccountsState {
    return { ...this.value, ...(this.restartResult ? { restartResult: { ...this.restartResult } } : {}), usageRefreshing: Boolean(this.usageFlight), usageRefreshFailed: this.usageRefreshFailed, accounts: this.value.accounts.map(a => this.named(a)),
      ...(this.value.detected ? { detected: this.named(this.value.detected) } : {}),
      revision: this.revision, rememberScope: this.prefs.rememberScope,
      ...(this.reauthId ? { reauthId: this.reauthId } : {}), renewedIds: this.prefs.renewedIds ?? [],
      ...(this.appliedReauthId ? { appliedReauthId: this.appliedReauthId } : {}),
      ...(this.operation === 'login' && this.nonce && this.loginAbort && !this.loginAbort.signal.aborted ? { loginAttemptId: this.nonce, loginCodeSubmitted: this.loginCodeSubmitted } : {}),
      operation: this.operation, sessions: this.sessions, pendingSessionIds: [...this.pending.keys()],
      ...(this.draft ? { draft: this.named(this.draft) } : {}), ...(this.error ? { error: this.error } : {}),
      ...(this.switchedAt ? { switchedAt: this.switchedAt } : {}) }
  }
  private emit() { this.revision++; if (!this.disposed) this.publish(this.state()) }
  async snapshot() {
    this.load ??= this.store.load().then(v => { this.prefs = v; this.loaded = true }).catch(() => { this.error = 'storage'; this.load = undefined })
    await this.load; return this.state()
  }
  private async savePrefs(next: AccountPreferences) { await this.store.save(next); this.prefs = next }
  private async refresh(action: 'snapshot' | 'refresh' = 'snapshot', signal?: AbortSignal) {
    if (!this.cleaned && !this.nonce) { await this.engine.run({ action: 'cleanup-logins' }); this.cleaned = true }
    const previous = this.value.currentId ?? this.value.detected?.id
    const next = await this.engine.run({ action }, signal)
    if (signal?.aborted) return
    this.value = next; this.initialized = true; this.rosterCheckedAt = Date.now()
    if (previous !== (this.value.currentId ?? this.value.detected?.id)) await this.runtime.request('usage-reset')
    this.sessions = await this.runtime.request('snapshot')
    await this.rememberUsage(this.sessions)
    for (const [id, run] of this.pending) if (!this.sessions.some(s => s.sessionId === id && s.runId === run)) this.pending.delete(id)
  }
  /** Poll local runtime observations only, including hidden sessions. No HTTP. */
  setUsageVisible(_owner: number, _visible: boolean, _recheck = false) {
    this.scheduleUsage(); return this.state()
  }
  suspendUsage(suspended: boolean) { this.usageSuspended = suspended; this.scheduleUsage() }
  private scheduleUsage() {
    if (this.usageTimer) clearTimeout(this.usageTimer)
    if (this.disposed || this.usageSuspended || !this.initialized || this.operation !== 'idle' || this.usageFlight) return
    this.usageTimer = setTimeout(() => { void this.refreshUsage() }, 5_000)
  }
  private async rememberUsage(sessions: ClaudeAccountSession[]) {
    const id = this.value.currentId ?? this.value.detected?.id
    if (!id) return
    const latest = sessions.filter(s => s.defaultLogin && s.usage?.accountId === id && s.usage.observedAt <= Date.now() + 1_000)
      .map(s => s.usage!).sort((a, b) => b.observedAt - a.observedAt)[0]
    const old = this.prefs.usage?.[id] ?? this.value.accounts.find(a => a.id === id)?.usage
    if (!latest || latest.observedAt <= (old?.checkedAt ?? 0)) return
    const usage: AccountUsage = { checkedAt: latest.observedAt, stale: false }
    for (const [label, key] of [['5h', 'fiveHour'], ['Weekly', 'weekly']] as const) {
      const window = latest.windows.find(w => w.label === label)
      if (window && Number.isFinite(window.percent)) usage[key] = { used: Math.max(0, Math.min(100, window.percent)),
        resetsAt: window.resetsAt && Number.isFinite(new Date(window.resetsAt).getTime()) ? new Date(window.resetsAt).toISOString() : '' }
    }
    await this.savePrefs({ ...this.prefs, usage: { ...this.prefs.usage, [id]: usage } })
    // A fresh successful official response supersedes a prior local missing-login flag.
    this.value = { ...this.value, accounts: this.value.accounts.map(a => a.id === id ? { ...a, needsLogin: false } : a) }
  }
  private refreshUsage(inspect = false): Promise<ClaudeAccountsState> {
    if (this.usageFlight) return this.usageFlight
    if (this.disposed || this.operation !== 'idle' || !this.initialized) return Promise.resolve(this.state())
    const abort = new AbortController(); this.usageAbort = abort
    this.usageFlight = (async () => {
      await Promise.resolve()
      if (inspect) this.emit()
      try {
        if (inspect || Date.now() - this.rosterCheckedAt >= 60_000) await this.refresh('snapshot', abort.signal) // local roster read, never network
        else {
          const sessions = await this.runtime.request('snapshot')
          if (!abort.signal.aborted && !this.disposed && this.operation === 'idle') {
            this.sessions = sessions; await this.rememberUsage(sessions)
          }
        }
        this.usageRefreshFailed = false
      } catch { if (!abort.signal.aborted) this.usageRefreshFailed = true }
      finally {
        this.usageFlight = undefined; this.usageAbort = undefined
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
    if (command.type === 'refresh-usage') return this.refreshUsage(true)
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
          const { [command.id]: _, ...names } = this.prefs.names
          const { [command.id]: _usage, ...usage } = this.prefs.usage ?? {}
          await this.savePrefs({ ...this.prefs, names, usage, ...(this.prefs.renewedIds ? { renewedIds: this.prefs.renewedIds.filter(id => id !== command.id) } : {}) })
          break
        }
        case 'apply-reauth': {
          if (!this.prefs.renewedIds?.includes(command.id)) throw new AccountFault('conflict')
          await this.switchTo(command.id, true)
          break
        }
        case 'switch': {
          await this.switchTo(command.id)
          await this.savePrefs({ ...this.prefs, rememberScope: command.rememberScope })
          break
        }
        case 'restart-all': {
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
      this.scheduleUsage()
    }
    return this.state()
  }
  private async switchTo(id: string, repair = false) {
    let held = false
    try {
      // Serialize against provider mutations only; working Claude processes keep running.
      // Upstream coordinates credential writes with Claude Code's own locks.
      this.sessions = await this.runtime.request('hold'); held = true
      await this.rememberUsage(this.sessions).catch(() => { this.usageRefreshFailed = true })
      this.value = await this.engine.run({ action: repair ? 'reauth' : 'switch', id })
      if (this.value.currentId !== id) throw new AccountFault('switch-failed')
      await this.runtime.request('usage-reset')
      this.pending = new Map(this.sessions.filter(s => s.defaultLogin).map(s => [s.sessionId, s.runId]))
      this.appliedReauthId = repair ? id : undefined
      this.restartResult = undefined
      this.switchedAt = Date.now()
      if (this.prefs.renewedIds?.includes(id)) await this.savePrefs({ ...this.prefs, renewedIds: this.prefs.renewedIds.filter(value => value !== id) })
    } finally { if (held) await this.runtime.request('release').catch(() => {}) }
  }
  dispose(): Promise<void> {
    this.closing ??= (async () => {
      this.disposed = true; this.loginAbort?.abort(); this.usageAbort?.abort()
      if (this.usageTimer) clearTimeout(this.usageTimer)
      await Promise.allSettled([...this.inFlight, this.usageFlight])
      if (this.initialized) await this.runtime.request('snapshot').then(s => this.rememberUsage(s)).catch(() => {})
      if (this.nonce) await this.engine.run({ action: 'cancel-login', nonce: this.nonce }).catch(() => {})
    })()
    return this.closing
  }
}
