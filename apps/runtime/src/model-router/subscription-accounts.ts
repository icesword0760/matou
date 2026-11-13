import { randomUUID } from 'node:crypto'
import { readdir } from 'node:fs/promises'
import { release } from 'node:os'
import { join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import type {
  ProviderCli, ProviderConfigSnapshot, ProviderConfigView,
  SubscriptionAccountDetails, SubscriptionAccountSummary, SubscriptionLoginStartInput,
  SubscriptionLoginView, SubscriptionModelsReadInput, SubscriptionModel, SubscriptionProvider,
  SubscriptionProviderConfigInput, SubscriptionSettingsSnapshot, ProviderActivationFailure
} from '@matou/contracts'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { ModelRouterManager, type RouterLease } from './manager'
import { RouterManagementClient, type RouterCredential } from './management-client'
import { OAuthLoopbackFlow, type OAuthOutcome } from './oauth'
import { buildClaudeLaunch, toClientModelId, FABLE_FALLBACK_ALIAS, type ClaudeLaunchInput, type ModelAlias } from './claude-launch'
import {
  parseCredentialSummary, parseChatGptPlan, readChatGptUsage, readChatGptModels,
  readAntigravityAccount, readAntigravityQuota, parseChatGptModels
} from './account-readers'
import { mapAccountStatus, mapSubscriptionLaunchFailure, mapSubscriptionLoginFailure, sanitizeProviderReason, type SubscriptionLaunchFailureKind } from './errors'

export interface PreparedSubscriptionLaunch {
  lease: RouterLease
  input: Omit<ClaudeLaunchInput, 'inheritedEnvNames'>
}
/** A platform requirement is final for this runtime, not a router retry. */
export class SubscriptionPlatformError extends Error {
  readonly messageLocalized = { zh: '需要 macOS 15 或更高版本', en: 'Requires macOS 15 or later' }
  constructor(locale = process.env.MATOU_LOCALE) {
    super(locale === 'en' ? 'Requires macOS 15 or later' : '需要 macOS 15 或更高版本')
  }
}
export class SubscriptionLaunchError extends Error {
  readonly providerFailure: ProviderActivationFailure
  constructor(diagnostic: string, kind: SubscriptionLaunchFailureKind = 'router-start-failed') {
    const mapped = mapSubscriptionLaunchFailure(kind, diagnostic)
    const locale = process.env.MATOU_LOCALE === 'en' ? 'en' : 'zh'
    const detail = mapped.reasonLocalized
    const message = detail ? { zh: `${mapped.message!.zh}：${detail.zh}`, en: `${mapped.message!.en}: ${detail.en}` } : mapped.message!
    super(message[locale])
    this.providerFailure = { status: kind === 'needs-sign-in' ? 'needs-sign-in' : 'router-start-failed', message, recovery: kind === 'needs-sign-in' ? 'sign-in' : 'retry', ...(detail ? { reason: detail[locale], reasonLocalized: detail } : {}) }
    console.error(`[subscription.prepare] ${kind}: ${sanitizeProviderReason(diagnostic) ?? 'No diagnostic detail'}`)
  }
}

const roles = ['opus', 'sonnet', 'haiku'] as const
const prefixFor = (provider: SubscriptionProvider) => provider === 'codex' ? 'matou-chatgpt' : 'matou-antigravity'
interface Attempt {
  owner: string
  view: SubscriptionLoginView
  lease: RouterLease
  client: RouterManagementClient
  flow: OAuthLoopbackFlow
  state: string
  authorizationUrl: string
  browserOpenRevision: number
  credential?: RouterCredential
  done: Promise<void>
}
interface SettingsRound {
  lease?: RouterLease
  ready: Promise<void>
  accounts: Map<string, { ref: string; details: SubscriptionAccountDetails }>
}
interface Owner { closed: boolean; round?: SettingsRound }
export interface SubscriptionConnection {
  start(input: SubscriptionLoginStartInput): Promise<SubscriptionLoginView>
  status(attemptId: string): SubscriptionLoginView
  reopen(attemptId: string): Promise<SubscriptionLoginView>
  cancel(attemptId: string): Promise<SubscriptionLoginView>
  models(input: SubscriptionModelsReadInput): Promise<SubscriptionAccountDetails>
  settings(): Promise<SubscriptionSettingsSnapshot>
  closeSettings(): Promise<void>
  confirmRisk(accepted: boolean): Promise<{ accepted: true }>
  save(input: SubscriptionProviderConfigInput): Promise<ProviderConfigView>
}
interface Options {
  dataRoot: string
  manager: ModelRouterManager
  store: ProviderConfigStore
  openAuthorizationUrl(provider: SubscriptionProvider, url: string): Promise<void>
  loginTimeoutMs?: number
  callbackPorts?: Partial<Record<SubscriptionProvider, number>>
  /** Injectable OS facts for the macOS 14/15 compatibility boundary. */
  platform?: NodeJS.Platform
  systemVersion?(): string
}

/** Runtime-only coordinator. Opaque attempts belong to one RPC connection.
 * The single mutation queue covers draft adoption/replacement/deletion; long
 * provider reads run outside it so cancellation remains responsive.
 */
export class SubscriptionAccounts {
  readonly #options: Options
  readonly #subscriptionSupported: boolean
  readonly #attempts = new Map<string, Attempt>()
  readonly #owners = new Map<string, Owner>()
  readonly #details = new Map<string, SubscriptionAccountDetails>()
  readonly #credentialStatus = new Map<string, SubscriptionAccountSummary>()
  #queue: Promise<unknown> = Promise.resolve()
  #startup: Promise<void> | undefined
  #closed = false
  #routerUnavailable = false
  readonly #changeListeners = new Set<() => void>()
  readonly #unsubscribeRouter: () => void
  constructor(options: Options) {
    this.#options = options
    // Darwin 24 is macOS 15. Only gate older macOS; other platforms keep their
    // existing behavior. No user-agent or persisted compatibility preference.
    this.#subscriptionSupported = (options.platform ?? process.platform) !== 'darwin' ||
      Number.parseInt(options.systemVersion?.() ?? String(Number.parseInt(release(), 10) - 9), 10) >= 15
    this.#unsubscribeRouter = options.manager.onStateChange(state => {
      const unavailable = state !== 'running'
      if (this.#routerUnavailable === unavailable) return
      this.#routerUnavailable = unavailable
      this.#notifyChanged()
    })
  }
  onChange(listener: () => void): () => void { this.#changeListeners.add(listener); return () => this.#changeListeners.delete(listener) }
  #notifyChanged(): void { for (const listener of this.#changeListeners) listener() }

  /** Shared preflight for activation and every final Claude spawn. The returned
   * lease belongs to the caller until process exit (or preparation failure).
   * No provider model/quota request is made here.
   */
  async prepareLaunch(providerConfigId: string, scope = 'subscription-session'): Promise<PreparedSubscriptionLaunch> {
    this.#assertSupported()
    let lease: RouterLease | undefined
    try {
      await this.initialize()
      return await this.#serial(async () => {
        if (this.#closed) throw new Error('Subscription accounts are closed')
        const row = (await this.#options.store.subscriptions()).find(row => row.id === providerConfigId)
        if (!row) throw new Error('Subscription provider does not exist')
        const provider = row.subscription.provider
        lease = await this.#options.manager.acquire(scope)
        const client = new RouterManagementClient(lease.connection)
        const file = (await client.credentials()).find(file => file.name === row.subscription.credentialRef.name && file.provider === provider)
        if (!file || credentialNeedsSignIn(file)) {
          this.#markNeedsSignIn(row.subscription.credentialRef.name, row.subscription.account)
          throw new SubscriptionLaunchError('Subscription credential is missing or no longer valid', 'needs-sign-in')
        }
        try {
          // The credential list is itself a runtime-auth-manager read. An
          // already active exact entry need not be fetched a second time.
          if (file.status !== 'active' || file.disabled === true || file.requires_sign_in === true) {
            await this.#options.manager.waitForCredentialAdoption({ name: file.name, provider, authIndex: file.auth_index })
          }
        } catch (error) {
          const files = await client.credentials().catch(() => undefined)
          const current = files?.find(value => value.name === file.name && value.provider === provider)
          if (files && (!current || credentialNeedsSignIn(current))) {
            this.#markNeedsSignIn(file.name, row.subscription.account)
            throw new SubscriptionLaunchError('Subscription credential disappeared or expired during preparation', 'needs-sign-in')
          }
          throw error
        }
        const aliases = aliasesFor(provider, row.modelMapping, row.subscription.modelMetadata)
        // Rewriting an identical prefix/config still wakes the engine watcher.
        // Activation and final spawn both prepare; keep the second pass read-only
        // when the first one has already registered this exact configuration.
        const [registered, registeredModels] = await Promise.all([client.aliases(provider), client.models(file.name)])
        if (!sameAliases(registered, aliases)) await client.setAliases(provider, aliases)
        // v8's credential list omits prefix. The published model IDs are the
        // authoritative proof that this credential has adopted its prefix.
        const modelsReady = hasAliases(registeredModels, prefixFor(provider), aliases)
        if (!modelsReady) await client.setPrefix(file.name, prefixFor(provider))
        if (!modelsReady || !sameAliases(registered, aliases)) await waitForAliases(client, file.name, prefixFor(provider), aliases)
        const models = Object.fromEntries(roles.map(role => [role, row.subscription.modelMetadata.find(model => model.id === row.modelMapping[role])])) as ClaudeLaunchInput['models']
        const input: PreparedSubscriptionLaunch['input'] = {
          subscription: provider === 'codex' ? 'chatgpt' : 'antigravity',
          accountPrefix: prefixFor(provider) as ClaudeLaunchInput['accountPrefix'],
          routerUrl: lease.connection.baseUrl, clientKey: lease.connection.clientKey,
          models, mainRole: 'opus', providerName: row.name
        }
        const projected = buildClaudeLaunch(input)
        if (!projected.ok) throw new SubscriptionLaunchError(`Subscription launch configuration: ${projected.error}`, projected.error === 'unsupported-context-window' ? 'unsupported-context-window' : 'router-start-failed')
        this.#options.manager.rememberModelAliases(provider, aliases)
        if (this.#credentialStatus.get(file.name)?.status === 'needs-sign-in') {
          this.#credentialStatus.set(file.name, { ...row.subscription.account, status: 'ready' })
          this.#notifyChanged()
        }
        return { lease, input }
      })
    } catch (error) {
      lease?.release()
      if (error instanceof SubscriptionLaunchError) throw error
      throw new SubscriptionLaunchError(error instanceof Error ? error.message : 'Subscription launch could not be prepared')
    }
  }
  #markNeedsSignIn(name: string, account: SubscriptionAccountSummary): void {
    const { reason: _reason, ...summary } = account
    this.#credentialStatus.set(name, { ...summary, status: 'needs-sign-in' })
    this.#notifyChanged()
  }

  initialize(): Promise<void> {
    if (!this.#subscriptionSupported) return Promise.resolve()
    if (this.#startup) return this.#startup
    const pending = this.#serial(() => this.#reconcile())
    this.#startup = pending
    void pending.catch(() => { if (this.#startup === pending) this.#startup = undefined })
    return pending
  }
  bindConnection(port: { once(event: 'close', listener: () => void): unknown }): SubscriptionConnection {
    const id = randomUUID(), owner: Owner = { closed: false }
    this.#owners.set(id, owner)
    const assertOpen = () => { if (owner.closed || this.#closed) throw new Error('Subscription connection is closed') }
    port.once('close', () => {
      owner.closed = true
      this.#options.manager.releaseConnection(id)
      void this.#closeSettings(id).catch(() => undefined).finally(() => {
        this.#owners.delete(id)
        for (const [attemptId, attempt] of this.#attempts) if (attempt.owner === id) this.#attempts.delete(attemptId)
      })
    })
    return {
      start: input => { assertOpen(); return this.#start(id, input) },
      status: attemptId => { assertOpen(); return structuredClone(this.#attempt(id, attemptId).view) },
      reopen: attemptId => { assertOpen(); return this.#serial(async () => {
        assertOpen()
        const attempt = this.#attempt(id, attemptId)
        if (!this.#canOpenBrowser(attempt)) throw new Error('Sign-in attempt is no longer waiting')
        attempt.view.status = 'waiting'
        delete attempt.view.reason
        delete attempt.view.reasonLocalized
        delete attempt.view.canReopen
        this.#openBrowser(attempt)
        return structuredClone(attempt.view)
      }) },
      cancel: attemptId => { assertOpen(); return this.#serial(() => this.#cancel(this.#attempt(id, attemptId))) },
      models: input => { assertOpen(); return this.#reloadModels(id, input) },
      settings: () => { assertOpen(); return this.#settings(id) },
      closeSettings: () => { assertOpen(); return this.#closeSettings(id) },
      confirmRisk: accepted => { assertOpen(); if (accepted !== true) throw new Error('Risk confirmation is required'); return this.#options.store.confirmAntigravityRisk() },
      save: input => { assertOpen(); return this.#save(id, input) }
    }
  }
  async snapshot(): Promise<ProviderConfigSnapshot> {
    const snapshot = await this.#options.store.snapshot()
    snapshot.subscriptionSupported = this.#subscriptionSupported
    const stored = await this.#options.store.subscriptions()
    for (const row of snapshot.providers['claude-code']) {
      const reference = stored.find(value => value.id === row.id)
      if (row.subscription && reference) {
        row.subscription.account = { ...row.subscription.account, ...this.#credentialStatus.get(reference.subscription.credentialRef.name) }
        if (this.#routerUnavailable && row.subscription.account.status !== 'needs-sign-in') row.subscription.account.status = 'router-not-running'
      }
    }
    return snapshot
  }
  #assertSupported(locale?: string): void {
    if (!this.#subscriptionSupported) throw new SubscriptionPlatformError(locale)
  }
  #attempt(owner: string, id: string): Attempt {
    const attempt = this.#attempts.get(id)
    if (!attempt || attempt.owner !== owner) throw new Error('Sign-in attempt does not exist for this connection')
    return attempt
  }
  #serial<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.#queue.then(operation)
    this.#queue = result.catch(() => undefined)
    return result
  }
  async #start(owner: string, input: SubscriptionLoginStartInput): Promise<SubscriptionLoginView> {
    if (!input || !['codex', 'antigravity'].includes(input.provider) || (input.locale !== undefined && !['zh-CN', 'en'].includes(input.locale))) throw new Error('Invalid subscription sign-in request')
    this.#assertSupported(input.locale)
    await this.initialize()
    return this.#serial(async () => {
      if (this.#owners.get(owner)?.closed || this.#closed) throw new Error('Subscription connection is closed')
      const pending = [...this.#attempts.values()].find(attempt => attempt.view.provider === input.provider && (['waiting', 'exchanging', 'ready'].includes(attempt.view.status) || attempt.view.canReopen))
      if (pending) {
        if (pending.owner !== owner) throw new Error('Sign-in is already in progress in another window')
        // A timed-out client may never have received the old attempt ID.
        // Release its callback listeners before creating the replacement.
        await this.#cancel(pending)
      }
      if (input.provider === 'antigravity' && !await this.#options.store.antigravityRiskAccepted()) throw new Error('Antigravity risk confirmation is required')
      const lease = await this.#options.manager.acquire('subscription-login')
      const client = new RouterManagementClient(lease.connection)
      let state: string | undefined
      try {
        const login = await client.startOAuth(input.provider)
        state = login.state
        const flow = await OAuthLoopbackFlow.start({
          provider: input.provider, login, client, ...(input.locale ? { locale: input.locale } : {}),
          ...(this.#options.loginTimeoutMs === undefined ? {} : { timeoutMs: this.#options.loginTimeoutMs }),
          ...(this.#options.callbackPorts?.[input.provider] === undefined ? {} : { callbackPort: this.#options.callbackPorts[input.provider] })
        })
        const view: SubscriptionLoginView = { attemptId: randomUUID(), provider: input.provider, status: 'waiting' }
        const attempt: Attempt = { owner, view, lease, client, flow, state, authorizationUrl: login.url, browserOpenRevision: 0, done: Promise.resolve() }
        this.#attempts.set(view.attemptId, attempt)
        attempt.done = flow.completion.then(outcome => this.#completed(attempt, outcome)).catch(() => undefined)
        this.#openBrowser(attempt)
        return structuredClone(view)
      } catch (error) {
        if (state) await client.cancelOAuth(state).catch(() => undefined)
        const attempt = [...this.#attempts.values()].find(value => value.state === state)
        try { if (attempt) await this.#cancel(attempt) } finally { lease.release() }
        throw new Error(sanitizeProviderReason(error instanceof Error ? error.message : undefined) ?? 'Sign-in could not be started')
      }
    })
  }
  #canOpenBrowser(attempt: Attempt): boolean {
    return !this.#closed && !this.#owners.get(attempt.owner)?.closed && !attempt.flow.callbackAccepted &&
      (attempt.view.status === 'waiting' || (attempt.view.status === 'failed' && attempt.view.canReopen === true))
  }
  #openBrowser(attempt: Attempt): void {
    const revision = ++attempt.browserOpenRevision
    // Launching the browser must never hold the mutation queue or login RPC.
    void Promise.resolve().then(() => {
      if (revision === attempt.browserOpenRevision && this.#canOpenBrowser(attempt)) {
        return this.#options.openAuthorizationUrl(attempt.view.provider, attempt.authorizationUrl)
      }
    }).catch(() => this.#serial(async () => {
      if (revision !== attempt.browserOpenRevision || !this.#canOpenBrowser(attempt)) return
      attempt.view.status = 'failed'
      attempt.view.canReopen = true
      attempt.view.reason = '浏览器没有打开'
      attempt.view.reasonLocalized = { zh: '浏览器没有打开', en: "The browser didn't open" }
    })).catch(() => undefined)
  }
  async #completed(attempt: Attempt, outcome: OAuthOutcome): Promise<void> {
    if (outcome.status !== 'ok') {
      await this.#serial(async () => {
        if (attempt.view.status === 'saved') return
        delete attempt.view.canReopen
        attempt.view.status = outcome.status === 'error' ? 'failed' : outcome.status
        attempt.view.reason = sanitizeProviderReason(outcome.reason) ?? 'Sign-in was not completed'
        attempt.view.reasonLocalized = mapSubscriptionLoginFailure(outcome.reason, outcome.status === 'error' ? undefined : outcome.status)
        try {
          await this.#collectCancelledCredential(attempt)
          await this.#cleanupDraft(attempt)
        } finally { attempt.lease.release() }
      })
      return
    }
    try {
      const credential = await this.#serial(async () => {
        const file = (await attempt.client.credentials()).find(value => value.id === outcome.authId || value.name === outcome.authId)
        if (!file || file.provider !== attempt.view.provider) throw new Error('Signed-in credential has not been adopted')
        attempt.credential = file
        if (!await this.#isReferenced(file.name)) {
          await attempt.client.setDisabled(file.name, true)
          await attempt.client.setPrefix(file.name, '')
        } else {
          // A same-account renewal can rewrite the same file without retaining
          // its routing fields. Preserve the committed card even if editing is cancelled.
          await this.#cleanupDraft(attempt)
        }
        if (this.#cancelled(attempt)) { await this.#cleanupDraft(attempt); return undefined }
        delete attempt.view.canReopen
        delete attempt.view.reason
        delete attempt.view.reasonLocalized
        attempt.view.status = 'exchanging'
        return file
      })
      if (!credential) return
      const details = await this.#readDetails(attempt.client, credential)
      await this.#serial(async () => {
        if (this.#cancelled(attempt)) { await this.#cleanupDraft(attempt); return }
        attempt.view.details = details
        attempt.view.status = 'ready'
        this.#details.set(credential.name, details)
      })
    } catch {
      await this.#serial(async () => {
        if (!this.#cancelled(attempt)) {
          attempt.view.status = 'failed'; attempt.view.reason = 'Signed-in account details could not be read'
          attempt.view.reasonLocalized = mapSubscriptionLoginFailure(attempt.view.reason)
        }
        try { await this.#cleanupDraft(attempt) } finally { attempt.lease.release() }
      })
    }
  }
  #cancelled(attempt: Attempt): boolean { return ['cancelled', 'expired'].includes(attempt.view.status) || (attempt.view.status === 'failed' && !attempt.view.canReopen) || this.#closed || this.#owners.get(attempt.owner)?.closed === true }
  async #isReferenced(name: string): Promise<boolean> { return (await this.#options.store.subscriptions()).some(row => row.subscription.credentialRef.name === name) }
  async #cleanupDraft(attempt: Attempt): Promise<void> {
    if (attempt.view.status === 'saved' || !attempt.credential) return
    const file = attempt.credential
    const saved = (await this.#options.store.subscriptions()).find(row => row.subscription.credentialRef.name === file.name)
    if (saved) {
      await attempt.client.setAliases(saved.subscription.provider, aliasesFor(saved.subscription.provider, saved.modelMapping, saved.subscription.modelMetadata))
      await attempt.client.setPrefix(file.name, prefixFor(saved.subscription.provider))
      await attempt.client.setDisabled(file.name, false)
    } else {
      await attempt.client.setDisabled(file.name, true).catch(() => undefined)
      await attempt.client.deleteCredential(file.name).catch(() => undefined)
      this.#details.delete(file.name)
    }
  }
  async #collectCancelledCredential(attempt: Attempt): Promise<void> {
    // The official cancel endpoint returns cancelled:false once the disk commit
    // gate is claimed. status can still be wait at that instant; keep the lease
    // and inspect its terminal result before discarding the draft.
    const cancelled = await attempt.client.cancelOAuth(attempt.state).catch(() => false)
    const deadline = Date.now() + 8_000
    do {
      const status = await attempt.client.oauthStatus(attempt.state).catch(() => undefined)
      if (status?.auth_id) {
        const file = (await attempt.client.credentials()).find(file => file.id === status.auth_id || file.name === status.auth_id)
        if (file) attempt.credential = file
        return
      }
      if (cancelled || status?.status !== 'wait') return
      await delay(50)
    } while (Date.now() < deadline)
  }
  async #cancel(attempt: Attempt): Promise<SubscriptionLoginView> {
    if (attempt.view.status === 'saved') return structuredClone(attempt.view)
    attempt.view.status = 'cancelled'
    attempt.browserOpenRevision++
    delete attempt.view.canReopen
    attempt.view.reasonLocalized = mapSubscriptionLoginFailure(undefined, 'cancelled')
    delete attempt.view.details
    try {
      await attempt.flow.cancel()
      await this.#collectCancelledCredential(attempt)
      await this.#cleanupDraft(attempt)
    } finally { attempt.lease.release() }
    return structuredClone(attempt.view)
  }
  async #save(owner: string, input: SubscriptionProviderConfigInput): Promise<ProviderConfigView> {
    this.#assertSupported()
    if (!input || input.kind !== 'subscription' || input.cli !== 'claude-code' || !input.subscription || !['codex', 'antigravity'].includes(input.subscription.provider) || typeof input.name !== 'string' || input.apiKey !== undefined || input.endpoint !== undefined) throw new Error('Invalid subscription provider request')
    await this.initialize()
    return this.#serial(async () => {
      if (this.#owners.get(owner)?.closed || this.#closed) throw new Error('Subscription connection is closed')
      const provider = input.subscription.provider
      const previous = (await this.#options.store.subscriptions()).find(row => row.subscription.provider === provider)
      const attempt = input.subscription.attemptId ? this.#attempt(owner, input.subscription.attemptId) : undefined
      if (attempt && (attempt.view.status !== 'ready' || attempt.view.provider !== provider || !attempt.credential || !attempt.view.details)) throw new Error('Sign-in is not ready to save')
      if (!attempt && (!previous || input.id !== previous.id)) throw new Error('Subscription requires a completed sign-in')
      const lease = await this.#options.manager.acquire('subscription-adoption')
      const client = new RouterManagementClient(lease.connection)
      try {
        const file = attempt?.credential ?? (await client.credentials()).find(value => value.name === previous?.subscription.credentialRef.name)
        if (!file) throw new Error('Subscription credential is missing')
        const details = attempt?.view.details ?? await this.#readDetails(client, file)
        if (!details || !roles.every(role => typeof input.modelMapping?.[role] === 'string' && details.models.some(model => model.id === input.modelMapping[role]))) throw new Error('Selected model is not available')
        const mapping = previous && attempt ? previous.modelMapping : input.modelMapping
        // Preserve valid existing mappings on re-login; replace unavailable IDs
        // only with the user's newly supplied, validated candidates.
        const selected = Object.fromEntries(roles.map(role => [role, details.models.some(model => model.id === mapping[role]) ? mapping[role] : input.modelMapping[role]])) as Record<typeof roles[number], string>
        const aliases = aliasesFor(provider, selected, details.models)
        const oldAliases = await client.aliases(provider)
        const oldRef = previous?.subscription.credentialRef.name
        const replacing = oldRef && oldRef !== file.name
        let committed = false
        try {
          if (replacing) await client.setDisabled(oldRef, true)
          await client.setAliases(provider, aliases)
          await client.setPrefix(file.name, prefixFor(provider))
          await client.setDisabled(file.name, false)
          const adopted = await this.#options.manager.waitForCredentialAdoption({ name: file.name, provider, authIndex: file.auth_index })
          await waitForAliases(client, file.name, prefixFor(provider), aliases)
          if (attempt && this.#cancelled(attempt)) throw new Error('Sign-in was cancelled')
          const result = await this.#options.store.adoptSubscription({
            ...(input.id ? { id: input.id } : {}), name: previous && attempt ? previous.name : input.name,
            modelMapping: selected,
            subscription: { provider, credentialRef: { name: file.name, authIndex: adopted.authIndex }, account: details.account, modelMetadata: details.models.filter(model => Object.values(selected).includes(model.id)) }
          })
          committed = true
          this.#options.manager.rememberModelAliases(provider, aliases)
          this.#details.set(file.name, details)
          this.#credentialStatus.set(file.name, details.account)
          if (attempt) { attempt.view.status = 'saved'; attempt.lease.release() }
          if (replacing) await client.deleteCredential(oldRef).catch(() => undefined)
          return result
        } catch (error) {
          if (!committed) {
            await client.setAliases(provider, oldAliases).catch(() => undefined)
            if (replacing) await client.setDisabled(oldRef, false).catch(() => undefined)
            if (!oldRef || oldRef !== file.name) { await client.setDisabled(file.name, true).catch(() => undefined); await client.setPrefix(file.name, '').catch(() => undefined) }
          }
          throw error
        }
      } finally { lease.release() }
    })
  }
  async delete(cli: ProviderCli, providerId: string): Promise<ProviderConfigSnapshot> {
    await this.initialize()
    return this.#serial(async () => {
      const snapshot = await this.#options.store.snapshot()
      if (snapshot.activeProviderIds[cli] === providerId) throw new Error('Active provider cannot be deleted')
      const saved = (await this.#options.store.subscriptions()).find(row => row.id === providerId)
      if (!saved || cli !== 'claude-code') return this.#options.store.delete(cli, providerId)
      this.#assertSupported()
      const lease = await this.#options.manager.acquire('subscription-delete')
      const client = new RouterManagementClient(lease.connection)
      try {
        const credentialExists = (await client.credentials()).some(file => file.name === saved.subscription.credentialRef.name)
        if (credentialExists) await client.setDisabled(saved.subscription.credentialRef.name, true)
        let result: ProviderConfigSnapshot
        try { result = await this.#options.store.delete(cli, providerId) }
        catch (error) { if (credentialExists) await client.setDisabled(saved.subscription.credentialRef.name, false).catch(() => undefined); throw error }
        this.#details.delete(saved.subscription.credentialRef.name)
        this.#credentialStatus.delete(saved.subscription.credentialRef.name)
        this.#options.manager.rememberModelAliases(saved.subscription.provider, [])
        await client.setAliases(saved.subscription.provider, []).catch(() => undefined)
        await client.deleteCredential(saved.subscription.credentialRef.name).catch(() => undefined)
        return result
      } finally { lease.release() }
    })
  }
  async #settings(id: string): Promise<SubscriptionSettingsSnapshot> {
    await this.initialize()
    const owner = this.#owners.get(id)
    if (!owner || owner.closed) throw new Error('Subscription connection is closed')
    if (!this.#subscriptionSupported) {
      const rows = await this.#options.store.subscriptions()
      return { snapshot: await this.snapshot(), accounts: Object.fromEntries(rows.map(row =>
        [row.id, { account: row.subscription.account, models: row.subscription.modelMetadata }])),
        antigravityRiskAccepted: await this.#options.store.antigravityRiskAccepted() }
    }
    if (!owner.round) {
      const round: SettingsRound = { ready: Promise.resolve(), accounts: new Map() }
      owner.round = round
      round.ready = (async () => {
        const lease = await this.#options.manager.openSettings(id)
        round.lease = lease
        if (owner.closed || owner.round !== round) { lease.release(); return }
        const client = new RouterManagementClient(lease.connection)
        const credentials = await client.credentials()
        const rows = await this.#options.store.subscriptions()
        await Promise.all(rows.map(async row => {
          const file = credentials.find(file => file.name === row.subscription.credentialRef.name)
          const statusAtRead = this.#credentialStatus.get(row.subscription.credentialRef.name)
          const details = file ? await this.#readDetails(client, file) : { account: { ...row.subscription.account, status: 'needs-sign-in' as const }, models: [] }
          round.accounts.set(row.id, { ref: row.subscription.credentialRef.name, details })
          // A slow settings read must not erase a newer activation failure.
          if (this.#credentialStatus.get(row.subscription.credentialRef.name) === statusAtRead) this.#credentialStatus.set(row.subscription.credentialRef.name, details.account)
        }))
      })().catch(async () => {
        for (const row of await this.#options.store.subscriptions()) {
          const details: SubscriptionAccountDetails = { account: { ...row.subscription.account, status: 'router-not-running' }, models: row.subscription.modelMetadata }
          round.accounts.set(row.id, { ref: row.subscription.credentialRef.name, details })
          if (this.#credentialStatus.get(row.subscription.credentialRef.name)?.status !== 'needs-sign-in') this.#credentialStatus.set(row.subscription.credentialRef.name, details.account)
        }
      })
    }
    const round = owner.round
    await round.ready
    if (owner.closed || owner.round !== round) throw new Error('Settings page was closed')
    const rows = await this.#options.store.subscriptions()
    const accounts = Object.fromEntries(rows.map(row => {
      const cached = round.accounts.get(row.id)
      const details = cached?.ref === row.subscription.credentialRef.name ? cached.details : this.#details.get(row.subscription.credentialRef.name)
      return [row.id, details ?? { account: row.subscription.account, models: [] }]
    }))
    return structuredClone({ snapshot: await this.snapshot(), accounts, antigravityRiskAccepted: await this.#options.store.antigravityRiskAccepted() })
  }
  async #closeSettings(id: string): Promise<void> {
    const owner = this.#owners.get(id)
    const round = owner?.round
    if (owner) delete owner.round
    round?.lease?.release()
    for (const attempt of this.#attempts.values()) if (attempt.owner === id && !['saved', 'cancelled', 'expired'].includes(attempt.view.status)) await this.#serial(() => this.#cancel(attempt))
  }
  async #reloadModels(owner: string, input: SubscriptionModelsReadInput): Promise<SubscriptionAccountDetails> {
    this.#assertSupported()
    const hasAttempt = typeof input?.attemptId === 'string' && Boolean(input.attemptId)
    const hasProvider = typeof input?.providerId === 'string' && Boolean(input.providerId)
    if (hasAttempt === hasProvider) throw new Error('Exactly one model source is required')
    const attempt = hasAttempt ? this.#attempt(owner, input.attemptId!) : undefined
    if (attempt && (attempt.view.status !== 'ready' || !attempt.credential)) throw new Error('Signed-in account is not ready')
    const saved = hasProvider ? (await this.#options.store.subscriptions()).find(row => row.id === input.providerId) : undefined
    const name = attempt?.credential?.name ?? saved?.subscription.credentialRef.name
    if (!name) throw new Error('Subscription account is not available')
    const lease = await this.#options.manager.acquire('subscription-models-read')
    try {
      const client = new RouterManagementClient(lease.connection)
      const file = (await client.credentials()).find(file => file.name === name)
      if (!file) throw new Error('Signed-in credential is not available')
      const models = await this.#readModels(client, file, typeof file.project_id === 'string' ? file.project_id : undefined)
      return await this.#serial(async () => {
        if (this.#owners.get(owner)?.closed || this.#closed || (attempt && attempt.view.status !== 'ready')) throw new Error('Subscription model read was cancelled')
        const previous = attempt?.view.details ?? this.#details.get(name)
        const details: SubscriptionAccountDetails = { ...(previous ?? { account: saved!.subscription.account }), models }
        if (attempt) attempt.view.details = details
        this.#details.set(name, details)
        return structuredClone(details)
      })
    } finally { lease.release() }
  }
  async #readModels(client: RouterManagementClient, file: RouterCredential, project?: string): Promise<SubscriptionModel[]> {
    const safe = <T>(operation: Promise<T>) => operation.catch(() => undefined)
    const [response, registered] = await Promise.all([
      safe(client.apiCall(file.provider === 'codex' ? 'chatgpt-models' : 'antigravity-models', file, project)),
      safe(client.modelDefinitions(file.provider))
    ])
    const models = readChatGptModels(response)
    return availableModels(models.ok ? models.value : undefined, registered ?? [], file.provider)
  }
  async #readDetails(client: RouterManagementClient, file: RouterCredential): Promise<SubscriptionAccountDetails> {
    const parsed = parseCredentialSummary(file)
    const summary = parsed.ok ? parsed.value : {}
    const mapped = mapAccountStatus({ ...summary, source: 'credential' })
    const account: SubscriptionAccountSummary = {
      status: mapped.status === 'needs-sign-in' || mapped.status === 'account-unavailable' ? mapped.status : 'ready',
      ...(summary.email ? { email: summary.email } : {}),
      ...(summary.plan ? { plan: file.provider === 'codex' ? parseChatGptPlan(summary.plan) ?? summary.plan : summary.plan } : {}),
      ...(mapped.reason ? { reason: mapped.reason } : {})
    }
    const safe = <T>(operation: Promise<T>) => operation.catch(() => undefined)
    if (file.provider === 'codex') {
      const [usageResponse, models] = await Promise.all([safe(client.apiCall('chatgpt-usage', file)), this.#readModels(client, file)])
      const usage = readChatGptUsage(usageResponse)
      if (usage.ok && usage.value.plan) account.plan = usage.value.plan
      return { account, models, ...(usage.ok && usage.value.windows.length ? { quota: usage.value.windows } : {}) }
    }
    const response = readAntigravityAccount(await safe(client.apiCall('antigravity-account', file)))
    const project = response.ok ? response.value.project ?? undefined : typeof file.project_id === 'string' ? file.project_id : undefined
    if (response.ok) {
      if (response.value.plan) account.plan = response.value.plan
      if (response.value.eligibility === 'ineligible') { account.status = 'account-unavailable'; if (response.value.reason) account.reason = response.value.reason }
    }
    const [quotaResponse, models] = await Promise.all([
      project ? safe(client.apiCall('antigravity-quota', file, project)) : Promise.resolve(undefined),
      this.#readModels(client, file, project)
    ])
    const quota = readAntigravityQuota(quotaResponse)
    return { account, models, ...(quota.ok ? { quota: quota.value.map(value => ({ name: value.name, group: value.group, usedPercent: value.usedPercent, ...(value.resetsAt ? { resetsAt: value.resetsAt } : {}) })) } : {}) }
  }
  async #reconcile(): Promise<void> {
    const rows = await this.#options.store.subscriptions()
    for (const row of rows) this.#options.manager.rememberModelAliases(row.subscription.provider, aliasesFor(row.subscription.provider, row.modelMapping, row.subscription.modelMetadata))
    const files = await readdir(join(this.#options.dataRoot, 'model-router/credentials')).catch(() => [] as string[])
    if (!rows.length && !files.some(file => file.endsWith('.json'))) return
    const lease = await this.#options.manager.acquire('subscription-reconciliation')
    const client = new RouterManagementClient(lease.connection)
    try {
      const credentials = await client.credentials()
      for (const file of credentials) {
        const row = rows.find(row => row.subscription.credentialRef.name === file.name)
        if (!row) {
          await client.setDisabled(file.name, true)
          await client.deleteCredential(file.name).catch(() => undefined)
        } else {
          await client.setPrefix(file.name, prefixFor(row.subscription.provider))
          await client.setAliases(row.subscription.provider, aliasesFor(row.subscription.provider, row.modelMapping, row.subscription.modelMetadata))
          await client.setDisabled(file.name, false)
        }
      }
      for (const row of rows) if (!credentials.some(file => file.name === row.subscription.credentialRef.name)) this.#credentialStatus.set(row.subscription.credentialRef.name, { ...row.subscription.account, status: 'needs-sign-in' })
    } finally { lease.release() }
  }
  async close(): Promise<void> {
    this.#closed = true
    this.#unsubscribeRouter()
    this.#changeListeners.clear()
    for (const owner of this.#owners.values()) owner.closed = true
    for (const id of this.#owners.keys()) await this.#closeSettings(id).catch(() => undefined)
    await Promise.all([...this.#attempts.values()].map(attempt => attempt.done))
    await this.#queue
    this.#owners.clear()
  }
}

function availableModels(models: SubscriptionModel[] | undefined, registered: Array<Record<string, unknown>>, provider: SubscriptionProvider): SubscriptionModel[] {
  const prefix = `${prefixFor(provider)}/`
  const parsed = parseChatGptModels({ models: registered.map(model => ({ ...model, id: typeof model.id === 'string' && model.id.startsWith(prefix) ? model.id.slice(prefix.length) : model.id })) })
  if (!parsed.ok) return []
  const definitions = new Map(parsed.value.filter(model => !/image|video|codex-auto-review|embedding|rerank|moderation|(?:^|[-_/])(?:tts|whisper|transcribe)(?:$|[-_/])/i.test(model.id)).map(model => [model.id, model]))
  const candidates = models ?? [...definitions.values()]
  return candidates.filter(model => definitions.has(model.id)).map(model => {
    const definition = definitions.get(model.id)!
    // Codex definitions are the routing engine's capability contract. The
    // account endpoint may carry different limits and lengthy instructions.
    const metadata = provider === 'codex' ? definition : model
    return {
      id: model.id, ...(metadata.displayName ? { displayName: metadata.displayName } : {}),
      ...(metadata.ownedBy ? { ownedBy: provider === 'antigravity' && metadata.ownedBy === 'Codex' ? 'Antigravity' : metadata.ownedBy } : {}),
      ...(metadata.contextWindow ? { contextWindow: metadata.contextWindow } : {}),
      ...(metadata.maxOutputTokens ? { maxOutputTokens: metadata.maxOutputTokens } : {})
    }
  })
}
function aliasesFor(provider: SubscriptionProvider, mapping: Record<typeof roles[number], string>, models: SubscriptionModel[]): ModelAlias[] {
  const aliases = roles.map(role => {
    const model = models.find(model => model.id === mapping[role])
    if (!model) throw new Error('Selected model metadata is missing')
    return { name: model.id.replace(/\[1m\]$/i, ''), alias: toClientModelId(provider === 'codex' ? 'chatgpt' : 'antigravity', role, model).replace(/\[1m\]$/i, ''), fork: true as const }
  })
  aliases.push({ name: mapping.opus.replace(/\[1m\]$/i, ''), alias: FABLE_FALLBACK_ALIAS, fork: true })
  return [...new Map(aliases.map(alias => [alias.alias, alias])).values()]
}
function sameAliases(left: ModelAlias[], right: ModelAlias[]): boolean {
  return left.length === right.length && right.every(alias => left.some(current =>
    current.name === alias.name && current.alias === alias.alias && current.fork === alias.fork))
}
function hasAliases(models: Array<Record<string, unknown>>, prefix: string, aliases: ModelAlias[]): boolean {
  const ids = new Set(models.map(model => model.id))
  return aliases.every(alias => ids.has(`${prefix}/${alias.alias}`))
}
async function waitForAliases(client: RouterManagementClient, name: string, prefix: string, aliases: ModelAlias[]): Promise<void> {
  const deadline = Date.now() + 8_000
  while (Date.now() < deadline) {
    if (hasAliases(await client.models(name), prefix, aliases)) return
    await delay(50)
  }
  throw new SubscriptionLaunchError('Account model aliases were not registered before the deadline', 'models-not-ready')
}

function credentialNeedsSignIn(file: RouterCredential): boolean {
  const parsed = parseCredentialSummary(file)
  return mapAccountStatus({ ...(parsed.ok ? parsed.value : {}), source: 'credential' }).status === 'needs-sign-in'
}
