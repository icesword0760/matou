import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  CLI_DEFAULT_MODEL,
  type Locale, type LocalePreference,
  type ProviderCli, type ProviderConfigActivationResult, type ProviderConfigInput,
  type ProviderConfigSnapshot, type ProviderConfigView, type RpcMethod
} from '@matou/contracts'

import { APP_DISPLAY_NAME } from '../../../shared/brand'
import { messages } from '../i18n/current'
import { ProviderSetupDialog } from './ProviderSetupDialog'
import { useLocale, useMessages } from '../i18n/LocaleProvider'

export interface ProviderConfigClient {
  request(method: RpcMethod, payload: unknown): Promise<unknown>
}

/** The settings panel shows one pane per category; the nav column switches between them. */
type SettingsCategory = 'model' | 'language'

export function ModelSwitchSettings({ client, onClose }: {
  client: ProviderConfigClient | null
  onClose(): void
}) {
  const zh = useLocale() === 'zh-CN', t = (cn: string, en: string) => zh ? cn : en
  const shell = useMessages().hierarchyShell
  const m = shell.modelSettings
  const language = shell.languageSettings
  const [category, setCategory] = useState<SettingsCategory>('model')
  const [cli, setCli] = useState<ProviderCli>('claude-code')
  const [snapshot, setSnapshot] = useState<ProviderConfigSnapshot>()
  const [loading, setLoading] = useState(true)
  const [failure, setFailure] = useState('')
  const [notice, setNotice] = useState<{ kind: 'success' | 'error'; text: string }>()
  const [editor, setEditor] = useState<{ existing?: ProviderConfigView }>()
  const [switching, setSwitching] = useState('')
  const [transitionResult, setTransitionResult] = useState<ProviderConfigActivationResult>()
  const [showSessions, setShowSessions] = useState(false)
  const operation = useRef(false)
  const revision = useRef(-1)
  const accept = (next: ProviderConfigSnapshot) => {
    if (next.revision < revision.current) return
    revision.current = next.revision; setSnapshot(next)
  }

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

  useEffect(() => { void refresh() }, [refresh])
  useEffect(() => {
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || editor || operation.current) return
      onClose()
    }
    window.addEventListener('keydown', onEscape)
    return () => window.removeEventListener('keydown', onEscape)
  }, [editor, onClose])
  useEffect(() => {
    if (typeof BroadcastChannel === 'undefined') return
    const channel = new BroadcastChannel('matou-provider-config')
    channel.onmessage = () => { void refresh() }
    return () => channel.close()
  }, [refresh])

  const providers = snapshot?.providers[cli] ?? []
  const activeId = snapshot?.activeProviderIds[cli]
  const announceChange = () => {
    if (typeof BroadcastChannel === 'undefined') return
    const channel = new BroadcastChannel('matou-provider-config')
    channel.postMessage({ changed: true }); channel.close()
  }
  const activate = async (provider: ProviderConfigView) => {
    if (!client || operation.current) return
    if (provider.id === activeId) { setEditor(undefined); return }
    operation.current = true; setSwitching(provider.id); setNotice(undefined); setTransitionResult(undefined)
    try {
      const next = await client.request('provider-config.activate', { cli, providerId: provider.id }) as ProviderConfigActivationResult
      accept(next); announceChange(); setTransitionResult(next); setShowSessions(false)
      const transitions = next.sessionTransitions ?? []
      const updated = transitions.filter(({ status }) => status === 'updated').length
      const deferred = transitions.length - updated
      setNotice({ kind: 'success', text: cli === 'codex' ? m.switchedCodex(provider.name) : deferred > 0 ? m.switchedDeferred(provider.name, updated, deferred) : updated > 0 ? m.switchedUpdated(provider.name, updated) : m.switchedClaude(provider.name) })
      setEditor(undefined)
    } catch (error) {
      setNotice({ kind: 'error', text: message(error, m.switchFailed) })
      // A failed RPC is not proof that no server-side change happened.
      void refresh()
      throw error
    } finally { operation.current = false; setSwitching('') }
  }
  const save = async (provider: ProviderConfigInput, use: boolean) => {
    if (!client) throw new Error(m.serviceConnecting)
    const result = await client.request('provider-config.upsert', { provider }) as { provider: ProviderConfigView }
    await refresh(); announceChange(); setEditor(undefined); setTransitionResult(undefined); setShowSessions(false)
    setNotice({ kind: 'success', text: provider.id ? m.configurationSaved : t('供应商已保存，尚未切换。', 'Provider saved; not switched yet.') })
    if (use) {
      try { await activate(result.provider) }
      catch { setNotice({ kind: 'error', text: t('配置已保存，但切换未确认完成。请核对当前默认供应商后重试，无需重新添加。', 'Configuration saved, but switching was not confirmed. Check the current default and retry; do not add it again.') }) }
    }
  }
  const remove = async (provider: ProviderConfigView) => {
    if (!client) throw new Error(m.serviceConnecting)
    const next = await client.request('provider-config.delete', { cli, providerId: provider.id }) as ProviderConfigSnapshot
    accept(next); setEditor(undefined); announceChange(); setTransitionResult(undefined); setShowSessions(false); setNotice({ kind: 'success', text: m.providerDeleted })
  }
  const deferredReason = (reason: string) => ({
    'durability-fault': t('会话存储待恢复', 'Session storage needs recovery'),
    'recovery-not-ready': t('会话正在恢复', 'Session is recovering'),
    'provider-identity-pending': t('正在确认会话身份', 'Waiting for session identity'),
    'session-not-running': t('会话未运行', 'Session is not running'),
    'binding-update-failed': t('会话配置更新失败', 'Session configuration update failed'),
    'restart-unavailable': t('会话暂未重连', 'Session has not reconnected')
  }[reason] ?? t('保留原配置', 'Kept previous configuration'))

  return <section className="model-settings provider-settings" aria-label={m.region}>
    <div className="model-settings__frame">
      <nav className="model-settings__nav" aria-label={m.categories}>
        <span className="model-settings__section-label">{m.aiServices}</span>
        <button className={`model-settings__nav-item${category === 'model' ? ' is-active' : ''}`}
          type="button" aria-current={category === 'model'} onClick={() => setCategory('model')}>
          <SlidersIcon /><span>{t('服务供应商', 'Service providers')}</span>
        </button>
        <span className="model-settings__section-label">{language.general}</span>
        <button className={`model-settings__nav-item${category === 'language' ? ' is-active' : ''}`}
          type="button" aria-current={category === 'language'} disabled={Boolean(switching)} onClick={() => setCategory('language')}>
          <GlobeIcon /><span>{language.title}</span>
        </button>
        {category === 'model' && <p>{m.globalHint(APP_DISPLAY_NAME)}</p>}
      </nav>
      <main className="model-settings__main">
        <header className="model-settings__heading">
          <div>
            <h1>{category === 'model' ? t('服务供应商', 'Service providers') : language.title}</h1>
            <p>{category === 'model' ? m.heading : language.heading}</p>
          </div>
          <button type="button" aria-label={m.close} disabled={Boolean(switching)} onClick={onClose}>×</button>
        </header>
        {category === 'language' && <LanguageSettings />}
        {category === 'model' && <div className="model-settings__tabs" role="tablist" aria-label={m.cliType}>
          {([['claude-code', 'Claude Code'], ['codex', 'Codex']] as const).map(([id, label]) =>
            <button key={id} role="tab" aria-selected={cli === id} className={cli === id ? 'is-active' : ''}
              type="button" disabled={Boolean(switching)} onClick={() => { setCli(id); setFailure(''); setNotice(undefined); setTransitionResult(undefined) }}>{label}</button>)}
        </div>}
        {category === 'model' && (loading ? <div className="model-settings__state" aria-busy="true">{m.loading}</div> : <>
          {notice && <section className={`provider-result is-${notice.kind}`} role={notice.kind === 'error' ? 'alert' : 'status'}>
            <span className="provider-result__icon">{notice.kind === 'error' ? '!' : '✓'}</span><div><strong>{notice.kind === 'error' ? t('需要检查', 'Needs attention') : transitionResult ? t('默认供应商已切换', 'Default provider switched') : t('配置已保存', 'Configuration saved')}</strong><p>{notice.text}</p>
            {Boolean(transitionResult?.sessionTransitions?.length) && <button className="provider-link" type="button" onClick={() => setShowSessions(!showSessions)}>{showSessions ? t('收起会话', 'Hide sessions') : t('查看会话', 'View sessions')} →</button>}
            {showSessions && <ul className="provider-session-results">{transitionResult?.sessionTransitions?.map(row => <li key={row.sessionId}><span>{t('会话', 'Session')} {row.sessionId}</span><b>{row.status === 'updated' ? t('已更新', 'Updated') : deferredReason(row.reason)}</b></li>)}</ul>}
            </div><button className="provider-icon-button" aria-label={t('关闭提示', 'Dismiss notice')} type="button" onClick={() => { setNotice(undefined); setShowSessions(false) }}>×</button>
          </section>}
          <div className="model-settings__toolbar">
            <h2>{m.providers} <span>{m.providerCount(providers.length)}</span></h2>
            <button type="button" aria-label={m.newProvider} disabled={Boolean(switching)} onClick={() => setEditor({})}>{m.newProviderAction}</button>
          </div>
          {failure && !editor && <div className="model-settings__error" role="alert">{failure}<button type="button" onClick={() => void refresh()}>{t('重试', 'Retry')}</button></div>}
          <div className="model-settings__providers">
            {providers.map((provider) => {
              const current = provider.id === activeId
              return <article key={provider.id} className={`model-provider${current ? ' is-current' : ''}`}>
                <ProviderLogo provider={provider} />
                <div className="model-provider__name"><strong>{provider.name}{current && <i>{m.inUse}</i>}</strong><span>{shortEndpoint(provider.endpoint)}</span></div>
                <div className="model-provider__model"><strong>{modelLabel(provider.model, m.cliDefaultModel)}</strong><span>{m.defaultModel}{provider.hasApiKey ? m.keyConfigured : ''}</span></div>
                <div className="model-provider__actions">
                  <button type="button" disabled={Boolean(switching)} onClick={() => setEditor({ existing: provider })}>{m.edit}</button>
                  <button className="primary" type="button" disabled={current || Boolean(switching)}
                    aria-label={current ? m.currentConfigurationOf(provider.name) : m.switchTo(provider.name)}
                    onClick={() => { void activate(provider).catch(() => {}) }}>{switching === provider.id ? t('切换中…', 'Switching…') : current ? t('默认使用', 'Default') : m.switchAction}</button>
                </div>
              </article>
            })}
          </div>
          <p className="model-settings__impact"><span>ⓘ</span><b>{m.impactTitle(APP_DISPLAY_NAME)}</b> {m.impactBody}</p>
        </>)}
      </main>
    </div>
    {editor && <ProviderSetupDialog key={cli} cli={cli} existing={editor.existing}
      official={providers.find(p => p.builtIn)} activeId={activeId} client={client}
      onClose={() => setEditor(undefined)} onSave={save} onUseOfficial={activate} onDelete={remove} />}
  </section>
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
function tone(name: string) { return name.toLowerCase().includes('openai') ? 'openai' : name.toLowerCase().includes('anthropic') ? 'anthropic' : 'custom' }

function message(error: unknown, fallback: string) {
  return error instanceof Error
    ? messages().hierarchyShell.modelSettings.errorDetail(fallback, error.message)
    : fallback
}
