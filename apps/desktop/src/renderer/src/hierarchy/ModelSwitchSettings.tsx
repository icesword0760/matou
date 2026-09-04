import { useCallback, useEffect, useMemo, useState } from 'react'

import type {
  ProviderCli, ProviderConfigActivationResult, ProviderConfigInput,
  ProviderConfigSnapshot, ProviderConfigView, RpcMethod
} from '@matou/contracts'

import { APP_DISPLAY_NAME } from '../../../shared/brand'

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
      setFailure('供应商配置服务正在连接，请稍后重试')
      return
    }
    try {
      const next = await client.request('provider-config.snapshot', {}) as ProviderConfigSnapshot
      setSnapshot(next); setFailure('')
    } catch (error) {
      setFailure(message(error, '供应商配置载入失败'))
    } finally {
      setLoading(false)
    }
  }, [client])

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
          ? `已切换为 ${provider.name}；${updated} 个 Claude Code 会话已更新，${deferred} 个会话暂缓并保持原配置`
          : updated > 0
            ? `已切换为 ${provider.name}；${updated} 个 Claude Code 会话已更新`
            : `已切换为 ${provider.name}；Claude Code 新会话直接生效`)
      } else {
        setToast(`已切换为 ${provider.name}；Codex 新会话直接生效`)
      }
    } catch (error) {
      setToast(message(error, '切换失败'))
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
      setToast(draft.id ? '供应商配置已保存' : '供应商已添加')
    } catch (error) {
      setFailure(message(error, '供应商配置保存失败'))
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
      setSnapshot(next); setDraft(undefined); announceChange(); setToast('供应商已删除')
    } catch (error) {
      setFailure(message(error, '删除失败'))
    } finally { setSaving(false) }
  }

  return <section className="model-settings" aria-label="模型切换设置">
    <div className="model-settings__frame">
      <nav className="model-settings__nav" aria-label="设置分类">
        <span className="model-settings__section-label">AI 服务</span>
        <button className="model-settings__nav-item is-active" type="button">
          <SlidersIcon /><span>模型切换</span>
        </button>
        <p>供应商切换是全局设置，将同步影响所有 {APP_DISPLAY_NAME} 窗口。</p>
      </nav>
      <main className="model-settings__main">
        <header className="model-settings__heading">
          <div><h1>模型切换</h1><p>配置 Claude Code 与 Codex 使用的供应商，并设置全局默认。</p></div>
          <button type="button" aria-label="关闭设置" onClick={onClose}>×</button>
        </header>
        <div className="model-settings__tabs" role="tablist" aria-label="CLI 类型">
          {([['claude-code', 'Claude Code'], ['codex', 'Codex']] as const).map(([id, label]) =>
            <button key={id} role="tab" aria-selected={cli === id} className={cli === id ? 'is-active' : ''}
              type="button" onClick={() => { setCli(id); setFailure('') }}>{label}</button>)}
        </div>
        {loading ? <div className="model-settings__state" aria-busy="true">正在载入供应商配置…</div> : <>
          {active && <section className="model-settings__current" aria-label="全局使用中的供应商">
            <ProviderLogo provider={active} />
            <div><strong>{active.name}</strong><span>{active.model} · {shortEndpoint(active.endpoint)}</span></div>
            <b>全局使用中</b>
          </section>}
          <div className="model-settings__toolbar">
            <h2>供应商 <span>{providers.length} 个配置</span></h2>
            <button type="button" aria-label="新增供应商" onClick={() => setDraft({ ...EMPTY_DRAFT })}>＋ 新增供应商</button>
          </div>
          {failure && !draft && <div className="model-settings__error" role="alert">{failure}</div>}
          <div className="model-settings__providers">
            {providers.map((provider) => {
              const current = provider.id === activeId
              return <article key={provider.id} className={`model-provider${current ? ' is-current' : ''}`}>
                <ProviderLogo provider={provider} />
                <div className="model-provider__name"><strong>{provider.name}{current && <i>使用中</i>}</strong><span>{shortEndpoint(provider.endpoint)}</span></div>
                <div className="model-provider__model"><strong>{provider.model}</strong><span>默认模型{provider.hasApiKey ? ' · Key 已配置' : ''}</span></div>
                <div className="model-provider__actions">
                  <button type="button" onClick={() => setDraft({
                    id: provider.id, name: provider.name, endpoint: provider.endpoint,
                    model: provider.builtIn && provider.model === 'CLI 默认' ? '' : provider.model,
                    apiKey: '', ...(provider.builtIn ? { builtIn: true } : {})
                  })}>编辑</button>
                  <button className="primary" type="button" disabled={current}
                    aria-label={current ? `${provider.name} 当前配置` : `切换到 ${provider.name}`}
                    onClick={() => void activate(provider)}>{current ? '当前配置' : '切换'}</button>
                </div>
              </article>
            })}
          </div>
          <p className="model-settings__impact"><span>ⓘ</span><b>切换会影响所有 {APP_DISPLAY_NAME} 窗口。</b> 符合条件的 Claude Code 会话自动更新，暂缓会话保持原配置；Codex 新会话直接生效，运行中的会话需要重启。</p>
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
  const update = (key: keyof ProviderDraft, value: string) => onChange({ ...draft, [key]: value })
  return <div className="provider-dialog-layer" onPointerDown={(event) => {
    if (event.currentTarget === event.target) onCancel()
  }}>
    <section className="provider-dialog" role="dialog" aria-modal="true" aria-label={draft.id ? `编辑 ${draft.name}` : '新增供应商'}>
      <header><strong>{draft.id ? `编辑 ${draft.name}` : '新增供应商'}</strong><button type="button" aria-label="关闭" onClick={onCancel}>×</button></header>
      <div className="provider-dialog__body">
        <label>供应商名称<input aria-label="供应商名称" autoFocus value={draft.name} onChange={(event) => update('name', event.target.value)} /></label>
        <label>默认模型<input aria-label="默认模型" value={draft.model} placeholder={cli === 'claude-code' ? 'claude-opus-5' : 'gpt-5.6-sol'} onChange={(event) => update('model', event.target.value)} /></label>
        <label className="wide">API 地址<input aria-label="API 地址" value={draft.endpoint} onChange={(event) => update('endpoint', event.target.value)} /></label>
        <label className="wide">API Key<input aria-label="API Key" type="password" value={draft.apiKey} placeholder={draft.id ? '留空表示保持原 Key' : '输入 API Key'} onChange={(event) => update('apiKey', event.target.value)} /></label>
        <div className="provider-dialog__advanced wide">▸ 高级配置　模型映射与未知配置字段将在这里保留</div>
        {failure && <div className="provider-dialog__error wide" role="alert">{failure}</div>}
      </div>
      <footer>
        <span>{draft.id && !draft.builtIn && <button className="delete" type="button" onClick={onDelete}>删除供应商</button>}</span>
        <button type="button" onClick={onCancel}>取消</button>
        <button className="primary" type="button" disabled={saving} onClick={onSave}>{saving ? '保存中…' : '保存配置'}</button>
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
function tone(name: string) { return name.toLowerCase().includes('openai') ? 'openai' : name.toLowerCase().includes('anthropic') ? 'anthropic' : 'custom' }
function validateDraft(draft: ProviderDraft): string {
  if (!draft.name.trim()) return '请输入供应商名称'
  if (!draft.model.trim() && !draft.builtIn) return '请输入默认模型'
  try {
    const url = new URL(draft.endpoint.trim())
    if (!['http:', 'https:'].includes(url.protocol)) return 'API 地址需要使用 HTTP 或 HTTPS'
  } catch { return '请输入有效的 API 地址' }
  return ''
}
function message(error: unknown, fallback: string) { return error instanceof Error ? `${fallback}：${error.message}` : fallback }
