import { useEffect, useRef, useState } from 'react'
import { CLI_DEFAULT_MODEL, PROVIDER_PRESETS, type ProviderCli, type ProviderConfigInput, type ProviderConfigView, type ProviderModelsResult, type ProviderPreset } from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import type { ProviderConfigClient } from './ModelSwitchSettings'
import './provider-setup.css'

type Draft = { name: string; endpoint: string; model: string; apiKey: string; modelMapping: NonNullable<ProviderConfigInput['modelMapping']> }
export function ProviderSetupDialog({ cli, existing, official, activeId, client, onClose, onSave, onUseOfficial, onDelete }: {
  cli: ProviderCli; existing?: ProviderConfigView | undefined; official?: ProviderConfigView | undefined; activeId?: string | undefined
  client: ProviderConfigClient | null; onClose(): void
  onSave(input: ProviderConfigInput, use: boolean): Promise<void>
  onUseOfficial(provider: ProviderConfigView): Promise<void>
  onDelete(provider: ProviderConfigView): Promise<void>
}) {
  const zh = useLocale() === 'zh-CN', t = (cn: string, en: string) => zh ? cn : en
  const app = cli === 'claude-code' ? 'Claude Code' : 'Codex'
  const presets = PROVIDER_PRESETS[cli]
  const initialPreset = presets.find(p => p.endpoint === existing?.endpoint)
  const [step, setStep] = useState<'pick' | 'form'>(existing ? 'form' : 'pick')
  const [selected, setSelected] = useState(initialPreset?.id ?? '')
  const [preset, setPreset] = useState<ProviderPreset | undefined>(initialPreset)
  const [query, setQuery] = useState(''), [filter, setFilter] = useState('all')
  const initial: Draft = { name: existing?.name ?? '', endpoint: existing?.endpoint ?? '', model: existing?.model === CLI_DEFAULT_MODEL ? '' : existing?.model ?? '', apiKey: '', modelMapping: existing?.modelMapping ?? {} }
  const [draft, setDraft] = useState<Draft>(initial), baseline = useRef(JSON.stringify(initial))
  const [busy, setBusy] = useState(false), working = useRef(false)
  const [failure, setFailure] = useState(''), [reveal, setReveal] = useState(false)
  const [advanced, setAdvanced] = useState(!initialPreset && Boolean(existing))
  const [confirm, setConfirm] = useState<'close' | 'back' | 'delete'>()
  const [fetching, setFetching] = useState(false), [modelMessage, setModelMessage] = useState('')
  const [models, setModels] = useState<string[]>([]), [modelQuery, setModelQuery] = useState('')
  const [chooser, setChooser] = useState(false)
  const generation = useRef(0), alive = useRef(true), root = useRef<HTMLDivElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const dirty = JSON.stringify(draft) !== baseline.current
  const errorNode = useRef<HTMLDivElement>(null), confirmNode = useRef<HTMLDivElement>(null)
  useEffect(() => { if (confirm) { confirmNode.current?.querySelector('button')?.focus(); confirmNode.current?.scrollIntoView?.({ block: 'nearest' }) } }, [confirm])
  useEffect(() => { if (failure) { errorNode.current?.focus(); errorNode.current?.scrollIntoView?.({ block: 'nearest' }) } }, [failure])
  const backToPick = () => { const blank: Draft = { name: '', endpoint: '', model: '', apiKey: '', modelMapping: {} }; setDraft(blank); baseline.current = JSON.stringify(blank); setStep('pick'); setConfirm(undefined); generation.current++; setFetching(false); setModels([]); setChooser(false); setModelMessage('') }
  const requestClose = () => { if (working.current) return; dirty ? setConfirm('close') : onClose() }
  const goBack = () => { if (working.current) return; dirty ? setConfirm('back') : backToPick() }
  useEffect(() => {
    alive.current = true; opener.current = document.activeElement as HTMLElement
    return () => { alive.current = false; generation.current++; opener.current?.focus() }
  }, [])
  useEffect(() => { root.current?.querySelector<HTMLElement>('[data-autofocus]')?.focus() }, [step])
  const update = (key: keyof Draft, value: Draft[keyof Draft]) => {
    setDraft(v => ({ ...v, [key]: value })); setFailure('')
    if (key === 'endpoint' || key === 'apiKey') { generation.current++; setFetching(false); setModels([]); setChooser(false); setModelMessage('') }
  }
  const choose = () => {
    setFailure(''); setConfirm(undefined)
    if (selected === 'official') { setStep('form'); return }
    const next = presets.find(p => p.id === selected)
    const value: Draft = { name: next?.name ?? '', endpoint: next?.endpoint ?? '', model: next?.model ?? '', apiKey: '', modelMapping: next?.modelMapping ?? {} }
    setPreset(next); setDraft(value); baseline.current = JSON.stringify(value); setAdvanced(!next)
    generation.current++; setModels([]); setModelMessage(''); setStep('form')
  }
  const run = async (operation: () => Promise<void>) => {
    if (working.current) return
    working.current = true; setBusy(true); setFailure('')
    try { await operation() } catch (e) { if (alive.current) setFailure(e instanceof Error ? e.message : t('操作未完成，请重试。', 'Operation did not finish. Try again.')) }
    finally { working.current = false; if (alive.current) setBusy(false) }
  }
  const save = (use: boolean) => {
    let error = ''
    if (!draft.name.trim()) error = t('请填写显示名称。', 'Enter a display name.')
    try { const u = new URL(draft.endpoint); if (!['https:', 'http:'].includes(u.protocol) || u.username || u.password || u.search || u.hash) throw Error() }
    catch { error = t('请填写有效的 API 基础地址，例如 https://api.example.com/v1。', 'Enter a valid API base URL, for example https://api.example.com/v1.') }
    if (!draft.apiKey.trim() && !existing?.hasApiKey && !existing?.builtIn) error = t('请先填写 API Key。', 'Enter an API key first.')
    if (draft.apiKey.trim() && (!/^[\x21-\x7e]+$/.test(draft.apiKey.trim()) || /^(export\s|[A-Z_]+=)/.test(draft.apiKey.trim()))) error = t('请仅粘贴密钥本身，不包含空格、换行或变量名。', 'Paste only the key, without whitespace or a variable name.')
    if (!draft.model.trim() && !existing?.builtIn) error = t('请选择或填写默认模型。', 'Choose or enter a default model.')
    if (existing?.hasApiKey && draft.endpoint.replace(/\/$/, '') !== existing.endpoint.replace(/\/$/, '') && !draft.apiKey.trim()) error = t('地址已更改，请重新填写此服务的密钥，避免把原密钥发送到其他服务。', 'The URL changed. Re-enter the key for this service instead of reusing the previous secret.')
    if (error) { setFailure(error); return }
    void run(() => onSave({ cli, ...(existing ? { id: existing.id } : {}), name: draft.name.trim(), endpoint: draft.endpoint.trim(), model: draft.model.trim() || '__cli_default__', ...(draft.apiKey.trim() ? { apiKey: draft.apiKey.trim() } : {}), modelMapping: draft.modelMapping }, use))
  }
  const discover = async () => {
    if (!client || fetching) return
    const token = ++generation.current; setFetching(true); setModelMessage(''); setChooser(false)
    const errors: Record<string, string> = {
      auth: t('密钥未通过验证，请检查密钥和对应的服务地址。', 'The key was rejected. Check the key and service URL.'),
      unsupported: t('该地址暂不支持模型列表，请手动填写模型名称。', 'This endpoint does not offer a compatible model list. Enter the model manually.'),
      timeout: t('获取超时，已保留填写内容。可以重试或手动填写。', 'Request timed out. Your draft is kept; retry or enter a model manually.'),
      network: t('连接未完成，请检查网络或服务地址后重试。', 'Connection failed. Check the network and service URL, then retry.'),
      invalid: t('请先填写有效的 API 基础地址。', 'Enter a valid API base URL first.'),
      'key-required': t('请填写此地址对应的 API Key；修改地址后需重新填写密钥。', 'Enter the key for this endpoint. A changed endpoint requires re-entering the key.'),
      empty: t('服务没有返回模型，可以手动填写模型名称。', 'No models returned. Enter a model manually.'),
      'too-large': t('模型列表过大，请手动填写模型名称。', 'Model list is too large. Enter a model manually.')
    }
    try {
      const result = await client.request('provider-config.models', { cli, endpoint: draft.endpoint, ...(existing ? { providerId: existing.id } : {}), ...(draft.apiKey.trim() ? { apiKey: draft.apiKey.trim() } : {}) }) as ProviderModelsResult
      if (!alive.current || generation.current !== token) return
      if ('error' in result) setModelMessage(errors[result.error] ?? errors.network!)
      else { setModels(result.models); setModelQuery(''); setChooser(true); setModelMessage(t('已获取模型列表；尚未验证实际对话能力。', 'Models fetched; actual inference has not been tested.')) }
    } catch { if (alive.current && generation.current === token) setModelMessage(errors.network!) }
    finally { if (alive.current && generation.current === token) setFetching(false) }
  }
  const officialMode = selected === 'official' && !existing
  const title = step === 'pick' ? t('添加供应商', 'Add provider') : officialMode ? t('使用官方账户', 'Use official account') : existing ? t('编辑供应商', 'Edit provider') : `${t('配置', 'Configure')} ${preset?.name ?? t('自定义服务', 'custom service')}`
  const matches = (value: string) => value.toLowerCase().includes(query.trim().toLowerCase())
  return <div className="provider-setup-layer" onPointerDown={e => { if (e.target === e.currentTarget) requestClose() }}>
    <div ref={root} className="provider-setup" role="dialog" aria-modal="true" aria-label={title} aria-busy={busy} onKeyDown={e => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); if (confirm) setConfirm(undefined); else if (chooser) setChooser(false); else requestClose() }
      if (e.key === 'Tab') {
        const controls = [...root.current!.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href], summary')].filter(el => el.getClientRects().length > 0)
        const first = controls[0], last = controls.at(-1)
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus() }
        else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus() }
      }
    }}>
      <header className="provider-setup__header">
        {step === 'form' && !existing && <button type="button" className="provider-icon-button" aria-label={t('返回选择服务', 'Back to services')} onClick={goBack} disabled={busy}>←</button>}
        <div><h2>{title}</h2><p>{t('用于', 'For')} {app}</p></div>
        <button className="provider-icon-button" type="button" aria-label={t('关闭', 'Close')} onClick={requestClose} disabled={busy}>×</button>
      </header>
      <div className="provider-setup__body">
      {step === 'pick' ? <>
        <input data-autofocus type="search" className="provider-search" aria-label={t('搜索供应商', 'Search providers')} placeholder={t('搜索供应商', 'Search providers')} value={query} onChange={e => setQuery(e.target.value)} />
        <div className="provider-filters" aria-label={t('供应商分类', 'Provider categories')}>{[['all',t('全部','All')],['official',t('官方登录','Official sign-in')],['api',t('API 服务','API services')]].map(([id, label]) => <button key={id} type="button" aria-pressed={filter === id} onClick={() => setFilter(id!)}>{label}</button>)}</div>
        {filter !== 'api' && matches(`${app} 官方 official`) && <button className={`provider-preset provider-preset--wide ${selected === 'official' ? 'is-selected' : ''}`} type="button" aria-pressed={selected === 'official'} onClick={() => setSelected('official')}><span className="provider-preset__icon">◎</span><span><strong>{cli === 'claude-code' ? 'Claude' : 'ChatGPT'} {t('官方账户', 'account')}</strong><small>{t('使用已有登录，无需 API Key', 'Use existing sign-in, no API key')}</small></span><b>›</b></button>}
        {filter !== 'official' && <>
          <p className="provider-field-label">API {t('服务', 'services')}</p>
          <div className="provider-preset-grid">{presets.filter(p => matches(`${p.name} ${p.id} ${p.endpoint}`)).map(p => <button key={p.id} type="button" className={`provider-preset ${selected === p.id ? 'is-selected' : ''}`} aria-pressed={selected === p.id} onClick={() => setSelected(p.id)}><span className={`provider-preset__icon tone-${p.id}`}>{p.name.slice(0,1)}</span><span><strong>{p.name}</strong><small>{t('预设接入配置', 'Preset configuration')}</small></span><b>{selected === p.id ? '✓' : '○'}</b></button>)}</div>
          {query && !presets.some(p => matches(`${p.name} ${p.id} ${p.endpoint}`)) && <p className="provider-help">{t('没有匹配的 API 服务，试试自定义配置。', 'No matching service. Try a custom configuration.')}</p>}
          <button type="button" className={`provider-preset provider-preset--wide ${selected === 'custom' ? 'is-selected' : ''}`} aria-pressed={selected === 'custom'} onClick={() => setSelected('custom')}><span className="provider-preset__icon">＋</span><span><strong>{t('自定义服务', 'Custom service')}</strong><small>{t('使用自己的 API 地址和密钥', 'Use your own API URL and key')}</small></span><b>›</b></button>
        </>}
        <p className="provider-help">ⓘ {t('选择服务后，自动填写推荐配置。预设不代表连接已验证。', 'Presets fill recommended settings, not a verified connection.')}</p>
      </> : officialMode ? <div className="provider-official">
        <span className="provider-preset__icon">◎</span><h3>{t('沿用本机官方登录', 'Use the existing official sign-in')}</h3>
        <p>{t(`这里切换 ${app} 的服务来源，不会新增或切换登录账户。`, `This changes the service used by ${app}, not the signed-in account.`)}</p>
        <p>{cli === 'claude-code' ? t('添加或切换 Claude 账户，请返回终端，在右下角打开「Claude 账户」。', 'To add or switch Claude accounts, return to a terminal and open Claude accounts at the bottom right.') : t('尚未登录时，请在 Codex 中完成官方登录。', 'If not signed in, complete official sign-in in Codex.')}</p>
        {official?.hasApiKey && <p className="provider-inline-error">{t('现有官方配置保存了 API Key，将沿用该配置。若要使用订阅登录，请先检查官方配置。', 'The official configuration contains an API key and will retain it. Review it before using subscription sign-in.')}</p>}
      </div> : <>
        <div className="provider-selected"><span className={`provider-preset__icon tone-${preset?.id ?? 'custom'}`}>{(preset?.name ?? draft.name ?? 'P').slice(0,1) || '＋'}</span><div><strong>{preset?.name ?? t('自定义服务', 'Custom service')}</strong><small>{cli === 'claude-code' ? 'Anthropic Messages' : 'OpenAI Responses'}</small></div>{!existing && <button className="provider-link" type="button" onClick={goBack} disabled={busy}>{t('更换', 'Change')}</button>}</div>
        <div className="provider-field"><div className="provider-field-heading"><label htmlFor="provider-api-key">API Key</label>{preset && <button className="provider-link" type="button" onClick={() => {
            const open = window.matouDesktop?.openProviderKeyPage
            if (!open) { setFailure(t('请在浏览器打开：', 'Open in your browser: ') + preset.keyUrl); return }
            void open(cli, preset.id).catch(() => setFailure(t('浏览器未打开，请访问：', 'Browser did not open. Visit: ') + preset.keyUrl))
          }}>{t('获取密钥', 'Get API key')} ↗</button>}</div>
          <div className="provider-key-input"><input data-autofocus id="provider-api-key" autoComplete="off" type={reveal ? 'text' : 'password'} value={draft.apiKey} disabled={busy} placeholder={existing?.hasApiKey ? t('已配置 · 留空保持原密钥', 'Configured · leave blank to keep') : t('粘贴 API Key', 'Paste API key')} onChange={e => update('apiKey', e.target.value)} /><button type="button" aria-label={reveal ? t('隐藏密钥','Hide key') : t('显示密钥','Show key')} onClick={() => setReveal(!reveal)}>{reveal ? '◉' : '◎'}</button></div>
          <small className="provider-help">{t('密钥保存在本机，仅向你配置的服务发送。', 'Stored locally and sent only to your configured service.')}</small>
        </div>
        <label className="provider-field">{t('API 地址', 'API URL')}<input aria-label={t('API 地址', 'API URL')} value={draft.endpoint} disabled={busy} onChange={e => update('endpoint',e.target.value)} placeholder="https://api.example.com/v1" /><small className="provider-help">{t('填写基础地址，不含 /messages 或 /responses；需支持上方协议。', 'Use a base URL without /messages or /responses, supporting the protocol above.')}</small></label>
        {draft.endpoint.startsWith('http:') && <p className="provider-inline-error">{t('当前为 HTTP，密钥将以明文传输。远程服务建议使用 HTTPS。', 'HTTP sends the key unencrypted. Use HTTPS for remote services.')}</p>}
        <div className="provider-field"><div className="provider-field-heading"><label htmlFor="provider-model">{t('默认模型', 'Default model')}</label><button className="provider-link" type="button" disabled={busy || fetching || !client} onClick={() => void discover()}>{fetching ? t('正在获取…', 'Fetching…') : t('获取模型', 'Get models')}</button></div>
          <input id="provider-model" value={draft.model} disabled={busy} placeholder={existing?.builtIn ? t('使用 CLI 默认模型', 'Use CLI default') : t('选择或填写模型名称', 'Choose or enter model ID')} onChange={e => update('model',e.target.value)} />
          <small className="provider-help">{t('已带出预设模型，也可以手动填写。', 'Preset models can also be edited manually.')}</small>
          {modelMessage && <p className="provider-model-message" role="status">{modelMessage}</p>}
          {models.length > 0 && !chooser && <button type="button" className="provider-link" onClick={() => setChooser(true)}>{t('从已获取列表选择', 'Choose from fetched models')}</button>}
          {chooser && <div className="provider-model-picker"><input type="search" aria-label={t('搜索模型', 'Search models')} placeholder={t('搜索模型', 'Search models')} value={modelQuery} onChange={e => setModelQuery(e.target.value)} /><div role="listbox" aria-label={t('可用模型','Available models')}>{models.filter(id => id.toLowerCase().includes(modelQuery.toLowerCase())).map(id => <button type="button" role="option" aria-selected={draft.model === id} key={id} onClick={() => { update('model',id); setChooser(false); document.getElementById('provider-model')?.focus() }}>{id}{draft.model === id ? ' ✓' : ''}</button>)}{!models.some(id => id.toLowerCase().includes(modelQuery.toLowerCase())) && <p className="provider-help">{t('没有匹配模型', 'No matching model')}</p>}</div></div>}
        </div>
        <details className="provider-more" open={advanced} onToggle={e => setAdvanced(e.currentTarget.open)}><summary>{t('更多设置', 'More settings')}<small>{cli === 'claude-code' ? t('显示名称、模型映射', 'Display name and model mapping') : t('显示名称', 'Display name')}</small></summary><div>
          <label className="provider-field">{t('供应商名称', 'Provider name')}<input value={draft.name} disabled={busy} onChange={e => update('name',e.target.value)} /></label>
          {cli === 'claude-code' && <><p className="provider-help">{t('指定各模型档位实际使用的模型；留空沿用 CLI 行为。', 'Map each role to a model. Leave blank for CLI defaults.')}</p>{(['opus','sonnet','haiku'] as const).map(role => <label className="provider-mapping" key={role}>{role}<input aria-label={`${role} ${t('模型映射','model mapping')}`} value={draft.modelMapping[role] ?? ''} disabled={busy} onChange={e => update('modelMapping',{...draft.modelMapping,[role]: e.target.value})} /></label>)}</>}
        </div></details>
        <p className="provider-help">ⓘ {existing ? t('保存修改不会重新连接运行中的会话。', 'Saving does not reconnect running sessions.') : t('仅保存不会切换；添加并使用会更新默认供应商。', 'Save only keeps the configuration; Add and use changes the default.')}</p>
      </>}
      {failure && <div ref={errorNode} tabIndex={-1} className="provider-inline-error" role="alert">{failure}</div>}
      {confirm && <div ref={confirmNode} className="provider-confirm" role="alert"><strong>{confirm === 'delete' ? t(`删除「${existing?.name}」？`, `Delete “${existing?.name}”?`) : t('有尚未保存的修改', 'You have unsaved changes')}</strong><p>{confirm === 'delete' ? t('此操作移除保存的配置，不会注销账户。', 'This removes the saved configuration, not your account.') : t('离开后，这次填写的内容将被丢弃。', 'Leaving discards this draft.')}</p><button type="button" onClick={() => setConfirm(undefined)}>{t('继续编辑','Keep editing')}</button><button type="button" onClick={() => { if (confirm === 'delete' && existing) void run(() => onDelete(existing)); else if (confirm === 'back') { backToPick() } else onClose() }} disabled={busy}>{confirm === 'delete' ? t('确认删除','Delete') : t('放弃修改','Discard')}</button></div>}
      </div>
      <footer className="provider-setup__footer">
        {existing && !existing.builtIn && existing.id !== activeId && <button type="button" className="provider-delete" disabled={busy} onClick={() => setConfirm('delete')}>{t('删除','Delete')}</button>}
        <span />
        <button type="button" className={existing ? 'primary' : undefined} disabled={busy} onClick={step === 'form' && !officialMode ? () => save(false) : requestClose}>{step === 'form' && !officialMode ? busy ? t('保存中…','Saving…') : existing ? t('保存修改','Save changes') : t('仅保存','Save only') : t('取消','Cancel')}</button>
        {!existing && <button type="button" className="primary" disabled={busy || (step === 'pick' && !selected) || (officialMode && !official)} onClick={() => { if (step === 'pick') choose(); else if (officialMode && official) void run(() => onUseOfficial(official)); else save(true) }}>{busy ? t('处理中…','Working…') : step === 'pick' ? t('下一步','Next') : officialMode ? t('使用官方服务','Use official service') : t('添加并使用','Add and use')}</button>}
      </footer>
    </div>
  </div>
}
