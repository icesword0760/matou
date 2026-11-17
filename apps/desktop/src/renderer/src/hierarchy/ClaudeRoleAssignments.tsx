import { useEffect, useRef, useState } from 'react'
import type { AgentModelSettingsSnapshot, ClaudeModelRole, ClaudeRoleAssignments as Assignments, ProviderConfigView, SubscriptionAccountDetails } from '@matou/contracts'
import type { ProviderConfigClient } from './ModelSwitchSettings'
import { useLocale } from '../i18n/LocaleProvider'
import { allocationModels, allocationProblem, CLAUDE_ROLES, defaultAllocationModel } from './claude-role-assignment-options'
import './claude-role-assignments.css'

const copy = (value: Assignments = {}): Assignments => structuredClone(value)
const key = (value: Assignments = {}) => JSON.stringify(CLAUDE_ROLES.map(r => value[r] ?? null))
export function ClaudeRoleAssignments({ client, providers, activeProviderId, snapshot, accounts, loadError, onRetry, onSaved }: {
  client: ProviderConfigClient | null
  providers: readonly ProviderConfigView[]
  activeProviderId?: string | undefined
  snapshot?: AgentModelSettingsSnapshot | undefined
  accounts: Record<string, SubscriptionAccountDetails>
  loadError?: string | undefined
  onRetry(): void
  onSaved(snapshot: AgentModelSettingsSnapshot): void
}) {
  const zh = useLocale() === 'zh-CN', t = (cn: string, en: string) => zh ? cn : en
  const [draft, setDraft] = useState<Assignments>()
  const baseline = useRef<Assignments>({})
  const [manual, setManual] = useState<Partial<Record<ClaudeModelRole, boolean>>>({})
  const [saving, setSaving] = useState(false)
  const savingRef = useRef(false)
  const [saved, setSaved] = useState(false)
  const [saveError, setSaveError] = useState('')
  useEffect(() => {
    if (!snapshot) return
    const next = copy(snapshot.preferences.claudeRoleAssignments)
    const previous = baseline.current
    setDraft(current => current === undefined || key(current) === key(previous) ? copy(next) : current)
    baseline.current = next
  }, [snapshot])
  const change = (role: ClaudeModelRole, value?: {providerId:string;model:string}) => {
    setSaved(false);setSaveError('')
    setDraft(current => { const next = copy(current); if(value) next[role]=value;else delete next[role];return next })
  }
  const mixed = Boolean(draft && Object.keys(draft).length)
  const problems = CLAUDE_ROLES.map(role => {
    if (!mixed) return undefined
    const assigned = draft?.[role]
    const provider = providers.find(p => p.id === (assigned?.providerId ?? activeProviderId))
    const model = assigned?.model ?? defaultAllocationModel(role, provider, providers, snapshot)?.id ?? ''
    const problem = allocationProblem(provider, model, snapshot, accounts)
    return !assigned && problem === 'empty' ? 'catalog' : problem
  })
  const dirty = draft !== undefined && key(draft) !== key(baseline.current)
  const unavailable = !client || !snapshot || !draft || !providers.length || Boolean(loadError)
  const invalid = problems.some(Boolean)
  const persist = async () => {
    if (!client || !draft || unavailable || invalid || !dirty || savingRef.current) return
    savingRef.current=true;setSaving(true);setSaveError('');setSaved(false)
    try {
      const result = await client.request('agent-model-settings.patch-assignments',{assignments:copy(draft)}) as AgentModelSettingsSnapshot
      if (!result?.preferences || !Array.isArray(result.providers)) throw new Error('Unconfirmed settings response')
      baseline.current=copy(result.preferences.claudeRoleAssignments);setDraft(copy(baseline.current));setSaved(true);onSaved(result)
    } catch { setSaveError(t('保存未确认，请重试。原选择已保留。','Save was not confirmed. Your selections were kept; please retry.')) }
    finally {savingRef.current=false;setSaving(false)}
  }
  const problemCopy = (code: string | undefined) => ({
    provider:t('供应商已不存在；原选择已保留，请重新选择。','Provider no longer exists. Your selection was kept; choose another provider.'),
    key:t('API Key 未配置，请先编辑供应商。','API key is missing. Edit the provider first.'),
    account:t('账号已失效，请重新登录或选择其他供应商。','Account is unavailable. Sign in again or choose another provider.'),
    empty:t('请选择或输入模型。','Choose or enter a model.'),
    model:t('模型已不可用；原选择已保留，请重新选择。','Model is unavailable. Your selection was kept; choose another model.'),
    catalog:t('模型目录暂不可用，请重试。','Model directory is unavailable. Please retry.'),
    invalid:t('模型 ID 无效，请检查输入。','Invalid model ID. Please check your input.')
  }[code ?? ''])
  return <section className="model-allocation" aria-label={t('模型分配','Model assignments')} aria-busy={saving}>
    <section className="model-allocation__section" aria-labelledby="model-allocation-heading">
      <header className="model-allocation__header"><div><h2 id="model-allocation-heading">{t('模型分配','Model assignments')}</h2><p>{t('为不同档位指定供应商和模型，未指定时沿用默认供应商。','Assign a provider and model to each tier. Unassigned tiers follow the default provider.')}</p></div></header>
      {(loadError || !client) && <p role="alert" className="model-allocation__error">{loadError || t('设置服务未连接。','Settings service is disconnected.')} <button type="button" onClick={onRetry}>{t('重试','Retry')}</button></p>}
      {!snapshot && !loadError && client && <p role="status">{t('正在读取模型分配…','Loading model assignments…')}</p>}
      {snapshot && !providers.length && <p role="alert">{t('没有可用供应商，请先添加供应商。','No providers are available. Add a provider first.')}</p>}
      <div className="model-allocation__card">
        <div className="model-allocation__head" aria-hidden="true"><span>{t('档位','Tier')}</span><span>{t('供应商 / 账号','Provider / account')}</span><span>{t('模型','Model')}</span></div>
        {CLAUDE_ROLES.map((role,index) => {
          const title = role[0]!.toUpperCase()+role.slice(1), assigned=draft?.[role]
          const provider=providers.find(p=>p.id===(assigned?.providerId ?? activeProviderId))
          const models=allocationModels(provider,snapshot), selected=assigned?.model ?? ''
          const defaultModel=defaultAllocationModel(role,provider,providers,snapshot)
          const problem=problems[index]
          const manualAllowed=provider && !provider.subscription && !provider.builtIn && provider.modelMetadata===undefined
          const manualActive=Boolean(manualAllowed && (manual[role] || selected && !models.some(m=>m.id===selected)))
          const unavailableModel=selected && !models.some(m=>m.id===selected)
          const account=provider?.subscription?.account ?? accounts[provider?.id ?? '']?.account
          const accountText=account ? [account.email,account.plan,account.status==='ready'?t('已登录','Signed in'):t('需要重新登录','Sign in required')].filter(Boolean).join(' · ') : provider ? provider.endpoint.replace(/^https?:\/\//,'') : ''
          return <div key={role} className={`model-allocation__row${problem?' is-invalid':''}`}>
            <div className="model-allocation__role"><strong>{title}</strong></div>
            <div className="model-allocation__field" data-label={t('供应商 / 账号','Provider / account')}>
              <select id={`${role}-provider`} aria-label={`${title} ${t('供应商','provider')}`} aria-describedby={`${role}-target${problem?` ${role}-error`:''}`} disabled={unavailable || saving} value={assigned?.providerId ?? ''} onChange={e=>{setManual(current=>({...current,[role]:false}));change(role,e.target.value?{providerId:e.target.value,model:''}:undefined)}}>
                <option value="">{t('沿用默认供应商','Follow default provider')}</option>
                {assigned && !provider && <option value={assigned.providerId}>{assigned.providerId} · {t('不可用','Unavailable')}</option>}
                {providers.map(p=><option key={p.id} value={p.id}>{p.name}</option>)}
              </select>
              <span className="model-allocation__account">{assigned ? accountText : `${provider?.name ?? t('默认供应商不可用','Default provider unavailable')} · ${t('默认供应商','Default provider')}`}</span>
            </div>
            <div className="model-allocation__field" data-label={t('模型','Model')}>
              <select id={`${role}-model`} aria-label={`${title} ${t('模型','model')}`} aria-invalid={Boolean(problem)} aria-describedby={`${role}-target${problem?` ${role}-error`:''}`} disabled={!assigned || !provider || unavailable || saving} value={!assigned?'':manualActive?'__manual__':selected} onChange={e=>{const value=e.target.value;setManual(current=>({...current,[role]:value==='__manual__'}));if(value!=='__manual__')change(role,{providerId:assigned!.providerId,model:value})}}>
                {!assigned ? <option value="">{defaultModel?.name ?? t(`${title} 原生默认档位（目录暂不可用）`,`${title} native default (directory unavailable)`)}</option> : <>
                  <option value="">{t('请选择模型','Choose a model')}</option>
                  {unavailableModel && !manualActive && <option value={selected}>{selected} · {t('不可用','Unavailable')}</option>}
                  {models.map(m=><option key={m.id} value={m.id}>{m.name}</option>)}
                  {manualAllowed && <option value="__manual__">{t('手动填写模型 ID…','Enter a model ID…')}</option>}
                </>}
              </select>
              {manualActive && <input aria-label={`${title} ${t('模型 ID','model ID')}`} value={selected} disabled={saving} onChange={e=>change(role,{providerId:assigned!.providerId,model:e.target.value})}/>}
            </div>
            <p className="model-allocation__target" id={`${role}-target`}>{assigned?t('当前目标：','Current target: '):t('最终：','Effective target: ')}{provider?.name ?? assigned?.providerId ?? '—'} · {assigned ? models.find(m=>m.id===selected)?.name ?? selected : defaultModel?.name ?? t('原生默认档位待确认','Native default tier pending')}</p>
            {problem && <p className="model-allocation__error" id={`${role}-error`} role="alert">{!assigned && `${t('沿用默认：','Default target: ')}${provider?.name ?? '—'} · ${defaultModel?.id || title}。`}{problemCopy(problem)}{!assigned && t('请为此档位重新选择供应商和模型。','Choose another provider and model for this tier.')}{problem==='catalog' && <button type="button" onClick={onRetry}>{t('重试','Retry')}</button>}</p>}
          </div>
        })}
      </div>
      <p className="model-allocation__privacy">{t('使用指定供应商时，请求所需的对话内容会发送给该供应商。','Conversation content needed for a request is sent to the assigned provider.')}</p>
      <div className="model-allocation__footer"><p className="model-allocation__scope">{t('新会话使用此分配，正在运行的会话不受影响','New sessions use these assignments. Running sessions are unaffected.')}</p>
        <div className="model-allocation__actions">
          <button type="button" disabled={!snapshot || saving} onClick={()=>{setDraft({});setManual({});setSaved(false);setSaveError('')}}>{t('恢复默认','Restore defaults')}</button>
          <button type="button" className="primary" disabled={unavailable || invalid || !dirty && !saved} aria-disabled={saving || !dirty} onClick={()=>void persist()}>{saving?t('保存中…','Saving…'):t('保存分配','Save assignments')}</button>
          {saved && <span className="model-allocation__status" role="status">{t('模型分配已保存，新会话将使用此配置','Model assignments saved. New sessions will use this configuration.')}</span>}
        </div>
        {saveError && <p className="model-allocation__error" role="alert">{saveError}</p>}
      </div>
    </section>
  </section>
}
