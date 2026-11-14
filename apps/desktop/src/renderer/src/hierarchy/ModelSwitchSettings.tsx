import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  CLI_DEFAULT_MODEL,
  type AgentModelSettingsSnapshot,
  type ContextWindowMode,
  type Locale, type LocalePreference,
  type ModelContextChoice,
  type ProviderCli, type ProviderActivationFailure, type ProviderConfigActivationResult, type ProviderConfigInput, type ProviderModelMetadata,
  type ProviderConfigSnapshot, type ProviderConfigView, type RpcMethod,
  type SubscriptionAccountDetails, type SubscriptionProvider, type SubscriptionSettingsSnapshot
} from '@matou/contracts'

import { APP_DISPLAY_NAME } from '../../../shared/brand'
import { messages } from '../i18n/current'
import { SubscriptionSetupDialog } from './SubscriptionSetupDialog'
import { SubscriptionProviderCard } from './SubscriptionProviderCard'
import { subscriptionQuotaText, subscriptionQuotaTitle } from './subscription-settings'
import { ProviderSetupDialog } from './ProviderSetupDialog'
import { ClaudeTimeZoneSettings } from './ClaudeTimeZoneSettings'
import { AgentModelSettings } from './AgentModelSettings'
import { formatContextWindowBadge, providerRoleModelIds } from './context-window-ui'
import { useLocale, useMessages } from '../i18n/LocaleProvider'

export interface ProviderConfigClient {
  request(method: RpcMethod, payload: unknown, options?: { timeoutMs?: number }): Promise<unknown>
  subscribeProviderConfig?(listener: () => void): () => void
}

/** The settings panel groups AI settings under Claude Code and system settings under General. */
type SettingsCategory = 'claude' | 'general'

export function ModelSwitchSettings({ client, onClose }: {
  client: ProviderConfigClient | null
  onClose(): void
}) {
  const zh = useLocale() === 'zh-CN', t = (cn: string, en: string) => zh ? cn : en
  const shell = useMessages().hierarchyShell
  const m = shell.modelSettings
  const language = shell.languageSettings
  const cli: ProviderCli = 'claude-code'
  const [category, setCategory] = useState<SettingsCategory>('claude')
  const [snapshot, setSnapshot] = useState<ProviderConfigSnapshot>()
  const [agentSettings, setAgentSettings] = useState<AgentModelSettingsSnapshot>()
  const [loading, setLoading] = useState(true)
  const [failure, setFailure] = useState('')
  const [notice, setNotice] = useState<{ kind: 'success' | 'error' | 'info'; text: string; retry?: ProviderConfigView; recovery?: ProviderActivationFailure['recovery'] }>()
  const [editor, setEditor] = useState<{ existing?: ProviderConfigView; subscriptionProvider?: SubscriptionProvider; relogin?: boolean }>()
  const [switching, setSwitching] = useState('')
  const [transitionResult, setTransitionResult] = useState<ProviderConfigActivationResult>()
  const [showSessions, setShowSessions] = useState(false)
  const [accounts, setAccounts] = useState<Record<string, SubscriptionAccountDetails>>({})
  const [riskAccepted, setRiskAccepted] = useState(false)
  // A runtime capability, not a UI preference. Storage-only mutation results
  // omit it, so remember the latest actual boolean instead of clearing it.
  const [subscriptionSupported, setSubscriptionSupported] = useState(true)
  const loginDetails = useRef<SubscriptionAccountDetails | undefined>(undefined)
  const dialogContextMode = useRef<{ initial: ContextWindowMode; current: ContextWindowMode; touched: boolean }>({
    initial: 'default', current: 'default', touched: false
  })
  const settingsCloser = useRef<() => void>(() => {})
  const requestClose = () => { settingsCloser.current(); onClose() }
  const operation = useRef(false)
  const revision = useRef(-1)
  const accept = (next: ProviderConfigSnapshot) => {
    // The capability is runtime-wide, so learn it even from an older
    // storage-only response; only the provider list is revision-gated.
    if (typeof next.subscriptionSupported === 'boolean') setSubscriptionSupported(next.subscriptionSupported)
    if (next.revision < revision.current) return
    revision.current = next.revision; setSnapshot(next)
  }

  const refreshAgentSettings = useCallback(async () => {
    if (!client) return
    try {
      const result = await client.request('agent-model-settings.get', {})
      if (isAgentModelSettingsSnapshot(result)) setAgentSettings(result)
    } catch { /* Provider list stays usable even if context metadata cannot be loaded. */ }
  }, [client])

  const refresh = useCallback(async () => {
    if (!client) {
      setLoading(false)
      setFailure(m.serviceConnecting)
      return
    }
    try {
      const next = await client.request('provider-config.snapshot', {}) as ProviderConfigSnapshot
      accept(next); setFailure('')
    } catch (error) {
      setFailure(message(error, m.loadFailed))
    } finally {
      setLoading(false)
    }
  }, [client, m])

  useEffect(() => { void refresh(); void refreshAgentSettings() }, [refresh, refreshAgentSettings])
  useEffect(() => {
    if (!client) return
    let live = true, closed = false
    const close = () => { if (closed) return; closed = true; void client.request('provider-config.subscription.settings-close', {}).catch(() => {}) }
    settingsCloser.current = close
    void client.request('provider-config.subscription.settings-read', {}).then(value => {
      if (!live || closed) return
      const result = value as SubscriptionSettingsSnapshot
      if (!result.snapshot) return
      accept(result.snapshot); setAccounts(current => ({ ...result.accounts, ...current })); setRiskAccepted(current => current || result.antigravityRiskAccepted)
      void refreshAgentSettings()
    }).catch(() => { /* Quota reads never block the provider list or switching. */ })
    return () => { live = false; close() }
  }, [client, refreshAgentSettings])
  useEffect(() => client?.subscribeProviderConfig?.(() => { void refresh(); void refreshAgentSettings() }), [client, refresh, refreshAgentSettings])
  useEffect(() => {
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || editor || operation.current) return
      settingsCloser.current(); onClose()
    }
    window.addEventListener('keydown', onEscape)
    return () => window.removeEventListener('keydown', onEscape)
  }, [editor, onClose])
  useEffect(() => {
    if (typeof BroadcastChannel === 'undefined') return
    const channel = new BroadcastChannel('matou-provider-config')
    channel.onmessage = () => { void refresh(); void refreshAgentSettings() }
    return () => channel.close()
  }, [refresh, refreshAgentSettings])

  const rawProviders = snapshot?.providers[cli] ?? []
  const activeId = snapshot?.activeProviderIds[cli]
  const providers = useMemo(
    () => [...rawProviders].sort((a, b) => (a.id === activeId ? -1 : b.id === activeId ? 1 : 0)),
    [rawProviders, activeId]
  )
  const official = providers.find(provider => provider.builtIn)

  const initialDialogContextMode = (existing?: ProviderConfigView, subscriptionProvider?: SubscriptionProvider): ContextWindowMode => {
    if (existing && agentSettings?.preferences.contextWindows[existing.id]) {
      return agentSettings.preferences.contextWindows[existing.id]!
    }
    if (existing?.subscription?.provider === 'codex' || subscriptionProvider === 'codex') return '512k'
    return 'default'
  }

  const openEditor = (next: { existing?: ProviderConfigView; subscriptionProvider?: SubscriptionProvider; relogin?: boolean }) => {
    const initialMode = initialDialogContextMode(next.existing, next.subscriptionProvider)
    dialogContextMode.current = { initial: initialMode, current: initialMode, touched: false }
    setEditor(next)
  }

  const signIn = (provider: ProviderConfigView) => {
    const current = providers.find(row => row.id === provider.id) ?? provider
    if (!current.subscription) return
    loginDetails.current = undefined
    openEditor({ existing: current, subscriptionProvider: current.subscription.provider, relogin: true })
  }
  const announceChange = () => {
    if (typeof BroadcastChannel === 'undefined') return
    const channel = new BroadcastChannel('matou-provider-config')
    channel.postMessage({ changed: true }); channel.close()
  }
  const activate = async (provider: ProviderConfigView, reapply = false) => {
    if (!client || operation.current) return
    if (provider.id === activeId && !reapply) { setEditor(undefined); return }
    const revisionBeforeActivate = revision.current
    operation.current = true; setSwitching(provider.id); setNotice(undefined); setTransitionResult(undefined)
    try {
      const next = await client.request('provider-config.activate', { cli, providerId: provider.id }) as ProviderConfigActivationResult
      accept(next); announceChange(); setTransitionResult(next); setShowSessions(false)
      const transitions = next.sessionTransitions ?? []
      const updated = transitions.filter(({ status }) => status === 'updated').length
      const deferred = transitions.length - updated
      setNotice({ kind: 'success', text: deferred > 0 ? m.switchedDeferred(provider.name, updated, deferred) : updated > 0 ? m.switchedUpdated(provider.name, updated) : m.switchedClaude(provider.name) })
      setEditor(undefined)
    } catch (error) {
      const failure = providerActivationFailure(error)
      if (failure) {
        setNotice({ kind: 'error', text: failure.message[zh ? 'zh' : 'en'], retry: provider, recovery: failure.recovery })
        // A failed RPC is not proof that no server-side change happened.
        void refresh()
        throw error
      }
      try {
        const latest = await client.request('provider-config.snapshot', {}) as ProviderConfigSnapshot
        accept(latest)
        if (latest.activeProviderIds[cli] === provider.id && latest.revision > revisionBeforeActivate && latest.revision >= revision.current) {
          announceChange()
          setNotice({
            kind: 'info',
            text: t(
              `默认供应商已切换为 ${provider.name}；现有会话更新结果尚未确认，请检查会话状态。`,
              `Default provider switched to ${provider.name}. The update result for existing sessions is not confirmed yet; check the session status.`
            )
          })
          setEditor(undefined)
          return
        }
      } catch { /* A failed confirmation read keeps the original activation error. */ }
      setNotice({ kind: 'error', text: message(error, m.switchFailed), retry: provider, recovery: 'retry' })
      throw error
    } finally { operation.current = false; setSwitching('') }
  }
  const save = async (provider: ProviderConfigInput, use: boolean) => {
    if (!client) throw new Error(m.serviceConnecting)
    const result = await client.request('provider-config.upsert', { provider }) as { provider: ProviderConfigView }
    if (provider.kind === 'subscription' && loginDetails.current) {
      const details = loginDetails.current
      setAccounts(current => ({ ...current, [result.provider.id]: details }))
    }
    const { touched, current: nextWindowMode } = dialogContextMode.current
    if (touched) {
      const latestRaw = await client.request('agent-model-settings.get', {}).catch(() => agentSettings)
      const latest = isAgentModelSettingsSnapshot(latestRaw) ? latestRaw : agentSettings
      if (latest) {
        const updated = await client.request('agent-model-settings.set', {
          ...latest.preferences,
          contextWindows: { ...latest.preferences.contextWindows, [result.provider.id]: nextWindowMode }
        })
        if (isAgentModelSettingsSnapshot(updated)) setAgentSettings(updated)
      }
    }
    await refresh(); await refreshAgentSettings(); announceChange(); setEditor(undefined); setTransitionResult(undefined); setShowSessions(false)
    setNotice({ kind: 'success', text: provider.id ? m.configurationSaved : t('供应商已保存，尚未切换。', 'Provider saved; not switched yet.') })
    if (use) {
      try { await activate(result.provider, true) }
      catch { /* activate preserves the runtime reason and retry action. */ }
    }
  }
  const remove = async (provider: ProviderConfigView) => {
    if (!client) throw new Error(m.serviceConnecting)
    const next = await client.request('provider-config.delete', { cli, providerId: provider.id }) as ProviderConfigSnapshot
    accept(next); setEditor(undefined); announceChange(); setTransitionResult(undefined); setShowSessions(false); setNotice({ kind: 'success', text: m.providerDeleted })
    void refreshAgentSettings()
  }
  const deferredReason = (reason: string) => ({
    'durability-fault': t('会话存储待恢复', 'Session storage needs recovery'),
    'recovery-not-ready': t('会话正在恢复', 'Session is recovering'),
    'provider-identity-pending': t('正在确认会话身份', 'Waiting for session identity'),
    'session-not-running': t('会话未运行', 'Session is not running'),
    'binding-update-failed': t('会话配置更新失败', 'Session configuration update failed'),
    'restart-unavailable': t('会话暂未重连', 'Session has not reconnected')
  }[reason] ?? t('保留原配置', 'Kept previous configuration'))

  const modelsForProvider = (provider: ProviderConfigView): ModelContextChoice[] => {
    const fromAgent = agentSettings?.providers.find(row => row.id === provider.id)?.models
    const cached = accounts[provider.id]?.models ?? provider.subscription?.modelMetadata
    const toChoices = (models: readonly ProviderModelMetadata[]): ModelContextChoice[] => models.map(model => ({
      id: model.id,
      name: model.displayName ?? model.id,
      ...(model.contextWindow ? { defaultTokens: model.contextWindow } : {}),
      ...(model.maxContextWindow ? { maximumTokens: model.maxContextWindow } : {}),
      fixedTokens: 512_000
    }))
    if (provider.kind !== 'subscription') {
      if (provider.modelMetadata !== undefined) return toChoices(provider.modelMetadata)
      if (provider.builtIn && !isOfficialEndpoint(provider.cli, provider.endpoint)) return []
    }
    if (fromAgent?.length) return fromAgent
    return cached?.length ? toChoices(cached) : []
  }

  const windowBadgeFor = (provider: ProviderConfigView): string | undefined => {
    if (!agentSettings) return undefined
    const mode = agentSettings.preferences.contextWindows[provider.id] ?? (provider.subscription?.provider === 'codex' ? '512k' : 'default')
    return formatContextWindowBadge(mode, modelsForProvider(provider), providerRoleModelIds(provider), zh)
  }

  return <section className="model-settings provider-settings" aria-label={m.region}>
    <div className="model-settings__frame">
      <nav className="model-settings__nav" aria-label={m.categories}>
        <span className="model-settings__section-label">{m.aiServices}</span>
        <button className={`model-settings__nav-item${category === 'claude' ? ' is-active' : ''}`}
          type="button" aria-current={category === 'claude'} disabled={Boolean(switching)} onClick={() => setCategory('claude')}>
          <SlidersIcon /><span>Claude Code</span>
        </button>
        <span className="model-settings__section-label">{language.general}</span>
        <button className={`model-settings__nav-item${category === 'general' ? ' is-active' : ''}`}
          type="button" aria-current={category === 'general'} disabled={Boolean(switching)} onClick={() => setCategory('general')}>
          <GlobeIcon /><span>{language.title}</span>
        </button>
        {category === 'claude' && <p>{m.globalHint(APP_DISPLAY_NAME)}</p>}
      </nav>
      <main className="model-settings__main">
        <header className="model-settings__heading">
          <div>
            <h1>{category === 'claude' ? 'Claude Code' : language.title}</h1>
            <p>{category === 'claude' ? m.heading : language.heading}</p>
          </div>
          <button type="button" aria-label={m.close} disabled={Boolean(switching)} onClick={requestClose}>×</button>
        </header>
        {category === 'general' && <>
          <LanguageSettings />
          <div className="model-settings__general-divider">
            <ClaudeTimeZoneSettings client={client} />
          </div>
        </>}
        {category === 'claude' && (loading ? <div className="model-settings__state" aria-busy="true">{m.loading}</div> : <>
          {notice && <section className={`provider-result is-${notice.kind}`} role={notice.kind === 'error' ? 'alert' : 'status'}>
            <span className="provider-result__icon">{notice.kind === 'error' ? '!' : notice.kind === 'info' ? 'i' : '✓'}</span><div><strong>{notice.kind === 'error' ? t('需要检查', 'Needs attention') : notice.kind === 'info' ? t('切换结果待确认', 'Switch result pending') : transitionResult ? t('默认供应商已切换', 'Default provider switched') : t('配置已保存', 'Configuration saved')}</strong><p>{notice.text}</p>{notice.retry && <button type="button" className="provider-link" disabled={Boolean(switching)} onClick={() => { if (notice.recovery === 'sign-in') signIn(notice.retry!); else void activate(notice.retry!, true).catch(() => {}) }}>{notice.recovery === 'sign-in' ? t('重新登录', 'Sign in again') : t('重试', 'Retry')}</button>}
            {Boolean(transitionResult?.sessionTransitions?.length) && <button className="provider-link" type="button" onClick={() => setShowSessions(!showSessions)}>{showSessions ? t('收起会话', 'Hide sessions') : t('查看会话', 'View sessions')} →</button>}
            {showSessions && <ul className="provider-session-results">{transitionResult?.sessionTransitions?.map(row => <li key={row.sessionId}><span>{t('会话', 'Session')} {row.sessionId}</span><b>{row.status === 'updated' ? t('已更新', 'Updated') : deferredReason(row.reason)}</b></li>)}</ul>}
            </div><button className="provider-icon-button" aria-label={t('关闭提示', 'Dismiss notice')} type="button" onClick={() => { setNotice(undefined); setShowSessions(false) }}>×</button>
          </section>}
          <div className="model-settings__toolbar">
            <h2>{m.providers} <span>{m.providerCount(providers.length)}</span></h2>
            <button type="button" aria-label={m.newProvider} disabled={Boolean(switching)} onClick={() => { loginDetails.current = undefined; openEditor({}) }}>{m.newProviderAction}</button>
          </div>
          {failure && !editor && <div className="model-settings__error" role="alert">{failure}<button type="button" onClick={() => void refresh()}>{t('重试', 'Retry')}</button></div>}
          <div className="model-settings__providers">
            {providers.map((provider) => {
              const current = provider.id === activeId
              const windowBadge = windowBadgeFor(provider)
              if (provider.kind === 'subscription' && provider.subscription) {
                const cached = accounts[provider.id]
                const details = { ...cached, account: { ...cached?.account, ...provider.subscription.account }, models: cached?.models ?? provider.subscription.modelMetadata }
                return <SubscriptionProviderCard key={provider.id} provider={provider} details={details} quotaText={subscriptionQuotaText(provider.subscription.provider, cached, zh)} quotaTitle={subscriptionQuotaTitle(cached, zh)} windowBadge={windowBadge} current={current} switching={Boolean(switching)} subscriptionSupported={subscriptionSupported}
                  onEdit={() => { loginDetails.current = undefined; openEditor({ existing: provider, subscriptionProvider: provider.subscription!.provider }) }}
                  onSignIn={() => signIn(provider)}
                  onActivate={() => { void activate(provider).catch(() => {}) }}
                  onUseOfficial={official ? () => { void activate(official).catch(() => {}) } : undefined} />
              }
              const selectedMetadata = provider.modelMetadata?.find(model => model.id === provider.model)
              return <article key={provider.id} className={`model-provider${current ? ' is-current' : ''}`}>
                <ProviderLogo provider={provider} />
                <div className="model-provider__name"><strong>{provider.name}{current && <i>{m.inUse}</i>}{windowBadge && <i className="model-provider__window-badge">{windowBadge}</i>}</strong><span>{shortEndpoint(provider.endpoint)}</span></div>
                <div className="model-provider__model"><strong>{selectedMetadata?.displayName ?? modelLabel(provider.model, m.cliDefaultModel)}</strong><span>{selectedMetadata?.displayName ? `${provider.model} · ` : ''}{m.defaultModel}{provider.hasApiKey ? m.keyConfigured : ''}</span></div>
                <div className="model-provider__actions">
                  <button type="button" disabled={Boolean(switching)} onClick={() => openEditor({ existing: provider })}>{m.edit}</button>
                  <button className="primary" type="button" disabled={current || Boolean(switching)}
                    aria-label={current ? m.currentConfigurationOf(provider.name) : m.switchTo(provider.name)}
                    onClick={() => { void activate(provider).catch(() => {}) }}>{switching === provider.id ? t('切换中…', 'Switching…') : current ? t('默认使用', 'Default') : m.switchAction}</button>
                </div>
              </article>
            })}
          </div>
          <p className="model-settings__impact"><span>ⓘ</span><span><b>{m.impactTitle(APP_DISPLAY_NAME)}</b> {m.impactBody}</span></p>
          <AgentModelSettings client={client} providerRevision={snapshot?.revision} activeProviderId={activeId} />
        </>)}
      </main>
    </div>
    {editor?.subscriptionProvider ? <SubscriptionSetupDialog key={`${editor.subscriptionProvider}-${editor.existing?.id ?? 'new'}`} provider={editor.subscriptionProvider} existing={editor.existing} relogin={editor.relogin}
      details={editor.existing ? accounts[editor.existing.id] : undefined} riskAccepted={riskAccepted} activeId={activeId} client={client}
      contextWindowMode={dialogContextMode.current.initial}
      onContextWindowChange={mode => { dialogContextMode.current = { ...dialogContextMode.current, current: mode, touched: true } }}
      onClose={() => setEditor(undefined)} onRiskAccepted={() => setRiskAccepted(true)} onDetails={details => { loginDetails.current = details }} onSave={save} onDelete={remove} />
      : editor && <ProviderSetupDialog key={cli} cli={cli} existing={editor.existing} subscriptionSupported={subscriptionSupported}
      contextWindowMode={editor.existing
        ? agentSettings?.preferences.contextWindows[editor.existing.id] ?? dialogContextMode.current.initial
        : dialogContextMode.current.initial}
      contextModels={editor.existing ? modelsForProvider(editor.existing) : undefined}
      onContextWindowChange={mode => { dialogContextMode.current = { ...dialogContextMode.current, current: mode, touched: true } }}
      onSelectSubscription={provider => { loginDetails.current = undefined; const existing = providers.find(p => p.kind === 'subscription' && p.subscription?.provider === provider); openEditor({ subscriptionProvider: provider, ...(existing ? { existing, relogin: true } : {}) }) }}
      official={providers.find(p => p.builtIn)} activeId={activeId} client={client}
      onClose={() => setEditor(undefined)} onSave={save} onUseOfficial={activate} onDelete={remove} />}
  </section>
}

function isAgentModelSettingsSnapshot(value: unknown): value is AgentModelSettingsSnapshot {
  return Boolean(
    value &&
    typeof value === 'object' &&
    'preferences' in value &&
    'providers' in value &&
    Array.isArray((value as AgentModelSettingsSnapshot).providers)
  )
}

/** The stored preference lives in the main process; the pane stays usable without a bridge. */
function LanguageSettings() {
  const m = useMessages().hierarchyShell.languageSettings
  const [preference, setPreference] = useState<LocalePreference>('system')
  const [forced, setForced] = useState<Locale>()

  useEffect(() => {
    const bridge = desktopBridge()
    if (!bridge?.getLocalePreference) return
    let active = true
    void bridge.getLocalePreference()
      .then((stored) => { if (active) setPreference(stored) })
      // A failed read keeps the default 'system' selection instead of blocking the pane.
      .catch(() => {})
    return () => { active = false }
  }, [])

  const choose = async (next: LocalePreference) => {
    setPreference(next); setForced(undefined)
    const bridge = desktopBridge()
    if (!bridge?.setLocalePreference) return
    try {
      const effective = await bridge.setLocalePreference(next)
      // MATOU_LOCALE overrides a concrete preference; 'system' never equals a locale.
      if (next !== 'system' && next !== effective) setForced(effective)
    } catch {
      // The tray menu stays available when the preference cannot be stored.
    }
  }

  const options: readonly (readonly [LocalePreference, string])[] = [
    ['system', m.followSystem], ['zh-CN', m.chinese], ['en', m.english]
  ]
  return <div className="model-settings__language">
    <div className="model-settings__language-options" role="radiogroup" aria-label={m.options}>
      {options.map(([value, label]) =>
        <label key={value} className={preference === value ? 'is-active' : ''}>
          <input type="radio" name="matou-locale-preference" value={value}
            checked={preference === value} onChange={() => void choose(value)} />
          <span>{label}</span>
        </label>)}
    </div>
    <p className="model-settings__note">{m.note}</p>
    {forced && <p className="model-settings__note" role="status">
      {m.envForced(forced === 'en' ? m.english : m.chinese)}
    </p>}
  </div>
}

function ProviderLogo({ provider }: { provider: ProviderConfigView }) {
  const initial = useMemo(() => provider.name.trim().slice(0, 1).toUpperCase() || 'P', [provider.name])
  return <span className={`model-provider__logo tone-${tone(provider.name)}`}>{initial}</span>
}
function SlidersIcon() { return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M4 7h10M18 7h2M4 17h2M10 17h10"/><circle cx="16" cy="7" r="2"/><circle cx="8" cy="17" r="2"/></svg> }
function GlobeIcon() { return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.4 2.3 3.6 5.2 3.6 8.5s-1.2 6.2-3.6 8.5c-2.4-2.3-3.6-5.2-3.6-8.5S9.6 5.8 12 3.5Z"/></svg> }
function desktopBridge() { return window.matouDesktop as Partial<typeof window.matouDesktop> | undefined }
function shortEndpoint(value: string) { return value.replace(/^https?:\/\//, '').replace(/\/$/, '') }
/** The runtime stores a sentinel when a provider keeps the model the CLI picks. */
function modelLabel(model: string, cliDefault: string) { return model === CLI_DEFAULT_MODEL ? cliDefault : model }
function isOfficialEndpoint(cli: ProviderCli, endpoint: string) {
  const official = cli === 'claude-code' ? 'https://api.anthropic.com' : 'https://api.openai.com/v1'
  return endpoint.replace(/\/+$/, '') === official
}
function tone(name: string) { return name.toLowerCase().includes('openai') ? 'openai' : name.toLowerCase().includes('anthropic') ? 'anthropic' : 'custom' }

function message(error: unknown, fallback: string) {
  return error instanceof Error
    ? messages().hierarchyShell.modelSettings.errorDetail(fallback, error.message)
    : fallback
}

function providerActivationFailure(error: unknown): ProviderActivationFailure | undefined {
  if (error && typeof error === 'object' && 'providerFailure' in error) {
    return error.providerFailure as ProviderActivationFailure | undefined
  }
  return undefined
}
