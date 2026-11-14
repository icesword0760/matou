import { useEffect, useRef, useState } from 'react'
import { CLI_DEFAULT_MODEL, PROVIDER_PRESETS, type ContextWindowMode, type ModelContextChoice, type ProviderCli, type ProviderConfigInput, type ProviderConfigView, type ProviderModelMetadata, type ProviderModelsResult, type ProviderPreset, type ReasoningEffort, type SubscriptionProvider } from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import type { ProviderConfigClient } from './ModelSwitchSettings'
import { ContextWindowPicker, resolveProviderWindow } from './context-window-ui'
import './provider-setup.css'

type Role = 'opus' | 'sonnet' | 'haiku'
type EffortMapping = Record<Role, ReasoningEffort>
type Draft = { name: string; endpoint: string; model: string; apiKey: string; reasoningEffort: ReasoningEffort; effortMapping: EffortMapping; modelMapping: NonNullable<ProviderConfigInput['modelMapping']> }
const ROLES: readonly Role[] = ['opus', 'sonnet', 'haiku']
const MAPPING_CLI_DEFAULT = '__provider_mapping_cli_default__'
const MAPPING_MANUAL = '__provider_mapping_manual__'
const MAPPING_MODEL_PREFIX = 'provider-model:'
const EMPTY_MANUAL_ROLES: Record<Role, boolean> = { opus: false, sonnet: false, haiku: false }
export function ProviderSetupDialog({ cli, existing, official, activeId, client, subscriptionSupported = true, contextWindowMode, contextModels, onContextWindowChange, onClose, onSave, onUseOfficial, onDelete, onSelectSubscription }: {
  cli: ProviderCli; existing?: ProviderConfigView | undefined; official?: ProviderConfigView | undefined; activeId?: string | undefined
  client: ProviderConfigClient | null; onClose(): void
  subscriptionSupported?: boolean
  contextWindowMode?: ContextWindowMode | undefined
  contextModels?: readonly ModelContextChoice[] | undefined
  onContextWindowChange?(mode: ContextWindowMode): void
  onSave(input: ProviderConfigInput, use: boolean): Promise<void>
  onUseOfficial(provider: ProviderConfigView): Promise<void>
  onDelete(provider: ProviderConfigView): Promise<void>
  onSelectSubscription?(provider: SubscriptionProvider): void
}) {
  const zh = useLocale() === 'zh-CN', t = (cn: string, en: string) => zh ? cn : en
  const app = cli === 'claude-code' ? 'Claude Code' : 'Codex'
  const presets = PROVIDER_PRESETS[cli]
  const initialPreset = presets.find(p => p.endpoint === existing?.endpoint)
  const [step, setStep] = useState<'pick' | 'form'>(existing ? 'form' : 'pick')
  const [selected, setSelected] = useState(initialPreset?.id ?? '')
  const [preset, setPreset] = useState<ProviderPreset | undefined>(initialPreset)
  const [query, setQuery] = useState(''), [filter, setFilter] = useState('all')
  const initial: Draft = {
    name: existing?.name ?? '', endpoint: existing?.endpoint ?? '', model: existing?.model === CLI_DEFAULT_MODEL ? '' : existing?.model ?? '', apiKey: '',
    reasoningEffort: existing?.reasoningEffort ?? 'follow',
    effortMapping: { opus: existing?.effortMapping?.opus ?? 'follow', sonnet: existing?.effortMapping?.sonnet ?? 'follow', haiku: existing?.effortMapping?.haiku ?? 'follow' },
    modelMapping: existing?.modelMapping ?? {}
  }
  const initialContextMode: ContextWindowMode = contextWindowMode ?? 'default'
  const [contextMode, setContextMode] = useState<ContextWindowMode>(initialContextMode)
  const contextTouched = useRef(false)
  const [draft, setDraft] = useState<Draft>(initial), baseline = useRef(JSON.stringify(initial))
  const [manualMappingRoles, setManualMappingRoles] = useState<Record<Role, boolean>>({ ...EMPTY_MANUAL_ROLES })
  const [metadata, setMetadata] = useState<ProviderModelMetadata[]>(existing?.modelMetadata ?? [])
  const [metadataExplicit, setMetadataExplicit] = useState(existing?.modelMetadata !== undefined)
  const metadataBaseline = useRef(metadataSnapshot(existing?.modelMetadata))
  const [busy, setBusy] = useState(false), working = useRef(false)
  const [failure, setFailure] = useState(''), [reveal, setReveal] = useState(false)
  const [confirm, setConfirm] = useState<'close' | 'back' | 'delete'>()
  const [fetching, setFetching] = useState(false), [modelMessage, setModelMessage] = useState('')
  const [fetchFailed, setFetchFailed] = useState(false)
  const [models, setModels] = useState<string[]>([]), [modelQuery, setModelQuery] = useState('')
  const [chooser, setChooser] = useState(false)
  const generation = useRef(0), alive = useRef(true), root = useRef<HTMLDivElement>(null)
  const modelPickerNode = useRef<HTMLDivElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const dirty = JSON.stringify(draft) !== baseline.current || contextMode !== initialContextMode || metadataSnapshot(metadata, metadataExplicit) !== metadataBaseline.current
  const errorNode = useRef<HTMLDivElement>(null), confirmNode = useRef<HTMLDivElement>(null)
  useEffect(() => { if (confirm) { confirmNode.current?.querySelector('button')?.focus(); confirmNode.current?.scrollIntoView?.({ block: 'nearest' }) } }, [confirm])
  useEffect(() => { if (failure) { errorNode.current?.focus(); errorNode.current?.scrollIntoView?.({ block: 'nearest' }) } }, [failure])
  useEffect(() => { if (!contextTouched.current) setContextMode(initialContextMode) }, [initialContextMode])
  useEffect(() => {
    if (!chooser || models.length === 0) return
    modelPickerNode.current?.querySelector<HTMLInputElement>('input[type="search"]')?.focus({ preventScroll: true })
  }, [chooser, models.length])
  const backToPick = () => { const blank: Draft = { name: '', endpoint: '', model: '', apiKey: '', reasoningEffort: 'follow', effortMapping: { opus: 'follow', sonnet: 'follow', haiku: 'follow' }, modelMapping: {} }; setDraft(blank); baseline.current = JSON.stringify(blank); setManualMappingRoles({ ...EMPTY_MANUAL_ROLES }); contextTouched.current = false; setContextMode(initialContextMode); onContextWindowChange?.(initialContextMode); setStep('pick'); setConfirm(undefined); generation.current++; setFetching(false); setModels([]); setChooser(false); setModelMessage(''); setMetadata([]); setMetadataExplicit(false); metadataBaseline.current = metadataSnapshot(undefined); setFetchFailed(false) }
  const requestClose = () => { if (working.current) return; dirty ? setConfirm('close') : onClose() }
  const goBack = () => { if (working.current) return; dirty ? setConfirm('back') : backToPick() }
  useEffect(() => {
    alive.current = true; opener.current = document.activeElement as HTMLElement
    return () => { alive.current = false; generation.current++; opener.current?.focus() }
  }, [])
  useEffect(() => { root.current?.querySelector<HTMLElement>('[data-autofocus]')?.focus() }, [step])
  const update = (key: keyof Draft, value: Draft[keyof Draft]) => {
    setDraft(v => ({ ...v, [key]: value })); setFailure('')
    if (key === 'endpoint' || key === 'apiKey') {
      generation.current++; setFetching(false); setModels([]); setChooser(false); setModelMessage(''); setFetchFailed(false)
      setMetadata([]); setMetadataExplicit(true)
    }
  }
  const choose = () => {
    setFailure(''); setConfirm(undefined)
    if (selected === 'subscription-codex' || selected === 'subscription-antigravity') {
      // Disabled options cannot be selected; keep this as the final guard for
      // a selection made before the runtime capability arrives.
      if (!subscriptionSupported) return
      onSelectSubscription?.(selected === 'subscription-codex' ? 'codex' : 'antigravity')
      return
    }
    if (selected === 'official') { setStep('form'); return }
    const next = presets.find(p => p.id === selected)
    const value: Draft = { name: next?.name ?? '', endpoint: next?.endpoint ?? '', model: next?.model ?? '', apiKey: '', reasoningEffort: 'follow', effortMapping: { opus: 'follow', sonnet: 'follow', haiku: 'follow' }, modelMapping: next?.modelMapping ?? {} }
    setPreset(next); setDraft(value); baseline.current = JSON.stringify(value); setManualMappingRoles({ ...EMPTY_MANUAL_ROLES })
    generation.current++; setModels([]); setModelMessage(''); setMetadata([]); setMetadataExplicit(false); metadataBaseline.current = metadataSnapshot(undefined); setFetchFailed(false); setStep('form')
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
    if (invalidEffortRole) {
      error = t(
        `${invalidEffortRole} 的推理强度不受当前模型支持，请重新选择。`,
        `The reasoning effort for ${invalidEffortRole} is not supported by its model. Choose another effort.`
      )
    } else if (codexEffortInvalid) {
      error = t('当前模型不支持所选推理强度，请重新选择。', 'This model does not support the selected reasoning effort. Choose another effort.')
    }
    if (error) { setFailure(error); return }
    void run(() => onSave({
      cli, ...(existing ? { id: existing.id } : {}), name: draft.name.trim(), endpoint: draft.endpoint.trim(), model: draft.model.trim() || '__cli_default__',
      ...(draft.apiKey.trim() ? { apiKey: draft.apiKey.trim() } : {}),
      ...(metadataExplicit || !existing?.builtIn ? { modelMetadata: metadata } : {}),
      ...(cli === 'claude-code' ? { effortMapping: draft.effortMapping } : { reasoningEffort: draft.reasoningEffort }),
      modelMapping: draft.modelMapping
    }, use))
  }
  const discover = async () => {
    if (!client || fetching) return
    const token = ++generation.current; setFetching(true); setModelMessage(''); setChooser(false); setFetchFailed(false)
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
      if ('error' in result) {
        setModels([]); setChooser(false); setFetchFailed(true); setModelMessage(errors[result.error] ?? errors.network!)
      } else {
        setModels(result.models); setMetadata(result.modelMetadata ?? []); setMetadataExplicit(true)
        setModelQuery(''); setChooser(true); setFetchFailed(false)
        setModelMessage(t(`已获取 ${result.models.length} 个模型；尚未验证实际对话能力。`, `${result.models.length} models fetched; actual inference has not been tested.`))
      }
    } catch { if (alive.current && generation.current === token) { setModels([]); setChooser(false); setFetchFailed(true); setModelMessage(errors.network!) } }
    finally { if (alive.current && generation.current === token) setFetching(false) }
  }
  const selectedModelId = draft.model.trim()
  const selectedMetadata = metadata.find(model => model.id === selectedModelId)
  const modelQueryText = modelQuery.trim().toLowerCase()
  const visibleModels = models.filter(id => {
    const displayName = metadata.find(model => model.id === id)?.displayName ?? ''
    return !modelQueryText || id.toLowerCase().includes(modelQueryText) || displayName.toLowerCase().includes(modelQueryText)
  })
  useEffect(() => {
    if (!chooser) return
    modelPickerNode.current?.scrollIntoView?.({ block: 'nearest' })
  }, [chooser, visibleModels.length])
  const mappingMetadata = fetchFailed ? [] : metadata
  const mappingOptionIds = [...new Set([...models, ...mappingMetadata.map(model => model.id)])]
  const mappedEffortKnown = ROLES.map(role => {
    const id = (draft.modelMapping[role] ?? '').trim()
    return Boolean(id && mappingMetadata.find(entry => entry.id === id)?.supportedEfforts !== undefined)
  })
  const allMappedEffortsKnown = mappedEffortKnown.every(Boolean)
  const anyMappedEffortsKnown = mappedEffortKnown.some(Boolean)
  const effortHint = allMappedEffortsKnown
    ? ''
    : anyMappedEffortsKnown
      ? t('已获取模型的推理强度来自接口返回范围；其余模型仍为候选，待服务确认。', 'Fetched models use their returned effort ranges; other models remain candidates pending service confirmation.')
      : t('推理强度选项为候选，实际可用性以供应商支持为准。', 'Reasoning effort options are candidates; actual availability depends on provider support.')
  const supportedEffortsForModel = (id: string) => !id || fetchFailed ? undefined : metadata.find(model => model.id === id)?.supportedEfforts
  const roleEffortStates = ROLES.map(role => {
    const model = (draft.modelMapping[role] ?? '').trim()
    const supportedEfforts = supportedEffortsForModel(model)
    const effort = draft.effortMapping[role]
    return { role, model, effort, supportedEfforts, valid: effortSupported(cli, model, effort, supportedEfforts) }
  })
  const invalidEffortRole = roleEffortStates.find(state => !state.valid)?.role
  const codexEffortInvalid = cli === 'codex' && !effortSupported(cli, draft.model, draft.reasoningEffort, supportedEffortsForModel(draft.model))
  const effortInvalid = cli === 'claude-code' ? Boolean(invalidEffortRole) : codexEffortInvalid
  const modelSource = fetching
    ? t('正在向当前地址获取模型信息…', 'Fetching model information from this endpoint…')
    : fetchFailed
      ? metadata.length > 0
        ? t('本次刷新失败；以下为上次记录的模型信息，名称与窗口可能已变化。', 'Refresh failed. The last recorded model information is shown; names and windows may have changed.')
        : t('本次刷新失败；当前没有可用的模型名称或窗口信息。', 'Refresh failed. No model names or windows are available.')
      : models.length > 0
        ? !selectedModelId
          ? t(`已获取 ${models.length} 个模型；请选择模型，或手动填写模型 ID。`, `${models.length} models fetched. Choose a model or enter an ID manually.`)
          : models.includes(selectedModelId)
            ? t('已在本次获取的模型列表中确认该 ID。', 'This ID was confirmed in the models fetched in this request.')
            : t('手动填写的模型 ID 不在本次获取列表中；名称与窗口未知。', 'The manually entered model ID is not in this response; its name and window are unknown.')
        : metadata.length > 0
          ? selectedMetadata
            ? t('显示已保存的模型信息，可获取模型刷新。', 'Showing saved model information. Fetch models to refresh it.')
            : selectedModelId
              ? t('手动填写的模型 ID 不在已保存的模型信息中；名称与窗口未知。', 'The manually entered ID is not in the saved model information; its name and window are unknown.')
              : t('显示已保存的模型信息，可获取模型刷新。', 'Showing saved model information. Fetch models to refresh it.')
          : existing
            ? t('当前仅有模型 ID；尚未获取名称或窗口信息。', 'Only a model ID is available; its name and window have not been fetched.')
            : preset && selectedModelId === preset.model.trim()
              ? t('预设模型尚未向服务验证名称、窗口或对话能力。', 'The preset model has not been verified against the service for its name, window, or inference.')
              : t('手动填写模型 ID；名称与窗口未知。', 'Entered manually; the name and window are unknown.')
  const fetchedSelectionMetadataMissing = models.includes(selectedModelId) && !selectedMetadata
  const fetchedSelectionNameMissing = models.includes(selectedModelId) && selectedMetadata && !selectedMetadata.displayName
  const fetchedSelectionWindowMissing = models.includes(selectedModelId) && selectedMetadata && !selectedMetadata.contextWindow && !selectedMetadata.maxContextWindow
  const fetchedMetadataGap = fetchedSelectionMetadataMissing
    ? t('本次响应只返回模型 ID，未提供显示名称与窗口。', 'This response returned only the model ID; no display name or window was provided.')
    : fetchedSelectionNameMissing && fetchedSelectionWindowMissing
      ? t('本次响应未提供显示名称与窗口。', 'This response did not provide a display name or window.')
      : fetchedSelectionNameMissing
        ? t('本次响应只返回模型 ID，未提供显示名称。', 'This response returned only the model ID; no display name was provided.')
        : fetchedSelectionWindowMissing
          ? t('本次响应未提供窗口。', 'This response did not provide a window.')
          : ''
  const metadataContextModels = metadata.map(model => ({
    id: model.id,
    name: model.displayName ?? model.id,
    ...(model.contextWindow ? { defaultTokens: model.contextWindow } : {}),
    ...(model.maxContextWindow ? { maximumTokens: model.maxContextWindow } : {}),
    fixedTokens: 512_000
  }))
  const nativeContextModels = existing?.builtIn && !metadataExplicit && isOfficialEndpoint(cli, existing.endpoint)
    ? contextModels ?? []
    : []
  const contextChoices = metadataExplicit ? metadataContextModels : nativeContextModels
  const selectedModelIds = [...new Set([draft.model, ...Object.values(draft.modelMapping)]
    .map(value => (value ?? '').trim())
    .filter(value => Boolean(value) && value !== CLI_DEFAULT_MODEL))]
  const windowResolution = resolveProviderWindow(contextMode, contextChoices, selectedModelIds)
  const contextWindowUnknown = contextMode !== '512k' && windowResolution.tokens === undefined
  const officialMode = selected === 'official' && !existing
  const subscriptionBlocked = !subscriptionSupported && (selected === 'subscription-codex' || selected === 'subscription-antigravity')
  const title = step === 'pick' ? t('添加供应商', 'Add provider') : officialMode ? t('使用官方账户', 'Use official account') : existing ? t('编辑供应商', 'Edit provider') : `${t('配置', 'Configure')} ${preset?.name ?? t('自定义服务', 'custom service')}`
  const matches = (value: string) => value.toLowerCase().includes(query.trim().toLowerCase())
  // The Codex tab keeps its native ChatGPT sign-in; only Claude Code offers these.
  const subscriptionChoices = cli === 'claude-code' && onSelectSubscription && filter !== 'api'
    ? [
        { id: 'subscription-codex' as const, provider: 'codex' as const, title: t('ChatGPT 账号', 'ChatGPT account'), description: t('使用你的 ChatGPT 订阅额度，不需要 API Key', 'Use your ChatGPT subscription. No API key needed.') },
        { id: 'subscription-antigravity' as const, provider: 'antigravity' as const, title: t('Antigravity 账号', 'Antigravity account'), description: t('使用 Google AI 订阅中的 Claude 与 Gemini 模型', 'Use Claude and Gemini models from your Google AI subscription.') }
      ].filter(option => matches(`${option.title} ${option.description} ${option.id}`))
    : []
  return <div className="provider-setup-layer" onPointerDown={e => { if (e.target === e.currentTarget) requestClose() }}>
    <div ref={root} className="provider-setup" role="dialog" aria-modal="true" aria-label={title} aria-busy={busy} onKeyDown={e => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); if (confirm) setConfirm(undefined); else if (chooser) setChooser(false); else requestClose() }
      if (e.key === 'Tab') {
        const controls = [...root.current!.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href]')].filter(el => el.getClientRects().length > 0)
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
        {subscriptionChoices.length > 0 && <>
          <p className="provider-field-label">{t('订阅账号', 'Subscription accounts')}</p>
          <div className="provider-subscription-options">{subscriptionChoices.map(option => <button key={option.id} type="button" className={`provider-preset ${selected === option.id ? 'is-selected' : ''}`} aria-pressed={selected === option.id} disabled={!subscriptionSupported} aria-describedby={subscriptionSupported ? undefined : 'provider-subscription-support-note'} onClick={() => setSelected(option.id)}><span className="provider-preset__icon">{option.provider === 'codex' ? 'C' : 'A'}</span><span><strong>{option.title}</strong><small>{option.description}</small></span><b>{selected === option.id ? '✓' : '○'}</b></button>)}</div>
          {!subscriptionSupported && <p id="provider-subscription-support-note" className="provider-subscription-support-note">{t('需要 macOS 15 或更高版本', 'Requires macOS 15 or later')}</p>}
        </>}
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
          {selectedMetadata && <div className="provider-model-detail" aria-live="polite">
            <strong>{selectedMetadata.displayName ?? selectedMetadata.id}</strong>
            <span>{selectedMetadata.displayName ? selectedMetadata.id : t('仅返回模型 ID', 'Model ID only')}</span>
            <em>{providerModelWindowText(selectedMetadata, zh)}</em>
          </div>}
          <small className="provider-help provider-model-source" aria-live="polite">{modelSource}</small>
          {fetchedMetadataGap && <small className="provider-help provider-model-gap">{fetchedMetadataGap}</small>}
          {modelMessage && <p className="provider-model-message" role="status">{modelMessage}</p>}
          {models.length > 0 && !chooser && <button type="button" className="provider-link" onClick={() => setChooser(true)}>{t('从已获取列表选择', 'Choose from fetched models')}</button>}
          {chooser && <div ref={modelPickerNode} className="provider-model-picker"><input type="search" aria-label={t('搜索模型', 'Search models')} placeholder={t('搜索模型名称或 ID', 'Search model name or ID')} value={modelQuery} onChange={e => setModelQuery(e.target.value)} /><div role="listbox" aria-label={t('可用模型','Available models')}>{visibleModels.map(id => {
            const modelMetadata = metadata.find(model => model.id === id)
            return <button type="button" role="option" aria-selected={draft.model === id} key={id} onClick={() => { update('model',id); setChooser(false); document.getElementById('provider-model')?.focus() }}>
              <span className="provider-model-option__name"><strong>{modelMetadata?.displayName ?? id}</strong><small>{modelMetadata?.displayName ? id : t('仅返回模型 ID', 'Model ID only')}</small></span>
              <em>{modelMetadata ? providerModelWindowText(modelMetadata, zh) : t('窗口未知', 'Window unknown')}</em>
              {draft.model === id && <b aria-hidden="true">✓</b>}
            </button>
          })}{visibleModels.length === 0 && <p className="provider-help">{t('没有匹配模型', 'No matching model')}</p>}</div></div>}
          {cli === 'codex' && <label className="provider-field provider-effort-field">{t('推理强度', 'Reasoning effort')}
            <select aria-label={t('推理强度', 'Reasoning effort')} value={draft.reasoningEffort} disabled={busy} onChange={e => update('reasoningEffort', e.target.value as ReasoningEffort)}>
              {effortChoices(cli, draft.model, draft.reasoningEffort, fetchFailed ? undefined : selectedMetadata?.supportedEfforts).map(effort => {
                const supported = effortSupported(cli, draft.model, effort, fetchFailed ? undefined : selectedMetadata?.supportedEfforts)
                return <option key={effort} value={effort}>{effortName(effort, zh)}{effort === draft.reasoningEffort && !supported ? ` ${t('（不支持）', '(unsupported)')}` : ''}</option>
              })}
            </select>
            <small className="provider-help">{fetchFailed ? undefined : selectedMetadata?.supportedEfforts !== undefined
              ? t('推理强度选项来自接口返回的模型信息。', 'Reasoning effort options come from the fetched model information.')
              : t('推理强度选项为候选，实际可用性以供应商支持为准。', 'Reasoning effort options are candidates; actual availability depends on provider support.')}</small>
            {!effortSupported(cli, draft.model, draft.reasoningEffort, fetchFailed ? undefined : selectedMetadata?.supportedEfforts) && <small className="provider-mapping__error" role="alert">{t('当前模型不支持所选推理强度。', 'This model does not support the selected reasoning effort.')}</small>}
          </label>}
        </div>
        <div className="provider-model-settings">
          <label className="provider-field">{t('供应商名称', 'Provider name')}<input value={draft.name} disabled={busy} onChange={e => update('name',e.target.value)} /></label>
          {cli === 'claude-code' && <>
            {ROLES.map(role => {
              const mapping = draft.modelMapping[role] ?? ''
              const roleModelId = mapping.trim()
              const effort = draft.effortMapping[role]
              const roleMetadata = roleModelId ? mappingMetadata.find(entry => entry.id === roleModelId) : undefined
              const supportedEfforts = roleMetadata?.supportedEfforts
              const manual = manualMappingRoles[role]
              const label = role[0]!.toUpperCase() + role.slice(1)
              const selectedValue = manual ? MAPPING_MANUAL : !roleModelId ? MAPPING_CLI_DEFAULT : `${MAPPING_MODEL_PREFIX}${roleModelId}`
              const currentNotListed = Boolean(roleModelId && !mappingOptionIds.includes(roleModelId))
              return <div className="provider-mapping" key={role} role="group" aria-label={label}>
                <span className="provider-mapping__role">{label}</span>
                {manual ? <div className="provider-mapping__manual">
                  <input aria-label={`${label} ${t('手动填写模型 ID','manual model ID')}`} value={mapping} disabled={busy} onChange={e => update('modelMapping',{...draft.modelMapping,[role]: e.target.value})} />
                  <button type="button" className="provider-link" aria-label={`${label} ${t('从列表选择','choose from list')}`} disabled={busy} onClick={() => setManualMappingRoles(current => ({ ...current, [role]: false }))}>{t('从列表选择', 'Choose from list')}</button>
                </div> : <select aria-label={`${label} ${t('模型','model')}`} value={selectedValue} title={roleModelId || undefined} disabled={busy} onChange={e => {
                  const next = e.target.value
                  if (next === MAPPING_MANUAL) { setManualMappingRoles(current => ({ ...current, [role]: true })); return }
                  setManualMappingRoles(current => ({ ...current, [role]: false }))
                  update('modelMapping', { ...draft.modelMapping, [role]: next === MAPPING_CLI_DEFAULT ? '' : next.slice(MAPPING_MODEL_PREFIX.length) })
                }}>
                  <option value={MAPPING_CLI_DEFAULT}>{t('沿用 CLI 默认', 'Use CLI default')}</option>
                  {mappingOptionIds.map(id => <option key={id} value={`${MAPPING_MODEL_PREFIX}${id}`} title={id}>{mappingMetadata.find(model => model.id === id)?.displayName ?? id}</option>)}
                  {currentNotListed && <option value={`${MAPPING_MODEL_PREFIX}${roleModelId}`} title={roleModelId}>{roleModelId}{models.length > 0 ? t('（未在列表中）', ' (not in fetched list)') : t('（尚未获取列表）', ' (models not fetched)')}</option>}
                  <option value={MAPPING_MANUAL}>{t('手动填写模型 ID…', 'Enter model ID manually…')}</option>
                </select>}
                <select aria-label={`${label} ${t('推理强度','reasoning effort')}`} value={effort} disabled={busy} onChange={e => update('effortMapping',{...draft.effortMapping,[role]: e.target.value as ReasoningEffort})}>
                  {effortChoices(cli, roleModelId, effort, supportedEfforts).map(option => {
                    const supported = effortSupported(cli, roleModelId, option, supportedEfforts)
                    return <option key={option} value={option}>{effortName(option, zh)}{option === effort && !supported ? ` ${t('（不支持）', '(unsupported)')}` : ''}</option>
                  })}
                </select>
                {!effortSupported(cli, roleModelId, effort, supportedEfforts) && <p className="provider-mapping__error" role="alert">{t('该模型不支持当前推理强度。', 'This model does not support the selected reasoning effort.')}</p>}
              </div>
            })}
            {effortHint && <p className="provider-help">{effortHint}</p>}
            <ContextWindowPicker
              value={contextMode}
              models={contextChoices}
              selectedModelIds={selectedModelIds}
              disabled={busy}
              onChange={mode => { contextTouched.current = true; setContextMode(mode); onContextWindowChange?.(mode) }}
            />
            {contextWindowUnknown && <p className="provider-help provider-context-unknown" role="status">{t('服务未提供完整的窗口信息，将沿用 CLI 默认设置。', 'The service did not provide complete window information; CLI default settings will be used.')}</p>}
          </>}
        </div>
        <p className="provider-help">ⓘ {existing ? t('保存修改不会重新连接运行中的会话。', 'Saving does not reconnect running sessions.') : t('仅保存不会切换；添加并使用会更新默认供应商。', 'Save only keeps the configuration; Add and use changes the default.')}</p>
      </>}
      {failure && <div ref={errorNode} tabIndex={-1} className="provider-inline-error" role="alert">{failure}</div>}
      {confirm && <div ref={confirmNode} className="provider-confirm" role="alert"><strong>{confirm === 'delete' ? t(`删除「${existing?.name}」？`, `Delete “${existing?.name}”?`) : t('有尚未保存的修改', 'You have unsaved changes')}</strong><p>{confirm === 'delete' ? t('此操作移除保存的配置，不会注销账户。', 'This removes the saved configuration, not your account.') : t('离开后，这次填写的内容将被丢弃。', 'Leaving discards this draft.')}</p><button type="button" onClick={() => setConfirm(undefined)}>{t('继续编辑','Keep editing')}</button><button type="button" onClick={() => { if (confirm === 'delete' && existing) void run(() => onDelete(existing)); else if (confirm === 'back') { backToPick() } else onClose() }} disabled={busy}>{confirm === 'delete' ? t('确认删除','Delete') : t('放弃修改','Discard')}</button></div>}
      </div>
      <footer className="provider-setup__footer">
        {existing && !existing.builtIn && existing.id !== activeId && <button type="button" className="provider-delete" disabled={busy} onClick={() => setConfirm('delete')}>{t('删除','Delete')}</button>}
        <span />
        {step === 'form' && !officialMode ? <>
          <button type="button" className={existing ? 'primary' : undefined} disabled={busy || effortInvalid} onClick={() => save(false)}>{busy ? t('保存中…','Saving…') : existing ? t('保存','Save') : t('仅保存','Save only')}</button>
          {existing
            ? <button type="button" disabled={busy || effortInvalid} onClick={() => save(true)}>{busy ? t('保存中…','Saving…') : t('保存并切换','Save and switch')}</button>
            : <button type="button" className="primary" disabled={busy || effortInvalid} onClick={() => save(true)}>{busy ? t('处理中…','Working…') : t('添加并使用','Add and use')}</button>}
        </> : <>
          <button type="button" disabled={busy} onClick={requestClose}>{t('取消','Cancel')}</button>
          {!existing && <button type="button" className="primary" disabled={busy || (step === 'pick' && (!selected || subscriptionBlocked)) || (officialMode && !official)} onClick={() => { if (step === 'pick') choose(); else if (officialMode && official) void run(() => onUseOfficial(official)) }}>{busy ? t('处理中…','Working…') : step === 'pick' ? t('下一步','Next') : t('使用官方服务','Use official service')}</button>}
        </>}
      </footer>
    </div>
  </div>
}

function effortChoices(cli: ProviderCli, model: string, current: ReasoningEffort, supportedEfforts?: readonly Exclude<ReasoningEffort, 'follow'>[]): ReasoningEffort[] {
  const choices: ReasoningEffort[] = ['follow', ...(supportedEfforts ? [...new Set(supportedEfforts)] : defaultEfforts(cli, model))]
  if (current !== 'follow' && !choices.includes(current)) choices.push(current)
  return choices
}

function effortSupported(cli: ProviderCli, model: string, effort: ReasoningEffort, supportedEfforts?: readonly Exclude<ReasoningEffort, 'follow'>[]) {
  return effort === 'follow' || (supportedEfforts ? supportedEfforts.includes(effort as Exclude<ReasoningEffort, 'follow'>) : defaultEfforts(cli, model).includes(effort as Exclude<ReasoningEffort, 'follow'>))
}

function defaultEfforts(cli: ProviderCli, model: string): Exclude<ReasoningEffort, 'follow'>[] {
  if (cli === 'codex' || /\b(gpt|codex|o[1-9])\b/i.test(model)) return ['low', 'medium', 'high', 'xhigh']
  return ['low', 'medium', 'high', 'xhigh', 'max']
}

function effortName(effort: ReasoningEffort, zh: boolean) {
  return effort === 'follow' ? zh ? '跟随会话' : 'Follow session'
    : effort === 'low' ? zh ? '低' : 'Low'
      : effort === 'medium' ? zh ? '中' : 'Medium'
        : effort === 'high' ? zh ? '高' : 'High'
          : effort === 'xhigh' ? zh ? '极高' : 'XHigh'
            : zh ? '最大' : 'Max'
}

function isOfficialEndpoint(cli: ProviderCli, endpoint: string) {
  const official = cli === 'claude-code' ? 'https://api.anthropic.com' : 'https://api.openai.com/v1'
  return endpoint.replace(/\/+$/, '') === official
}

function metadataSnapshot(models: ProviderModelMetadata[] | undefined, explicit = models !== undefined) {
  return JSON.stringify({ explicit, models: models ?? [] })
}

function providerModelWindowText(model: ProviderModelMetadata, _zh: boolean) {
  const tokens = model.contextWindow ?? model.maxContextWindow
  return tokens && tokens > 0 ? `${tokens.toLocaleString('en-US')} tokens` : _zh ? '窗口未知' : 'Window unknown'
}
