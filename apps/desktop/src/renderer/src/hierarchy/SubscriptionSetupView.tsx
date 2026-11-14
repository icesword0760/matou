import { useEffect, useRef } from 'react'

import type { ContextWindowMode, ModelContextChoice, ProviderConfigView, ReasoningEffort, SubscriptionAccountDetails, SubscriptionModel, SubscriptionProvider } from '@matou/contracts'

import { useLocale, useMessages } from '../i18n/LocaleProvider'
import { ContextWindowPicker } from './context-window-ui'
import { usableSubscriptionModels } from './subscription-settings'
import './provider-setup.css'

type Role = 'opus' | 'sonnet' | 'haiku'
const ROLES: readonly Role[] = ['opus', 'sonnet', 'haiku']

/**
 * Pure view for the subscription sign-in flow. It owns no request or business
 * state: the settings pane supplies the current phase, draft and callbacks.
 */
export function SubscriptionSetupView({
  provider, phase, existing, details, name, mapping, effortMapping, contextWindowMode, riskChecked, canReopen, reopening, busy, connected, failure, confirm, canDelete,
  onClose, onRiskChecked, onContinue, onLogin, onReopen, onCancel, onName, onMapping, onEffort, onContextWindowChange, onSave, onDelete, onKeepEditing, onConfirm, onRetryModels
}: {
  provider: SubscriptionProvider
  phase: 'risk' | 'login' | 'waiting' | 'error' | 'models'
  existing?: ProviderConfigView | undefined
  details?: SubscriptionAccountDetails | undefined
  name: string
  mapping: Record<Role, string>
  effortMapping: Record<Role, ReasoningEffort>
  contextWindowMode?: ContextWindowMode | undefined
  riskChecked: boolean
  canReopen: boolean
  reopening: boolean
  busy: boolean
  connected: boolean
  failure: string
  confirm?: 'close' | 'delete' | undefined
  canDelete: boolean
  onClose(): void
  onRiskChecked(value: boolean): void
  onContinue(): void
  onLogin(): void
  onReopen(): void
  onCancel(): void
  onName(value: string): void
  onMapping(role: Role, value: string): void
  onEffort(role: Role, value: ReasoningEffort): void
  onContextWindowChange?(mode: ContextWindowMode): void
  onSave(use: boolean): void
  onDelete(): void
  onKeepEditing(): void
  onConfirm(): void
  onRetryModels(): void
}) {
  const zh = useLocale() === 'zh-CN'
  const t = (cn: string, en: string) => zh ? cn : en
  const m = useMessages().hierarchyShell.modelSettings
  const app = provider === 'codex' ? 'ChatGPT' : 'Antigravity'
  const description = provider === 'codex'
    ? t('使用你的 ChatGPT 订阅额度，不需要 API Key', 'Use your ChatGPT subscription. No API key needed.')
    : t('使用 Google AI 订阅中的 Claude 与 Gemini 模型', 'Use Claude and Gemini models from your Google AI subscription.')
  const riskTitle = t('使用 Antigravity 账号有封号风险', 'Using an Antigravity account may get it suspended')
  const title = phase === 'risk' ? riskTitle
    : phase === 'models' ? existing ? t('编辑供应商', 'Edit provider') : t('添加供应商', 'Add provider')
      : existing ? t('重新登录', 'Sign in again') : t('添加供应商', 'Add provider')
  const startDisabled = busy || !connected
  const root = useRef<HTMLDivElement>(null)
  const opener = useRef<HTMLElement | null>(null)
  const confirmNode = useRef<HTMLDivElement>(null)
  const errorNode = useRef<HTMLDivElement>(null)
  // Generated media models and the review helper are not coding targets.
  const modelOptions = usableSubscriptionModels(details?.models ?? [])
  const modelsReady = modelOptions.length > 0
  const contextModels: ModelContextChoice[] = modelOptions.map(model => ({
    id: model.id,
    name: model.displayName ?? model.id,
    ...(model.contextWindow ? { defaultTokens: model.contextWindow } : {}),
    ...(model.maxContextWindow ? { maximumTokens: model.maxContextWindow } : {}),
    fixedTokens: 512_000
  }))
  const resolvedContextMode: ContextWindowMode = contextWindowMode ?? (provider === 'codex' ? '512k' : 'default')
  const saveDisabled = startDisabled || !modelsReady || ROLES.some(role => {
    const model = modelOptions.find(row => row.id === mapping[role])
    return !effortSupported(provider, model, effortMapping[role])
  })
  const effortSelectionUsesFallback = ROLES.some(role => {
    const model = modelOptions.find(row => row.id === mapping[role])
    return Boolean(model && model.supportedEfforts === undefined)
  })

  useEffect(() => {
    opener.current = document.activeElement as HTMLElement
    return () => { opener.current?.focus() }
  }, [])
  // One focus owner for the view: a confirmation wins, then a failure, then the
  // phase target. Waiting keeps its enabled Cancel reachable while the sign-in
  // request starts; every other step waits for `busy` to settle so a disabled
  // model-name input cannot swallow the first focus (and the next Tab).
  useEffect(() => {
    if (confirm) { confirmNode.current?.querySelector('button')?.focus(); confirmNode.current?.scrollIntoView?.({ block: 'nearest' }); return }
    if (failure) { errorNode.current?.focus(); errorNode.current?.scrollIntoView?.({ block: 'nearest' }); return }
    if (busy && phase !== 'waiting') return
    const target = root.current?.querySelector<HTMLElement>('[data-autofocus]')
    if (target && !target.matches(':disabled')) target.focus()
  }, [phase, busy, failure, confirm])

  const requestClose = () => {
    if (confirm) return
    if (busy && phase !== 'waiting') return
    onClose()
  }

  return <div className="provider-setup-layer" onPointerDown={e => { if (e.target === e.currentTarget) requestClose() }}>
    <div ref={root} className="provider-setup provider-setup--subscription" role="dialog" aria-modal="true" aria-label={title} aria-busy={busy}
      onKeyDown={e => {
        if (e.key === 'Escape') {
          e.preventDefault(); e.stopPropagation()
          if (confirm) onKeepEditing()
          else requestClose()
        }
        if (e.key !== 'Tab') return
        const controls = [...root.current!.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), a[href], summary')].filter(el => el.getClientRects().length > 0)
        const first = controls[0], last = controls.at(-1)
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus() }
        else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus() }
      }}>
      <header className="provider-setup__header">
        <div><h2>{title}</h2><p>{t('用于', 'For')} Claude Code</p></div>
        <button className="provider-icon-button" type="button" aria-label={t('关闭', 'Close')} onClick={requestClose} disabled={busy && phase !== 'waiting'}>×</button>
      </header>
      <div className="provider-setup__body">
        {phase === 'risk' ? <>
          <p className="provider-subscription__risk-body">{t('Google 的 Antigravity 条款不允许第三方工具使用该账号。被判定违规时，Antigravity 与 Gemini CLI 账号可能被停用，申诉机会有限。', 'Google\'s Antigravity terms do not allow third-party tools to use the account. If a violation is detected, your Antigravity and Gemini CLI accounts may be suspended, with limited appeals.')}</p>
          <label className="provider-risk">
            <input data-autofocus type="checkbox" checked={riskChecked} disabled={busy} onChange={e => onRiskChecked(e.target.checked)} />
            <span>{t('我了解风险，仍要继续', 'I understand the risk and want to continue')}</span>
          </label>
          {!connected && <p className="provider-help" role="status">{m.serviceConnecting}</p>}
        </> : <>
          {phase !== 'models' && <div className="provider-selected">
            <span className="provider-preset__icon">{provider === 'codex' ? 'C' : 'A'}</span>
            <div><strong>{app} {t('账号', 'account')}</strong><small>{description}</small></div>
          </div>}
          {phase === 'waiting' && <>
            <p className="provider-subscription__waiting" role="status">{t('等待浏览器完成登录…', 'Waiting for the browser to finish signing in…')}</p>
            <p className="provider-help"><button type="button" className="provider-link" disabled={!connected || !canReopen || reopening} onClick={onReopen}>{t('重新打开浏览器', 'Open browser again')}</button></p>
          </>}
          {phase === 'error' && <div ref={errorNode} tabIndex={-1} className="provider-inline-error" role="alert">{failure}</div>}
          {phase === 'models' && <div className="provider-subscription__models">
            <label className="provider-field">{t('供应商名称', 'Provider name')}
              <input data-autofocus value={name} disabled={busy} onChange={e => onName(e.target.value)} />
            </label>
            {modelsReady ? <>{ROLES.map(role => {
              const model = modelOptions.find(row => row.id === mapping[role])
              const known = Boolean(model)
              const label = role[0]!.toUpperCase() + role.slice(1)
              const invalidEffort = !effortSupported(provider, model, effortMapping[role])
              return <div className="provider-mapping" key={role} role="group" aria-label={label}>
                <span className="provider-mapping__role">{label}</span>
                <select aria-label={`${label} ${t('模型', 'model')}`} value={known ? mapping[role] : ''}
                  disabled={busy} onChange={e => onMapping(role, e.target.value)}>
                  {!known && <option value="" disabled>—</option>}
                  {modelOptions.map(model => <option key={model.id} value={model.id}>{model.displayName || model.id}</option>)}
                </select>
                <select aria-label={`${label} ${t('推理强度', 'reasoning effort')}`} value={effortMapping[role]}
                  disabled={busy} onChange={e => onEffort(role, e.target.value as ReasoningEffort)}>
                  {effortChoices(provider, model, effortMapping[role]).map(effort => <option key={effort} value={effort}>{effortName(effort, zh)}</option>)}
                </select>
                {invalidEffort && <p className="provider-mapping__error" role="alert">{t('该模型不支持当前推理强度。', 'This model does not support the selected reasoning effort.')}</p>}
              </div>
            })}
            {effortSelectionUsesFallback && <p className="provider-help">{t('推理强度选项为候选，实际可用性以供应商支持为准。', 'Reasoning effort options are candidates; actual availability depends on provider support.')}</p>}
            {onContextWindowChange && <ContextWindowPicker
              value={resolvedContextMode}
              models={contextModels}
              selectedModelIds={ROLES.map(role => mapping[role]).filter(Boolean)}
              disabled={busy}
              onChange={onContextWindowChange}
            />}
            </> : <div ref={errorNode} tabIndex={-1} className="provider-inline-error" role={failure ? 'alert' : 'status'}>
              <span>{t('没有读到可用的模型', 'No available models were found')}</span>
              {failure && <p className="provider-help">{failure}</p>}
              <p className="provider-help"><button type="button" className="provider-link" disabled={busy} onClick={onRetryModels}>{busy ? t('重试中…', 'Retrying…') : t('重试', 'Retry')}</button></p>
            </div>}
          </div>}
          {existing && phase !== 'models' && <p className="provider-confirm provider-subscription__replacement">{t('已有该类账号，重新登录会替换它，模型设置会保留。', 'You already have this account type. Signing in again replaces it and keeps your model settings.')}</p>}
          {!connected && <p className="provider-help" role="status">{m.serviceConnecting}</p>}
        </>}
        {phase !== 'error' && failure && !(phase === 'models' && !modelsReady) && <div ref={errorNode} tabIndex={-1} className="provider-inline-error" role="alert">{failure}</div>}
        {confirm && <div ref={confirmNode} className="provider-confirm" role="alert">
          <strong>{confirm === 'delete' ? t(`删除「${existing?.name ?? name}」？`, `Delete “${existing?.name ?? name}”?`) : t('有尚未保存的修改', 'You have unsaved changes')}</strong>
          <p>{confirm === 'delete' ? t('此操作移除保存的配置，不会注销账户。', 'This removes the saved configuration, not your account.') : t('离开后，这次填写的内容将被丢弃。', 'Leaving discards this draft.')}</p>
          <button type="button" onClick={onKeepEditing}>{t('继续编辑', 'Keep editing')}</button>
          <button type="button" onClick={onConfirm} disabled={busy}>{confirm === 'delete' ? t('确认删除', 'Delete') : t('放弃修改', 'Discard')}</button>
        </div>}
      </div>
      <footer className="provider-setup__footer">
        {existing && canDelete && phase !== 'waiting' && <button type="button" className="provider-delete" disabled={busy} onClick={onDelete}>{t('删除', 'Delete')}</button>}
        <span />
        {phase === 'risk' && <>
          <button type="button" disabled={busy} onClick={onClose}>{t('取消', 'Cancel')}</button>
          <button type="button" className="primary" disabled={startDisabled || !riskChecked} onClick={onContinue}>{busy ? t('处理中…', 'Working…') : t('继续', 'Continue')}</button>
        </>}
        {phase === 'login' && <>
          <button type="button" disabled={busy} onClick={onClose}>{t('取消', 'Cancel')}</button>
          <button type="button" className="primary" data-autofocus disabled={startDisabled} onClick={onLogin}>{t('在浏览器中登录', 'Sign in with browser')}</button>
        </>}
        {phase === 'waiting' && <button type="button" data-autofocus onClick={onCancel}>{t('取消', 'Cancel')}</button>}
        {phase === 'error' && <>
          <button type="button" disabled={busy} onClick={onClose}>{t('取消', 'Cancel')}</button>
          <button type="button" className="primary" data-autofocus disabled={startDisabled} onClick={onLogin}>{busy ? t('处理中…', 'Working…') : t('重试', 'Retry')}</button>
        </>}
        {phase === 'models' && <>
          <button type="button" className={existing ? 'primary' : undefined} disabled={saveDisabled} onClick={() => onSave(false)}>{busy && modelsReady ? t('保存中…', 'Saving…') : t('保存', 'Save')}</button>
          <button type="button" className={existing ? undefined : 'primary'} disabled={saveDisabled} onClick={() => onSave(true)}>{busy && modelsReady ? t('保存中…', 'Saving…') : t('保存并切换', 'Save and switch')}</button>
        </>}
      </footer>
    </div>
  </div>
}

function supportedEfforts(provider: SubscriptionProvider, model: SubscriptionModel | undefined): Exclude<ReasoningEffort, 'follow'>[] {
  if (model?.supportedEfforts !== undefined) return model.supportedEfforts
  const label = `${provider} ${model?.id ?? ''} ${model?.displayName ?? ''}`
  return !(provider === 'codex' || /\b(gpt|codex|o[1-9])\b/i.test(label)) && /claude|opus|sonnet|haiku/i.test(label)
    ? ['low', 'medium', 'high', 'xhigh', 'max']
    : ['low', 'medium', 'high', 'xhigh']
}

function effortChoices(provider: SubscriptionProvider, model: SubscriptionModel | undefined, current: ReasoningEffort): ReasoningEffort[] {
  const choices: ReasoningEffort[] = ['follow', ...supportedEfforts(provider, model)]
  if (current !== 'follow' && !choices.includes(current)) choices.push(current)
  return choices
}

function effortSupported(provider: SubscriptionProvider, model: SubscriptionModel | undefined, effort: ReasoningEffort) {
  return effort === 'follow' || supportedEfforts(provider, model).includes(effort as Exclude<ReasoningEffort, 'follow'>)
}

function effortName(effort: ReasoningEffort, zh: boolean) {
  return effort === 'follow' ? zh ? '跟随会话' : 'Follow session'
    : effort === 'low' ? zh ? '低' : 'Low'
      : effort === 'medium' ? zh ? '中' : 'Medium'
        : effort === 'high' ? zh ? '高' : 'High'
          : effort === 'xhigh' ? zh ? '极高' : 'XHigh'
            : zh ? '最大' : 'Max'
}
