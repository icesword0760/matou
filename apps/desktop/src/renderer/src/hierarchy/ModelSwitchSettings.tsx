import { useCallback, useEffect, useMemo, useState } from 'react'

import {
  CLI_DEFAULT_MODEL,
  type ProviderCli, type ProviderConfigActivationResult, type ProviderConfigInput,
  type ProviderConfigSnapshot, type ProviderConfigView, type RpcMethod
} from '@matou/contracts'

import { APP_DISPLAY_NAME } from '../../../shared/brand'
import { messages } from '../i18n/current'
import { useMessages } from '../i18n/LocaleProvider'

export interface ProviderConfigClient {
  request(method: RpcMethod, payload: unknown): Promise<unknown>
}

interface ProviderDraft {
  id?: string
  name: string
  endpoint: string
  model: string
  apiKey: string
  builtIn?: boolean
}

const EMPTY_DRAFT: ProviderDraft = {
  name: '', endpoint: 'https://', model: '', apiKey: ''
}

export function ModelSwitchSettings({ client, onClose }: {
  client: ProviderConfigClient | null
  onClose(): void
}) {
  const m = useMessages().hierarchyShell.modelSettings
  const [cli, setCli] = useState<ProviderCli>('claude-code')
  const [snapshot, setSnapshot] = useState<ProviderConfigSnapshot>()
  const [loading, setLoading] = useState(true)
  const [failure, setFailure] = useState('')
  const [toast, setToast] = useState('')
  const [draft, setDraft] = useState<ProviderDraft>()
  const [saving, setSaving] = useState(false)

  const refresh = useCallback(async () => {
    if (!client) {
      setLoading(false)
      setFailure(m.serviceConnecting)
      return
    }
    try {
      const next = await client.request('provider-config.snapshot', {}) as ProviderConfigSnapshot
      setSnapshot(next); setFailure('')
    } catch (error) {
      setFailure(message(error, m.loadFailed))
    } finally {
      setLoading(false)
    }
  }, [client, m])

  useEffect(() => { void refresh() }, [refresh])
  useEffect(() => {
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      draft ? setDraft(undefined) : onClose()
    }
    window.addEventListener('keydown', onEscape)
    return () => window.removeEventListener('keydown', onEscape)
  }, [draft, onClose])
  useEffect(() => {
    if (typeof BroadcastChannel === 'undefined') return
    const channel = new BroadcastChannel('matou-provider-config')
    channel.onmessage = () => { void refresh() }
    return () => channel.close()
  }, [refresh])
  useEffect(() => {
    if (!toast) return
    const timer = window.setTimeout(() => setToast(''), 2_400)
    return () => window.clearTimeout(timer)
  }, [toast])

  const providers = snapshot?.providers[cli] ?? []
  const activeId = snapshot?.activeProviderIds[cli]
  const active = providers.find(({ id }) => id === activeId)
  const announceChange = () => {
    if (typeof BroadcastChannel === 'undefined') return
    const channel = new BroadcastChannel('matou-provider-config')
    channel.postMessage({ changed: true }); channel.close()
  }
  const activate = async (provider: ProviderConfigView) => {
    if (!client || provider.id === activeId) return
    try {
      const next = await client.request('provider-config.activate', {
        cli, providerId: provider.id
      }) as ProviderConfigActivationResult
      setSnapshot(next); announceChange()
      if (cli === 'claude-code') {
        const transitions = next.sessionTransitions ?? []
        const updated = transitions.filter(({ status }) => status === 'updated').length
        const deferred = transitions.length - updated
        setToast(deferred > 0
          ? m.switchedDeferred(provider.name, updated, deferred)
          : updated > 0
            ? m.switchedUpdated(provider.name, updated)
            : m.switchedClaude(provider.name))
      } else {
        setToast(m.switchedCodex(provider.name))
      }
    } catch (error) {
      setToast(message(error, m.switchFailed))
    }
  }
  const save = async () => {
    if (!client || !draft || saving) return
    const validation = validateDraft(draft)
    if (validation) { setFailure(validation); return }
    setSaving(true); setFailure('')
    const provider: ProviderConfigInput = {
      ...(draft.id ? { id: draft.id } : {}), cli,
      name: draft.name.trim(), endpoint: draft.endpoint.trim(),
      model: draft.model.trim() || '__cli_default__',
      ...(draft.apiKey.trim() ? { apiKey: draft.apiKey.trim() } : {})
    }
    try {
      await client.request('provider-config.upsert', { provider })
      setDraft(undefined); await refresh(); announceChange()
      setToast(draft.id ? m.configurationSaved : m.providerAdded)
    } catch (error) {
      setFailure(message(error, m.saveFailed))
    } finally {
      setSaving(false)
    }
  }
  const remove = async () => {
    if (!client || !draft?.id || draft.builtIn || saving) return
    setSaving(true); setFailure('')
    try {
      const next = await client.request('provider-config.delete', {
        cli, providerId: draft.id
      }) as ProviderConfigSnapshot
      setSnapshot(next); setDraft(undefined); announceChange(); setToast(m.providerDeleted)
    } catch (error) {
      setFailure(message(error, m.deleteFailed))
    } finally { setSaving(false) }
  }

  return <section className="model-settings" aria-label={m.region}>
    <div className="model-settings__frame">
      <nav className="model-settings__nav" aria-label={m.categories}>
        <span className="model-settings__section-label">{m.aiServices}</span>
        <button className="model-settings__nav-item is-active" type="button">
          <SlidersIcon /><span>{m.modelSwitch}</span>
        </button>
        <p>{m.globalHint(APP_DISPLAY_NAME)}</p>
      </nav>
      <main className="model-settings__main">
        <header className="model-settings__heading">
          <div><h1>{m.modelSwitch}</h1><p>{m.heading}</p></div>
          <button type="button" aria-label={m.close} onClick={onClose}>×</button>
        </header>
        <div className="model-settings__tabs" role="tablist" aria-label={m.cliType}>
          {([['claude-code', 'Claude Code'], ['codex', 'Codex']] as const).map(([id, label]) =>
            <button key={id} role="tab" aria-selected={cli === id} className={cli === id ? 'is-active' : ''}
              type="button" onClick={() => { setCli(id); setFailure('') }}>{label}</button>)}
        </div>
        {loading ? <div className="model-settings__state" aria-busy="true">{m.loading}</div> : <>
          {active && <section className="model-settings__current" aria-label={m.activeProvider}>
            <ProviderLogo provider={active} />
            <div><strong>{active.name}</strong><span>{modelLabel(active.model, m.cliDefaultModel)} · {shortEndpoint(active.endpoint)}</span></div>
            <b>{m.inGlobalUse}</b>
          </section>}
          <div className="model-settings__toolbar">
            <h2>{m.providers} <span>{m.providerCount(providers.length)}</span></h2>
            <button type="button" aria-label={m.newProvider} onClick={() => setDraft({ ...EMPTY_DRAFT })}>{m.newProviderAction}</button>
          </div>
          {failure && !draft && <div className="model-settings__error" role="alert">{failure}</div>}
          <div className="model-settings__providers">
            {providers.map((provider) => {
              const current = provider.id === activeId
              return <article key={provider.id} className={`model-provider${current ? ' is-current' : ''}`}>
                <ProviderLogo provider={provider} />
                <div className="model-provider__name"><strong>{provider.name}{current && <i>{m.inUse}</i>}</strong><span>{shortEndpoint(provider.endpoint)}</span></div>
                <div className="model-provider__model"><strong>{modelLabel(provider.model, m.cliDefaultModel)}</strong><span>{m.defaultModel}{provider.hasApiKey ? m.keyConfigured : ''}</span></div>
                <div className="model-provider__actions">
                  <button type="button" onClick={() => setDraft({
                    id: provider.id, name: provider.name, endpoint: provider.endpoint,
                    model: provider.builtIn && provider.model === CLI_DEFAULT_MODEL ? '' : provider.model,
                    apiKey: '', ...(provider.builtIn ? { builtIn: true } : {})
                  })}>{m.edit}</button>
                  <button className="primary" type="button" disabled={current}
                    aria-label={current ? m.currentConfigurationOf(provider.name) : m.switchTo(provider.name)}
                    onClick={() => void activate(provider)}>{current ? m.currentConfiguration : m.switchAction}</button>
                </div>
              </article>
            })}
          </div>
          <p className="model-settings__impact"><span>ⓘ</span><b>{m.impactTitle(APP_DISPLAY_NAME)}</b> {m.impactBody}</p>
        </>}
      </main>
    </div>
    {draft && <ProviderDialog cli={cli} draft={draft} failure={failure} saving={saving}
      onChange={setDraft} onCancel={() => { setDraft(undefined); setFailure('') }}
      onSave={() => void save()} onDelete={() => void remove()} />}
    {toast && <div className="model-settings__toast" role="status"><i>✓</i>{toast}</div>}
  </section>
}

function ProviderDialog({ cli, draft, failure, saving, onChange, onCancel, onSave, onDelete }: {
  cli: ProviderCli; draft: ProviderDraft; failure: string; saving: boolean
  onChange(value: ProviderDraft): void; onCancel(): void; onSave(): void; onDelete(): void
}) {
  const catalog = useMessages()
  const m = catalog.hierarchyShell.modelSettings
  const update = (key: keyof ProviderDraft, value: string) => onChange({ ...draft, [key]: value })
  return <div className="provider-dialog-layer" onPointerDown={(event) => {
    if (event.currentTarget === event.target) onCancel()
  }}>
    <section className="provider-dialog" role="dialog" aria-modal="true" aria-label={draft.id ? m.editProvider(draft.name) : m.newProvider}>
      <header><strong>{draft.id ? m.editProvider(draft.name) : m.newProvider}</strong><button type="button" aria-label={catalog.common.close} onClick={onCancel}>×</button></header>
      <div className="provider-dialog__body">
        <label>{m.providerName}<input aria-label={m.providerName} autoFocus value={draft.name} onChange={(event) => update('name', event.target.value)} /></label>
        <label>{m.defaultModel}<input aria-label={m.defaultModel} value={draft.model} placeholder={cli === 'claude-code' ? 'claude-opus-5' : 'gpt-5.6-sol'} onChange={(event) => update('model', event.target.value)} /></label>
        <label className="wide">{m.apiEndpoint}<input aria-label={m.apiEndpoint} value={draft.endpoint} onChange={(event) => update('endpoint', event.target.value)} /></label>
        <label className="wide">API Key<input aria-label="API Key" type="password" value={draft.apiKey} placeholder={draft.id ? m.apiKeyKeepHint : m.apiKeyPlaceholder} onChange={(event) => update('apiKey', event.target.value)} /></label>
        <div className="provider-dialog__advanced wide">{m.advanced}</div>
        {failure && <div className="provider-dialog__error wide" role="alert">{failure}</div>}
      </div>
      <footer>
        <span>{draft.id && !draft.builtIn && <button className="delete" type="button" onClick={onDelete}>{m.deleteProvider}</button>}</span>
        <button type="button" onClick={onCancel}>{catalog.common.cancel}</button>
        <button className="primary" type="button" disabled={saving} onClick={onSave}>{saving ? m.saving : m.saveConfiguration}</button>
      </footer>
    </section>
  </div>
}

function ProviderLogo({ provider }: { provider: ProviderConfigView }) {
  const initial = useMemo(() => provider.name.trim().slice(0, 1).toUpperCase() || 'P', [provider.name])
  return <span className={`model-provider__logo tone-${tone(provider.name)}`}>{initial}</span>
}
function SlidersIcon() { return <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8"><path d="M4 7h10M18 7h2M4 17h2M10 17h10"/><circle cx="16" cy="7" r="2"/><circle cx="8" cy="17" r="2"/></svg> }
function shortEndpoint(value: string) { return value.replace(/^https?:\/\//, '').replace(/\/$/, '') }
/** The runtime stores a sentinel when a provider keeps the model the CLI picks. */
function modelLabel(model: string, cliDefault: string) { return model === CLI_DEFAULT_MODEL ? cliDefault : model }
function tone(name: string) { return name.toLowerCase().includes('openai') ? 'openai' : name.toLowerCase().includes('anthropic') ? 'anthropic' : 'custom' }
function validateDraft(draft: ProviderDraft): string {
  const m = messages().hierarchyShell.modelSettings
  if (!draft.name.trim()) return m.nameRequired
  if (!draft.model.trim() && !draft.builtIn) return m.modelRequired
  try {
    const url = new URL(draft.endpoint.trim())
    if (!['http:', 'https:'].includes(url.protocol)) return m.endpointScheme
  } catch { return m.endpointInvalid }
  return ''
}
function message(error: unknown, fallback: string) {
  return error instanceof Error
    ? messages().hierarchyShell.modelSettings.errorDetail(fallback, error.message)
    : fallback
}
