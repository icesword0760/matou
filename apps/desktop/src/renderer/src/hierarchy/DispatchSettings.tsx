import { useEffect, useRef, useState } from 'react'

import {
  DISPATCH_DEFAULTS,
  type DispatchConfigSnapshot,
  type DispatchDefaults
} from '@matou/contracts'

import { useMessages } from '../i18n/LocaleProvider'
import type { ProviderConfigClient } from './ModelSwitchSettings'
import './dispatch-settings.css'

const CODEX_MODELS = ['sol', 'best', 'astra', 'terra', 'luna'] as const
const ANTIGRAVITY_MODELS = ['gemini-pro-orchestrator', 'hydrogen', 'argon', 'carbon'] as const
const ORCHESTRATOR_MODEL = 'gemini-pro-orchestrator'
const DSH_MODEL = 'deepseek-flash'
const EFFORTS = ['low', 'medium', 'high', 'xhigh'] as const
const DSH_EFFORTS = ['low', 'high'] as const
const AGENTS = ['codex', 'agy', 'dsh'] as const
const CONFIG_PATH = '~/.claude/agent-foreman/config.json'

type DispatchAgent = DispatchDefaults['agent']
type DispatchEffort = DispatchDefaults['effort']
type NumericField = 'idleMin' | 'maxMin' | 'askWaitMin' | 'retries'
type RowKey = 'agent' | 'model' | 'effort' | NumericField | 'defaults'
type NumberDraft = Record<NumericField, string>
type DispatchMessages = ReturnType<typeof useMessages>['hierarchyShell']['dispatchSettings']

const NUMBER_LIMITS: Record<NumericField, { min: number; max: number }> = {
  idleMin: { min: 1, max: 60 },
  maxMin: { min: 5, max: 180 },
  askWaitMin: { min: 1, max: 60 },
  retries: { min: 0, max: 5 }
}

export function DispatchSettings({ client }: { client: ProviderConfigClient | null }) {
  const m = useMessages().hierarchyShell.dispatchSettings
  const [snapshot, setSnapshot] = useState<DispatchConfigSnapshot>()
  const [draft, setDraft] = useState<DispatchDefaults>()
  const [numbers, setNumbers] = useState<NumberDraft>(() => toNumberDraft(DISPATCH_DEFAULTS))
  const [saving, setSaving] = useState(false)
  const [loadError, setLoadError] = useState('')
  const [rowErrors, setRowErrors] = useState<Partial<Record<RowKey, string>>>({})
  const [saved, setSaved] = useState(false)
  const [retry, setRetry] = useState(0)
  const snapshotRef = useRef<DispatchConfigSnapshot | undefined>(undefined)
  const savingRef = useRef(false)
  const aliveRef = useRef(true)

  useEffect(() => {
    aliveRef.current = true
    return () => { aliveRef.current = false }
  }, [])

  useEffect(() => {
    let active = true
    setLoadError('')
    setRowErrors({})
    setSaved(false)
    if (!client) {
      snapshotRef.current = undefined
      setSnapshot(undefined)
      setDraft(undefined)
      setLoadError(m.serviceDisconnected)
      return
    }
    snapshotRef.current = undefined
    setSnapshot(undefined)
    setDraft(undefined)
    void client.request('dispatch-config.snapshot', {}).then(result => {
      if (!active) return
      if (!isDispatchSnapshot(result)) {
        setLoadError(m.loadFailed)
        return
      }
      applySnapshot(result)
    }).catch(() => {
      if (!active) return
      setLoadError(m.loadFailed)
    })
    return () => { active = false }
  }, [client, retry, m])

  const applySnapshot = (next: DispatchConfigSnapshot) => {
    snapshotRef.current = next
    setSnapshot(next)
    setDraft({ ...next.defaults })
    setNumbers(toNumberDraft(next.defaults))
  }

  const persist = async (next: DispatchDefaults, row: RowKey) => {
    if (!client || !snapshotRef.current || savingRef.current) return false
    savingRef.current = true
    setSaving(true)
    setRowErrors({})
    setSaved(false)
    try {
      const result = await client.request('dispatch-config.save', { defaults: { ...next } })
      if (!aliveRef.current) return false
      if (!isDispatchSnapshot(result)) throw new Error(m.invalidSaveResponse)
      applySnapshot(result)
      setSaved(true)
      return true
    } catch (error) {
      if (!aliveRef.current) return false
      const base = snapshotRef.current
      if (base) {
        setDraft({ ...base.defaults })
        setNumbers(toNumberDraft(base.defaults))
      }
      setRowErrors({ [row]: errorText(m.saveFailed, m.saveFailedDetail, error) })
      return false
    } finally {
      if (aliveRef.current) setSaving(false)
      savingRef.current = false
    }
  }

  const chooseAgent = (agent: DispatchAgent) => {
    if (!draft || savingRef.current || agent === draft.agent) return
    // Keep the saved model untouched: an executor switch only saves the
    // executor, and the model list picks up the saved value again later.
    const next: DispatchDefaults = { ...draft, agent }
    setDraft(next)
    setSaved(false)
    setRowErrors({})
    void persist(next, 'agent')
  }

  const chooseModel = (model: string) => {
    if (!draft || savingRef.current || model === draft.model) return
    const next: DispatchDefaults = { ...draft, model }
    setDraft(next)
    setSaved(false)
    setRowErrors({})
    void persist(next, 'model')
  }

  const chooseEffort = (effort: DispatchEffort) => {
    if (!draft || savingRef.current || effort === draft.effort) return
    const next: DispatchDefaults = { ...draft, effort }
    setDraft(next)
    setSaved(false)
    setRowErrors({})
    void persist(next, 'effort')
  }

  const changeNumber = (field: NumericField, value: string) => {
    setNumbers(current => ({ ...current, [field]: value }))
    setSaved(false)
    setRowErrors(current => {
      if (!current[field]) return current
      const next = { ...current }
      delete next[field]
      return next
    })
  }

  const commitNumber = (field: NumericField) => {
    if (!draft || savingRef.current) return
    const raw = numbers[field].trim()
    const limits = NUMBER_LIMITS[field]
    const integerField = field === 'retries'
    if (!/^\d+(?:\.\d+)?$/u.test(raw)) {
      setRowErrors(current => ({ ...current, [field]: integerField ? m.invalidInteger : m.invalidNumber }))
      return
    }
    const value = Number(raw)
    if (draft[field] === value) {
      // A legal value already saved (even one outside the UI range) is not an
      // edit: normalize the text and leave the file alone.
      setNumbers(current => ({ ...current, [field]: String(value) }))
      setRowErrors(current => {
        if (!current[field]) return current
        const next = { ...current }
        delete next[field]
        return next
      })
      return
    }
    if (integerField && !Number.isInteger(value)) {
      setRowErrors(current => ({ ...current, [field]: m.invalidInteger }))
      return
    }
    if (value < limits.min || value > limits.max) {
      setRowErrors(current => ({
        ...current,
        [field]: integerField ? m.invalidIntegerRange(limits.min, limits.max) : m.invalidRange(limits.min, limits.max)
      }))
      return
    }
    const next: DispatchDefaults = { ...draft, [field]: value }
    setDraft(next)
    setNumbers(current => ({ ...current, [field]: String(value) }))
    setSaved(false)
    setRowErrors({})
    void persist(next, field)
  }

  const restoreDefaults = () => {
    if (!draft || savingRef.current) return
    const next: DispatchDefaults = { ...DISPATCH_DEFAULTS }
    setDraft(next)
    setNumbers(toNumberDraft(next))
    setSaved(false)
    setRowErrors({})
    void persist(next, 'defaults')
  }

  const busy = (!snapshot && !loadError) || saving
  const dsh = draft?.agent === 'dsh'
  const modelOptions = draft?.agent === 'agy' ? ANTIGRAVITY_MODELS : CODEX_MODELS
  const shownModel = draft ? modelForAgent(draft.agent, draft.model) : undefined
  const orchestrator = Boolean(draft && draft.agent === 'agy' && shownModel === ORCHESTRATOR_MODEL)
  const unknownModel = Boolean(draft && !dsh && !(modelOptions as readonly string[]).includes(draft.model))
  const dshEffortClamped = Boolean(dsh && (draft?.effort === 'medium' || draft?.effort === 'xhigh'))
  const shownEffort = dsh ? clampDshEffort(draft?.effort) : draft?.effort
  const effortOptions = dsh ? DSH_EFFORTS : EFFORTS
  const effortText = (value: DispatchEffort | undefined) => value ? effortLabel(value, m) : ''

  return <section className="dispatch-settings" aria-label={m.region} aria-busy={busy}>
    {!snapshot && !loadError && <p>{m.loading}</p>}
    {loadError && <div className="dispatch-settings__load-error" role="alert">
      {loadError}{client && <button type="button" onClick={() => setRetry(value => value + 1)}>{m.retry}</button>}
    </div>}
    {snapshot && draft && <section className="dispatch-settings__section" aria-labelledby="dispatch-settings-heading">
      <header className="dispatch-settings__header">
        <div>
          <h2 id="dispatch-settings-heading">{m.title}</h2>
          <p>{m.heading}</p>
        </div>
      </header>

      <div className="dispatch-settings__card">
        <div className="dispatch-settings__row">
          <label className="dispatch-settings__row-label" htmlFor="dispatch-agent">{m.agent}</label>
          <div className="dispatch-settings__row-body">
            <select id="dispatch-agent" aria-label={m.agent} value={draft.agent} disabled={saving}
              aria-describedby={rowErrors.agent ? 'dispatch-agent-error' : undefined}
              onChange={event => chooseAgent(event.target.value as DispatchAgent)}>
              <option value="codex">{m.agentCodex}</option>
              <option value="agy">{m.agentAntigravity}</option>
              <option value="dsh">{m.agentDsh}</option>
            </select>
          </div>
          {rowErrors.agent && <p className="dispatch-settings__row-error" id="dispatch-agent-error" role="alert">{rowErrors.agent}</p>}
        </div>

        <div className="dispatch-settings__row">
          <label className="dispatch-settings__row-label" htmlFor="dispatch-model">{m.model}</label>
          <div className="dispatch-settings__row-body">
            {dsh
              ? <input id="dispatch-model" className="dispatch-settings__readonly" aria-label={m.model} value={DSH_MODEL} readOnly aria-describedby="dispatch-model-hint" />
              : <select id="dispatch-model" aria-label={m.model} value={shownModel} disabled={saving}
                aria-describedby={modelHintId(unknownModel, rowErrors.model)}
                onChange={event => chooseModel(event.target.value)}>
                {modelOptions.map(model => <option key={model} value={model}>{modelLabel(model, m)}</option>)}
              </select>}
            {dsh && <p className="dispatch-settings__hint" id="dispatch-model-hint">{m.modelDshNote}</p>}
            {!dsh && unknownModel && <p className="dispatch-settings__hint" id="dispatch-model-hint">{m.modelUnknownNote}</p>}
          </div>
          {rowErrors.model && <p className="dispatch-settings__row-error" id="dispatch-model-error" role="alert">{rowErrors.model}</p>}
        </div>

        <div className="dispatch-settings__row">
          <label className="dispatch-settings__row-label" htmlFor="dispatch-effort">{m.effort}</label>
          <div className="dispatch-settings__row-body">
            <select id="dispatch-effort" aria-label={m.effort} value={shownEffort} disabled={saving || orchestrator}
              aria-describedby={effortHintId(dsh, dshEffortClamped, orchestrator, rowErrors.effort)}
              onChange={event => chooseEffort(event.target.value as DispatchEffort)}>
              {effortOptions.map(effort => <option key={effort} value={effort}>{effortLabel(effort, m)}</option>)}
            </select>
            {dsh && <p className="dispatch-settings__hint" id="dispatch-effort-hint">{m.effortDshNote}</p>}
            {dsh && dshEffortClamped && <p className="dispatch-settings__hint" id="dispatch-effort-clamp">{m.effortDshClamped(effortText(draft.effort))}</p>}
            {orchestrator && <p className="dispatch-settings__hint" id="dispatch-effort-hint">{m.effortOrchestratorNote}</p>}
          </div>
          {rowErrors.effort && <p className="dispatch-settings__row-error" id="dispatch-effort-error" role="alert">{rowErrors.effort}</p>}
        </div>

        {numberFieldRows(m, numbers, rowErrors, saving, changeNumber, commitNumber)}
        <div className="dispatch-settings__actions">
          <button type="button" disabled={saving} onClick={restoreDefaults}>{m.restoreDefaults}</button>
          {(saving || saved) && <span className="dispatch-settings__saved" role="status">{saving ? m.saving : m.saved}</span>}
          {rowErrors.defaults && <p className="dispatch-settings__row-error" role="alert">{rowErrors.defaults}</p>}
        </div>
      </div>

      <p className="dispatch-settings__scope">{m.scopeNote}</p>
      <p className="dispatch-settings__path">
        <span>{m.pathLabel}</span>
        <code title={snapshot.path || CONFIG_PATH}>{shortConfigPath(snapshot.path)}</code>
        {!snapshot.exists && <span>· {m.fileMissing}</span>}
      </p>
    </section>}
  </section>
}

function numberFieldRows(
  m: DispatchMessages,
  numbers: NumberDraft,
  rowErrors: Partial<Record<RowKey, string>>,
  saving: boolean,
  changeNumber: (field: NumericField, value: string) => void,
  commitNumber: (field: NumericField) => void
) {
  const fields: { key: NumericField; label: string; unit: string }[] = [
    { key: 'idleMin', label: m.idleMin, unit: m.minutes },
    { key: 'maxMin', label: m.maxMin, unit: m.minutes },
    { key: 'askWaitMin', label: m.askWaitMin, unit: m.minutes },
    { key: 'retries', label: m.retries, unit: '' }
  ]
  return fields.map(field => <div className="dispatch-settings__row" key={field.key}>
    <label className="dispatch-settings__row-label" htmlFor={`dispatch-${field.key}`}>{field.label}</label>
    <div className="dispatch-settings__row-body">
      <div className="dispatch-settings__number">
        <input id={`dispatch-${field.key}`} type="number" inputMode={field.key === 'retries' ? 'numeric' : 'decimal'}
          step={field.key === 'retries' ? 1 : 'any'}
          min={NUMBER_LIMITS[field.key].min} max={NUMBER_LIMITS[field.key].max}
          aria-label={field.label} value={numbers[field.key]} disabled={saving}
          aria-invalid={Boolean(rowErrors[field.key])}
          aria-describedby={rowErrors[field.key] ? `dispatch-${field.key}-error` : undefined}
          onChange={event => changeNumber(field.key, event.target.value)}
          onBlur={() => commitNumber(field.key)}
          onKeyDown={event => {
            if (event.key !== 'Enter') return
            event.preventDefault()
            commitNumber(field.key)
          }} />
        {field.unit && <span className="dispatch-settings__unit">{field.unit}</span>}
      </div>
    </div>
    {rowErrors[field.key] && <p className="dispatch-settings__row-error" id={`dispatch-${field.key}-error`} role="alert">{rowErrors[field.key]}</p>}
  </div>)
}

function modelHintId(unknownModel: boolean, error?: string) {
  return [unknownModel ? 'dispatch-model-hint' : '', error ? 'dispatch-model-error' : ''].filter(Boolean).join(' ') || undefined
}

function effortHintId(dsh: boolean, clamped: boolean, orchestrator: boolean, error?: string) {
  return [
    dsh || orchestrator ? 'dispatch-effort-hint' : '',
    clamped ? 'dispatch-effort-clamp' : '',
    error ? 'dispatch-effort-error' : ''
  ].filter(Boolean).join(' ') || undefined
}

function isDispatchSnapshot(value: unknown): value is DispatchConfigSnapshot {
  if (!value || typeof value !== 'object') return false
  const candidate = value as Partial<DispatchConfigSnapshot>
  const defaults = candidate.defaults as Partial<DispatchDefaults> | undefined
  if (typeof candidate.exists !== 'boolean' || typeof candidate.path !== 'string' || !defaults) return false
  if (!AGENTS.includes(defaults.agent as DispatchAgent)) return false
  if (typeof defaults.model !== 'string' || defaults.model.length === 0) return false
  if (!EFFORTS.includes(defaults.effort as DispatchEffort)) return false
  return (['idleMin', 'maxMin', 'askWaitMin', 'retries'] as const)
    .every(key => typeof defaults[key] === 'number' && Number.isFinite(defaults[key]))
}

function toNumberDraft(defaults: DispatchDefaults): NumberDraft {
  return {
    idleMin: String(defaults.idleMin),
    maxMin: String(defaults.maxMin),
    askWaitMin: String(defaults.askWaitMin),
    retries: String(defaults.retries)
  }
}

function modelForAgent(agent: DispatchAgent, current: string): string {
  if (agent === 'dsh') return current
  const options: readonly string[] = agent === 'agy' ? ANTIGRAVITY_MODELS : CODEX_MODELS
  if (options.includes(current)) return current
  return agent === 'agy' ? ORCHESTRATOR_MODEL : 'sol'
}

function clampDshEffort(effort?: DispatchEffort): DispatchEffort | undefined {
  if (effort === 'medium' || effort === 'xhigh') return 'high'
  return effort
}

function modelLabel(model: string, m: DispatchMessages) {
  if (model === ORCHESTRATOR_MODEL) return m.modelOrchestrator
  return model
}

function effortLabel(effort: DispatchEffort, m: DispatchMessages) {
  if (effort === 'low') return m.effortLow
  if (effort === 'medium') return m.effortMedium
  if (effort === 'high') return m.effortHigh
  return m.effortXHigh
}

function shortConfigPath(path: string): string {
  const normalized = path.replace(/\\/gu, '/')
  const marker = '/.claude/agent-foreman/config.json'
  const index = normalized.lastIndexOf(marker)
  if (index < 0) return path || CONFIG_PATH
  return `~${normalized.slice(index)}`
}

function errorText(
  fallback: string,
  withDetail: (detail: string) => string,
  error: unknown
) {
  return error instanceof Error && error.message ? withDetail(error.message) : fallback
}
