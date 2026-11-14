import { useEffect, useRef, useState } from 'react'
import type { ContextWindowMode, ReasoningEffort, SubscriptionAccountDetails, SubscriptionLoginView, SubscriptionProvider, ProviderConfigInput, ProviderConfigView } from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import type { ProviderConfigClient } from './ModelSwitchSettings'
import { SubscriptionSetupView } from './SubscriptionSetupView'
import { subscriptionDefaults, usableSubscriptionModels } from './subscription-settings'

type Mapping = Record<'opus' | 'sonnet' | 'haiku', string>
type Role = keyof Mapping
type EffortMapping = Record<Role, ReasoningEffort>
const ROLES: readonly Role[] = ['opus', 'sonnet', 'haiku']
type LocalizedText = { zh: string; en: string }
/** Covers router cold start and account reconciliation; other requests keep the default. */
const LOGIN_START_TIMEOUT_MS = 30_000
/** Runtime adds these fields; read them structurally so older payloads still work. */
const localizedText = (value: unknown, locale: 'zh-CN' | 'en'): string | undefined => {
  if (!value || typeof value !== 'object') return undefined
  const text = (value as Partial<LocalizedText>)[locale === 'zh-CN' ? 'zh' : 'en']
  return typeof text === 'string' && text.trim() ? text : undefined
}
export function SubscriptionSetupDialog({ provider, existing, details: initialDetails, relogin, riskAccepted, activeId, client, contextWindowMode, onContextWindowChange, onClose, onRiskAccepted, onDetails, onSave, onDelete }: {
  provider: SubscriptionProvider; existing?: ProviderConfigView | undefined; details?: SubscriptionAccountDetails | undefined
  relogin?: boolean | undefined; riskAccepted: boolean; activeId?: string | undefined; client: ProviderConfigClient | null
  contextWindowMode?: ContextWindowMode | undefined; onContextWindowChange?(mode: ContextWindowMode): void
  onClose(): void; onRiskAccepted(): void; onDetails(details: SubscriptionAccountDetails): void
  onSave(input: ProviderConfigInput, use: boolean): Promise<void>; onDelete(provider: ProviderConfigView): Promise<void>
}) {
  const locale = useLocale(), t = (cn: string, en: string) => locale === 'zh-CN' ? cn : en
  const editing = Boolean(existing && !relogin)
  const initialContextMode: ContextWindowMode = contextWindowMode ?? (provider === 'codex' ? '512k' : 'default')
  const [contextMode, setContextMode] = useState<ContextWindowMode>(initialContextMode)
  const [phase, setPhase] = useState<'risk' | 'login' | 'waiting' | 'error' | 'models'>(editing ? 'models' : provider === 'antigravity' && !riskAccepted ? 'risk' : 'login')
  const [details, setDetails] = useState(initialDetails ?? (existing?.subscription ? { account: existing.subscription.account, models: existing.subscription.modelMetadata } : undefined))
  const [name, setName] = useState(existing?.name ?? '')
  const initialMapping = { opus: existing?.modelMapping?.opus ?? '', sonnet: existing?.modelMapping?.sonnet ?? '', haiku: existing?.modelMapping?.haiku ?? '' }
  const [mapping, setMapping] = useState<Mapping>(initialMapping)
  const initialEffortMapping: EffortMapping = { opus: existing?.effortMapping?.opus ?? 'follow', sonnet: existing?.effortMapping?.sonnet ?? 'follow', haiku: existing?.effortMapping?.haiku ?? 'follow' }
  const [effortMapping, setEffortMapping] = useState<EffortMapping>(initialEffortMapping)
  const baseline = useRef(JSON.stringify({ name: existing?.name ?? '', mapping: initialMapping, effortMapping: initialEffortMapping }))
  const [canReopen, setCanReopenState] = useState(false), [reopening, setReopening] = useState(false)
  const canReopenRef = useRef(false)
  const setCanReopen = (value: boolean) => { canReopenRef.current = value; if (mounted.current) setCanReopenState(value) }
  const reopenWorking = useRef(false), loginStatus = useRef<SubscriptionLoginView['status'] | undefined>(undefined)
  const canReopenLogin = () => loginStatus.current === 'waiting' || loginStatus.current === 'exchanging' || (loginStatus.current === 'failed' && canReopenRef.current)
  const [riskChecked, setRiskChecked] = useState(false), [busy, setBusy] = useState(false), [failure, setFailure] = useState('')
  const [confirm, setConfirm] = useState<'close' | 'delete'>()
  const mounted = useRef(true), locked = useRef(false), attempt = useRef<string | undefined>(undefined), generation = useRef(0), submitted = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined), cancelled = useRef(new Set<string>())
  // Settings can finish its model metadata read after this editor has opened.
  // Refresh capabilities without replacing the user's model/effort draft.
  useEffect(() => {
    if (editing && phase === 'models' && !attempt.current && initialDetails) setDetails(initialDetails)
  }, [editing, phase, initialDetails])
  const stopPolling = () => { generation.current++; clearTimeout(timer.current); reopenWorking.current = false; if (mounted.current) setReopening(false) }
  const cancel = async (id: string) => {
    if (!client || cancelled.current.has(id)) return undefined
    cancelled.current.add(id)
    try { return await client.request('provider-config.subscription.login-cancel', { attemptId: id }) as SubscriptionLoginView }
    catch (error) { cancelled.current.delete(id); throw error }
  }
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false; stopPolling(); if (attempt.current && !submitted.current) void cancel(attempt.current).catch(() => {}) }
  }, [client])
  const errorText = (error: unknown) => {
    if (error && typeof error === 'object') {
      const text = localizedText((error as { messageLocalized?: unknown }).messageLocalized, locale)
      if (text) return text
    }
    return error instanceof Error ? error.message : t('操作未完成，请重试。', 'Operation did not finish. Try again.')
  }
  const viewReason = (view: SubscriptionLoginView) =>
    localizedText((view as SubscriptionLoginView & { reasonLocalized?: unknown }).reasonLocalized, locale) ?? view.reason
  const run = async (operation: () => Promise<void>) => {
    if (locked.current) return
    locked.current = true; setBusy(true); setFailure('')
    try { await operation() } catch (error) { if (mounted.current) setFailure(errorText(error)) }
    finally { locked.current = false; if (mounted.current) setBusy(false) }
  }
  const close = () => {
    if (locked.current && phase !== 'waiting') return
    if (phase === 'models' && (JSON.stringify({ name, mapping, effortMapping }) !== baseline.current || contextMode !== initialContextMode)) { setConfirm('close'); return }
    stopPolling(); onClose()
  }
  const apply = (view: SubscriptionLoginView, token: number) => {
    if (!mounted.current || token !== generation.current) return
    attempt.current = view.attemptId; loginStatus.current = view.status
    // A failed browser launch keeps the OAuth attempt alive: stay on the
    // waiting page, show why, and let the same status chain keep polling.
    const recoverable = view.status === 'failed' && view.canReopen === true
    setCanReopen(view.status === 'waiting' || recoverable)
    if (view.status === 'ready' && view.details) {
      const nextName = existing?.name ?? (provider === 'codex' ? 'ChatGPT' : 'Antigravity')
      const nextMapping = subscriptionDefaults(provider, view.details.models, existing?.modelMapping)
      const usableDetails = { ...view.details, models: usableSubscriptionModels(view.details.models) }; setDetails(usableDetails); onDetails(usableDetails); setName(nextName); setMapping(nextMapping)
      baseline.current = JSON.stringify({ name: nextName, mapping: nextMapping, effortMapping }); setPhase('models'); setFailure(''); return
    }
    if (view.status === 'waiting' || view.status === 'exchanging' || recoverable) {
      setPhase('waiting')
      if (recoverable) setFailure(viewReason(view) ?? t('浏览器没有打开', "The browser didn't open"))
      clearTimeout(timer.current)
      timer.current = setTimeout(() => { void poll(view.attemptId, token) }, 800); return
    }
    setPhase('error'); setFailure(viewReason(view) ?? t('登录未完成，请重试。', 'Sign-in did not finish. Try again.'))
  }
  const poll = async (id: string, token: number) => {
    if (!client || !mounted.current || token !== generation.current) return
    try { apply(await client.request('provider-config.subscription.login-status', { attemptId: id }) as SubscriptionLoginView, token) }
    catch (error) { if (mounted.current && token === generation.current) { loginStatus.current = 'failed'; setCanReopen(false); setPhase('error'); setFailure(errorText(error)) } }
  }
  const login = () => { void run(async () => {
    if (!client) {
      stopPolling(); setCanReopen(false); loginStatus.current = undefined; setPhase('error')
      setFailure(t('服务正在连接，请稍后重试。', 'Service is connecting. Try again shortly.'))
      return
    }
    stopPolling(); const token = generation.current; setCanReopen(false); loginStatus.current = undefined; setPhase('waiting')
    try {
      if (attempt.current) {
        const previous = attempt.current
        await cancel(previous)
        if (attempt.current === previous) attempt.current = undefined
      }
      const view = await client.request('provider-config.subscription.login-start', { provider, locale }, { timeoutMs: LOGIN_START_TIMEOUT_MS }) as SubscriptionLoginView
      if (!mounted.current || token !== generation.current) { void cancel(view.attemptId).catch(() => {}); return }
      apply(view, token)
    } catch (error) {
      if (mounted.current && token === generation.current) { setPhase('error'); setFailure(errorText(error)) }
    }
  }) }
  const reopen = async () => {
    const id = attempt.current, token = generation.current
    if (!client || !id || !canReopenLogin() || reopenWorking.current) return
    reopenWorking.current = true; setReopening(true); setFailure('')
    try {
      const view = await client.request('provider-config.subscription.login-reopen', { attemptId: id }) as SubscriptionLoginView
      if (!mounted.current || token !== generation.current || !canReopenLogin()) return
      // The existing status reader stays in charge; reopening must never start
      // a second polling chain for the same attempt.
      if (view.status === 'waiting' || view.status === 'exchanging') {
        loginStatus.current = view.status; setCanReopen(view.status === 'waiting'); setFailure('')
      } else if (view.status === 'failed' && view.canReopen === true) {
        loginStatus.current = 'failed'; setCanReopen(true)
        setFailure(viewReason(view) ?? t('浏览器没有打开', "The browser didn't open"))
      } else { stopPolling(); apply(view, generation.current) }
    } catch (error) {
      if (mounted.current && token === generation.current && canReopenLogin()) setFailure(errorText(error))
    } finally { if (token === generation.current) { reopenWorking.current = false; if (mounted.current) setReopening(false) } }
  }
  const save = (use: boolean) => { void run(async () => {
    if (!name.trim()) throw new Error(t('请填写显示名称。', 'Enter a display name.'))
    const usable = usableSubscriptionModels(details?.models ?? [])
    if (!usable.length || Object.values(mapping).some(id => !usable.some(model => model.id === id))) throw new Error(t('请选择可用模型。', 'Choose available models.'))
    const invalidEffort = ROLES.find(role => {
      const model = usable.find(row => row.id === mapping[role])
      return effortMapping[role] !== 'follow' && !supportedEfforts(provider, model).includes(effortMapping[role] as Exclude<ReasoningEffort, 'follow'>)
    })
    if (invalidEffort) throw new Error(t(
      `${roleLabel(invalidEffort)} 的推理强度不受当前模型支持，请重新选择。`,
      `The reasoning effort for ${roleLabel(invalidEffort)} is not supported by its model. Choose another effort.`
    ))
    submitted.current = true
    try { await onSave({ kind: 'subscription', ...(existing ? { id: existing.id } : {}), cli: 'claude-code', name: name.trim(), subscription: { provider, ...(attempt.current ? { attemptId: attempt.current } : {}) }, modelMapping: mapping, effortMapping }, use) }
    catch (error) { submitted.current = false; throw error }
  }) }
  // Retry only rereads the model list for the current draft or saved account.
  // It never starts OAuth, cancels the draft or saves the card.
  const retryModels = () => { void run(async () => {
    if (!client) throw new Error(t('服务正在连接，请稍后重试。', 'Service is connecting. Try again shortly.'))
    const payload = attempt.current ? { attemptId: attempt.current } : existing?.id ? { providerId: existing.id } : undefined
    if (!payload) throw new Error(t('没有读到可用的模型', 'No available models were found'))
    const next = await client.request('provider-config.subscription.models-read', payload) as SubscriptionAccountDetails
    if (!mounted.current) return
    const usableDetails = { ...next, models: usableSubscriptionModels(next.models ?? []) }
    const nextMapping = subscriptionDefaults(provider, usableDetails.models, mapping)
    // Auto-derived mappings are the new baseline; a name the user typed stays unsaved.
    const previous = JSON.parse(baseline.current) as { name: string; mapping: Mapping; effortMapping: EffortMapping }
    baseline.current = JSON.stringify({ name: previous.name, mapping: nextMapping, effortMapping: previous.effortMapping })
    setDetails(usableDetails); onDetails(usableDetails); setMapping(nextMapping)
  }) }
  return <SubscriptionSetupView provider={provider} phase={phase} existing={existing} details={details} name={name} mapping={mapping} effortMapping={effortMapping} contextWindowMode={contextMode}
    riskChecked={riskChecked} canReopen={canReopen} reopening={reopening} busy={busy} connected={Boolean(client)} failure={failure} confirm={confirm} canDelete={Boolean(existing && existing.id !== activeId)}
    onClose={close} onRiskChecked={setRiskChecked} onContinue={() => { if (riskChecked) void run(async () => { if (!client) return; await client.request('provider-config.subscription.risk-confirm', { accepted: true }); onRiskAccepted(); setPhase('login') }) }}
    onLogin={login} onReopen={() => { void reopen() }} onCancel={() => { if (locked.current) { stopPolling(); setPhase('error'); setFailure(t('登录已取消。', 'Sign-in was cancelled.')); return }; void run(async () => { stopPolling(); const view = attempt.current ? await cancel(attempt.current) : undefined; setPhase('error'); setFailure((view ? viewReason(view) : undefined) ?? t('登录已取消。', 'Sign-in was cancelled.')) }) }}
    onName={setName} onMapping={(role, value) => setMapping(previous => ({ ...previous, [role]: value }))} onEffort={(role, value) => setEffortMapping(previous => ({ ...previous, [role]: value }))} onContextWindowChange={mode => { setContextMode(mode); onContextWindowChange?.(mode) }} onSave={save}
    onDelete={() => setConfirm('delete')} onKeepEditing={() => setConfirm(undefined)} onConfirm={() => { if (confirm === 'delete' && existing) void run(() => onDelete(existing)); else { stopPolling(); onClose() } }} onRetryModels={retryModels} />
}

function roleLabel(role: Role) {
  return role[0]!.toUpperCase() + role.slice(1)
}

function supportedEfforts(provider: SubscriptionProvider, model: SubscriptionAccountDetails['models'][number] | undefined): Exclude<ReasoningEffort, 'follow'>[] {
  if (model?.supportedEfforts !== undefined) return model.supportedEfforts
  const label = `${provider} ${model?.id ?? ''} ${model?.displayName ?? ''}`
  return !(provider === 'codex' || /\b(gpt|codex|o[1-9])\b/i.test(label)) && /claude|opus|sonnet|haiku/i.test(label)
    ? ['low', 'medium', 'high', 'xhigh', 'max']
    : ['low', 'medium', 'high', 'xhigh']
}
