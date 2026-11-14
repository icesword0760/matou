import { useEffect, useState } from 'react'

import type {
  AgentModelPreferences,
  AgentModelProvider,
  AgentModelSettingsSnapshot,
  CompactionEffort,
  ModelContextChoice
} from '@matou/contracts'

import { useLocale } from '../i18n/LocaleProvider'
import type { ProviderConfigClient } from './ModelSwitchSettings'
import './agent-model-settings.css'

const MODEL_OPTION_PREFIX = 'agent-compaction-model:'
const MANUAL_MODEL_OPTION = '__agent_compaction_manual__'

export function AgentModelSettings({
  client,
  providerRevision,
  activeProviderId
}: {
  client: ProviderConfigClient | null
  providerRevision?: number | undefined
  activeProviderId?: string | undefined
}) {
  const zh = useLocale() === 'zh-CN'
  const t = (cn: string, en: string) => zh ? cn : en
  const [snapshot, setSnapshot] = useState<AgentModelSettingsSnapshot>()
  const [draft, setDraft] = useState<AgentModelPreferences>()
  const [saving, setSaving] = useState(false)
  const [unavailable, setUnavailable] = useState(false)
  const [loadError, setLoadError] = useState('')
  const [saveError, setSaveError] = useState('')
  const [saved, setSaved] = useState(false)
  const [retry, setRetry] = useState(0)
  const [manualModel, setManualModel] = useState(false)

  useEffect(() => {
    let active = true
    setLoadError('')
    setUnavailable(false)
    if (!client) {
      setSnapshot(undefined); setDraft(undefined)
      setLoadError(t('设置服务未连接，请重试。', 'Settings service is disconnected. Please retry.'))
      return
    }
    void client.request('agent-model-settings.get', {}).then(result => {
      if (!active) return
      if (!isAgentModelSettingsSnapshot(result)) {
        setUnavailable(true)
        return
      }
      setSnapshot(result)
      setDraft(current => current ? {
        contextWindows: { ...result.preferences.contextWindows },
        compaction: { ...current.compaction }
      } : copyPreferences(result.preferences))
    }).catch(() => {
      if (active && !snapshot) {
        setLoadError(t('无法读取压缩模型设置，请重试。', 'Could not load compaction model settings. Please retry.'))
      }
    })
    return () => { active = false }
  }, [client, retry, zh, providerRevision])

  const providers = (snapshot?.providers ?? []).filter(provider => provider.cli === 'claude-code')
  const allProviders = snapshot?.providers ?? []
  const availableCompactionProviders = providers.filter(provider => provider.compactionAvailable)
  const selectedCompactionProvider = allProviders.find(provider => provider.id === draft?.compaction.providerId)
  const selectedCompactionModel = selectedCompactionProvider?.models.find(model => model.id === draft?.compaction.model)
  const currentModelId = draft?.compaction.model?.trim() ?? ''
  const currentModelMissing = Boolean(currentModelId && !selectedCompactionProvider?.models.some(model => model.id === currentModelId))
  const supportedEfforts = selectedCompactionModel?.supportedEfforts ?? defaultEfforts(selectedCompactionProvider, selectedCompactionModel, draft?.compaction.model)
  const effortChoices = withCurrentEffort(supportedEfforts, draft?.compaction.effort)
  const effortFallback = draft?.compaction.mode === 'custom' &&
    Boolean(selectedCompactionProvider?.compactionAvailable && selectedCompactionProvider.cli === 'claude-code') &&
    Boolean(draft.compaction.model?.trim()) &&
    selectedCompactionModel?.supportedEfforts === undefined
  const invalidProviderSelection = draft?.compaction.mode === 'custom' && draft.compaction.providerId &&
    !availableCompactionProviders.some(provider => provider.id === draft.compaction.providerId)
    ? selectedCompactionProvider?.name ?? draft.compaction.providerId
    : undefined
  const manualModelActive = manualModel && Boolean(draft?.compaction.providerId) && !invalidProviderSelection
  const modelSelectValue = manualModelActive
    ? MANUAL_MODEL_OPTION
    : currentModelId ? `${MODEL_OPTION_PREFIX}${currentModelId}` : ''
  const customValidation = draft?.compaction.mode !== 'custom' ? '' : !draft.compaction.providerId
    ? t('请选择压缩供应商。', 'Choose a compaction provider.')
    : !selectedCompactionProvider
      ? t('已保存的压缩供应商已不存在；原选择已保留，请重新选择。', 'The saved compaction provider no longer exists. Its selection was kept; choose another provider.')
      : !selectedCompactionProvider.compactionAvailable || selectedCompactionProvider.cli !== 'claude-code'
        ? t('该供应商当前不支持独立压缩；请重新选择。', 'This provider does not support separate compaction. Choose another provider.')
        : !draft.compaction.model?.trim()
          ? t('请选择或输入压缩模型。', 'Choose or enter a compaction model.')
          : draft.compaction.effort !== 'follow' && !supportedEfforts.includes(draft.compaction.effort)
            ? t('当前推理强度不受该模型支持；原选择已保留，请重新选择。', 'The selected reasoning effort is not supported by this model. Its value was kept; choose another effort.')
            : ''
  const dirty = Boolean(snapshot && draft && JSON.stringify(snapshot.preferences.compaction) !== JSON.stringify(draft.compaction))
  const busy = (!snapshot && !loadError) || saving
  const crossProvider = Boolean(
    draft?.compaction.mode === 'custom' &&
    draft.compaction.providerId &&
    activeProviderId &&
    draft.compaction.providerId !== activeProviderId
  )

  const persistCompaction = async (nextDraft: AgentModelPreferences) => {
    if (!client || !snapshot || saving) return
    setSaving(true); setSaveError(''); setSaved(false)
    try {
      const latest = await client.request('agent-model-settings.get', {}).catch(() => snapshot)
      const baseWindows = isAgentModelSettingsSnapshot(latest) ? latest.preferences.contextWindows : snapshot.preferences.contextWindows
      const payload: AgentModelPreferences = {
        contextWindows: { ...baseWindows },
        compaction: { ...nextDraft.compaction }
      }
      const result = await client.request('agent-model-settings.set', payload) as AgentModelSettingsSnapshot
      setSnapshot(result)
      setDraft(copyPreferences(result.preferences))
      setSaved(true)
    } catch {
      setSaveError(t('保存未确认，请重试。', 'Save was not confirmed. Please retry.'))
    } finally { setSaving(false) }
  }

  const selectCompactionMode = (mode: 'follow' | 'custom') => {
    if (!draft) return
    const nextDraft: AgentModelPreferences = {
      ...draft,
      compaction: { ...draft.compaction, mode }
    }
    setDraft(nextDraft)
    setSaved(false); setSaveError('')
    if (mode === 'follow' && snapshot && snapshot.preferences.compaction.mode !== 'follow') {
      void persistCompaction(nextDraft)
    }
  }

  const updateCompaction = (patch: Partial<AgentModelPreferences['compaction']>) => {
    setDraft(current => current ? { ...current, compaction: { ...current.compaction, ...patch } } : current)
    setSaved(false); setSaveError('')
  }

  const selectCompactionProvider = (providerId: string) => {
    const provider = availableCompactionProviders.find(row => row.id === providerId)
    setManualModel(false)
    setDraft(current => {
      if (!current) return current
      const compaction = { ...current.compaction, model: provider?.models[0]?.id ?? '' }
      if (providerId) compaction.providerId = providerId
      else delete compaction.providerId
      return { ...current, compaction }
    })
    setSaved(false); setSaveError('')
  }

  const save = () => {
    if (!draft || customValidation) return
    void persistCompaction(draft)
  }

  if (unavailable) return null

  return <section className="agent-model-settings" aria-label={t('对话压缩模型设置', 'Compaction model settings')} aria-busy={busy}>
    {!snapshot && !loadError && <p>{t('正在加载压缩模型设置…', 'Loading compaction model settings…')}</p>}
    {loadError && <div className="agent-model-settings__load-error" role="alert">{loadError}<button type="button" onClick={() => setRetry(value => value + 1)}>{t('重试', 'Retry')}</button></div>}
    {snapshot && draft && <section className="agent-model-settings__section" aria-labelledby="agent-compaction-heading">
      <div className="agent-model-settings__header">
        <div>
          <h2 id="agent-compaction-heading">{t('对话压缩模型', 'Compaction model')}</h2>
          <p>{t(
            '长对话触发 /compact 时，默认跟随会话模型；也可单独指定轻量快模型生成摘要，减少高推理档位下的等待。',
            'Follows the session model during /compact by default, or use a lighter model for faster summaries without changing your working model.'
          )}</p>
        </div>
        {saved && draft.compaction.mode === 'follow' && <span className="agent-model-settings__saved" role="status">{t(
          '已保存，新会话生效',
          'Saved · applies to new sessions'
        )}</span>}
      </div>

      <div className="agent-model-settings__card">
        <fieldset className="agent-model-settings__fieldset">
          <legend className="visually-hidden">{t('压缩配置', 'Compaction configuration')}</legend>
          <div className="agent-model-settings__choices agent-model-settings__choices--two">
            <label className={`agent-model-settings__choice${draft.compaction.mode === 'follow' ? ' is-selected' : ''}`}>
              <input type="radio" name="agent-compaction-mode" value="follow" checked={draft.compaction.mode === 'follow'} disabled={saving} onChange={() => selectCompactionMode('follow')} />
              <span><strong>{t('跟随会话', 'Follow session')}</strong><small>{t('使用会话当前的模型和推理强度', 'Use the session model and reasoning effort')}</small></span>
            </label>
            <label className={`agent-model-settings__choice${draft.compaction.mode === 'custom' ? ' is-selected' : ''}`}>
              <input type="radio" name="agent-compaction-mode" value="custom" checked={draft.compaction.mode === 'custom'} disabled={saving} onChange={() => selectCompactionMode('custom')} />
              <span><strong>{t('独立配置', 'Separate configuration')}</strong><small>{t('仅影响压缩请求，不改变工作模型', 'Affects compaction requests only')}</small></span>
            </label>
          </div>
        </fieldset>

        {draft.compaction.mode === 'custom' && <div className="agent-model-settings__custom-body">
          <div className="agent-model-settings__grid">
            <label htmlFor="agent-compaction-provider">{t('压缩供应商', 'Compaction provider')}
              <select id="agent-compaction-provider" aria-label={t('压缩供应商', 'Compaction provider')} value={draft.compaction.providerId ?? ''} disabled={saving} onChange={event => selectCompactionProvider(event.target.value)} aria-describedby="agent-compaction-help">
                <option value="">{t('请选择供应商', 'Choose a provider')}</option>
                {availableCompactionProviders.map(provider => <option key={provider.id} value={provider.id}>{provider.name}</option>)}
                {invalidProviderSelection && <option value={draft.compaction.providerId} disabled>{invalidProviderSelection}{t('（不可用于压缩）', ' (unavailable for compaction)')}</option>}
              </select>
            </label>
            <label htmlFor="agent-compaction-model">{t('压缩模型', 'Compaction model')}
              {manualModelActive ? <div className="agent-model-settings__manual-model">
                <input id="agent-compaction-model" aria-label={t('压缩模型', 'Compaction model')} value={draft.compaction.model ?? ''} disabled={saving || Boolean(invalidProviderSelection)} onChange={event => updateCompaction({ model: event.target.value })} aria-describedby="agent-compaction-help" autoComplete="off" />
                <button type="button" disabled={saving} onClick={() => setManualModel(false)}>{t('从列表选择', 'Choose from list')}</button>
              </div> : <select id="agent-compaction-model" aria-label={t('压缩模型', 'Compaction model')} value={modelSelectValue} title={currentModelId || undefined} disabled={saving || !draft.compaction.providerId || Boolean(invalidProviderSelection)} onChange={event => {
                const next = event.target.value
                if (next === MANUAL_MODEL_OPTION) { setManualModel(true); return }
                setManualModel(false)
                updateCompaction({ model: next === '' ? '' : next.slice(MODEL_OPTION_PREFIX.length) })
              }} aria-describedby="agent-compaction-help">
                <option value="" disabled>{t('请选择压缩模型', 'Choose a compaction model')}</option>
                {selectedCompactionProvider?.models.map(model => <option key={model.id} value={`${MODEL_OPTION_PREFIX}${model.id}`} title={model.id}>{model.name || model.id}</option>)}
                {currentModelMissing && <option value={`${MODEL_OPTION_PREFIX}${currentModelId}`} title={currentModelId}>{currentModelId}{t('（不在当前列表中）', ' (not in this list)')}</option>}
                <option value={MANUAL_MODEL_OPTION}>{t('手动填写模型 ID…', 'Enter model ID manually…')}</option>
              </select>}
            </label>
            <label htmlFor="agent-compaction-effort">{t('推理强度', 'Reasoning effort')}
              <select id="agent-compaction-effort" aria-label={t('推理强度', 'Reasoning effort')} value={draft.compaction.effort} disabled={saving} onChange={event => updateCompaction({ effort: event.target.value as CompactionEffort })} aria-describedby="agent-compaction-help">
                {effortChoices.map(effort => <option key={effort} value={effort}>{effortLabel(effort, t)}</option>)}
              </select>
            </label>
          </div>
          {effortFallback && <p className="agent-model-settings__fallback-note">{t(
            '推理强度选项为候选，实际可用性以供应商支持为准。',
            'Reasoning effort options are candidates; actual availability depends on provider support.'
          )}</p>}
          {customValidation && <p className="agent-model-settings__validation" role="alert">{customValidation}</p>}
          <p id="agent-compaction-help" className={crossProvider ? 'agent-model-settings__cross-note' : undefined}>
            {crossProvider
              ? t('跨供应商压缩时，当前对话历史会交给所选供应商；不会修改会话的工作模型。', 'With another provider, the current conversation history is sent to that provider; the session working model stays unchanged.')
              : t('仅压缩请求使用所选模型与推理强度，不会修改会话的工作模型。', 'Only compaction requests use the chosen model and effort; the session working model stays unchanged.')}
          </p>
          <div className="agent-model-settings__actions">
            <button type="button" disabled={!client || saving || Boolean(customValidation) || !dirty} onClick={save}>
              {saving ? t('保存中…', 'Saving…') : t('保存', 'Save')}
            </button>
            {saved && <span role="status">{t(
              '已保存，下次启动或恢复会话时生效；正在运行的会话不会被打断。',
              'Saved. Applies to the next launched or resumed session; running sessions are not interrupted.'
            )}</span>}
          </div>
        </div>}

        {draft.compaction.mode === 'follow' && dirty && <div className="agent-model-settings__actions">
          <button type="button" disabled={!client || saving} onClick={() => void persistCompaction(draft)}>
            {saving ? t('保存中…', 'Saving…') : t('保存', 'Save')}
          </button>
        </div>}
      </div>
      {saveError && <div role="alert">{saveError}</div>}
    </section>}
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

function copyPreferences(preferences: AgentModelPreferences): AgentModelPreferences {
  return { contextWindows: { ...preferences.contextWindows }, compaction: { ...preferences.compaction } }
}

function effortLabel(effort: CompactionEffort, t: (cn: string, en: string) => string) {
  if (effort === 'follow') return t('跟随会话', 'Follow session')
  if (effort === 'low') return t('低', 'Low')
  if (effort === 'medium') return t('中', 'Medium')
  if (effort === 'high') return t('高', 'High')
  if (effort === 'xhigh') return t('极高', 'XHigh')
  return t('最大', 'Max')
}

function withCurrentEffort(supported: Exclude<CompactionEffort, 'follow'>[], current?: CompactionEffort): CompactionEffort[] {
  const choices: CompactionEffort[] = ['follow', ...supported]
  if (current && current !== 'follow' && !choices.includes(current)) choices.push(current)
  return choices
}

/** Metadata wins; model families provide the fallback used before metadata arrives. */
function defaultEfforts(provider: AgentModelProvider | undefined, model: ModelContextChoice | undefined, customModel?: string): Exclude<CompactionEffort, 'follow'>[] {
  const label = `${provider?.id ?? ''} ${provider?.name ?? ''} ${model?.id ?? ''} ${model?.name ?? ''} ${customModel ?? ''}`
  if (/\b(gpt|codex|o[1-9])\b/i.test(label) || provider?.id?.includes('chatgpt')) return ['low', 'medium', 'high', 'xhigh']
  if (/claude|opus|sonnet|haiku/i.test(label)) return ['low', 'medium', 'high', 'xhigh', 'max']
  return ['low', 'medium', 'high', 'xhigh']
}
