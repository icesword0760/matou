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
type ChoiceDraft = Pick<DispatchDefaults, 'agent' | 'model' | 'effort'>
type NumberErrors = Partial<Record<NumericField, string>>
type NumberDraft = Record<NumericField, string>
type DispatchMessages = ReturnType<typeof useMessages>['hierarchyShell']['dispatchSettings']

const NUMBER_LIMITS: Record<NumericField, { min: number; max: number }> = {
  idleMin: { min: 1, max: 60 },
  maxMin: { min: 5, max: 180 },
  askWaitMin: { min: 1, max: 60 },
  retries: { min: 0, max: 5 }
}
const NUMBER_FIELDS = Object.keys(NUMBER_LIMITS) as NumericField[]

export function DispatchSettings({ client }: { client: ProviderConfigClient | null }) {
  const m = useMessages().hierarchyShell.dispatchSettings
  const [snapshot, setSnapshot] = useState<DispatchConfigSnapshot>()
  const [draft, setDraft] = useState<ChoiceDraft>()
  const [numbers, setNumbers] = useState<NumberDraft>(() => toNumberDraft(DISPATCH_DEFAULTS))
  const [saving, setSaving] = useState(false)
  const [loadError, setLoadError] = useState('')
  const [rowErrors, setRowErrors] = useState<NumberErrors>({})
  const [saveError, setSaveError] = useState('')
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
    setSaveError('')
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
    const { agent, model, effort } = next.defaults
    setDraft({ agent, model, effort })
    setNumbers(toNumberDraft(next.defaults))
  }

  const savedDefaults = snapshot?.defaults
  const numberValues = savedDefaults && NUMBER_FIELDS.map(field => (
    readNumber(field, numbers[field], savedDefaults[field], m)
  ))
  const dirty = Boolean(savedDefaults && draft && numberValues && (
    draft.agent !== savedDefaults.agent ||
    draft.model !== savedDefaults.model ||
    draft.effort !== savedDefaults.effort ||
    NUMBER_FIELDS.some((field, index) => numberValues[index] !== savedDefaults[field])
  ))
  const canSave = dirty && !saving && Boolean(numberValues?.every(value => typeof value === 'number'))

  const saveDraft = async (button: HTMLButtonElement) => {
    const base = snapshotRef.current
    if (!client || !base || !draft || !dirty || savingRef.current) return
    const next: DispatchDefaults = { ...base.defaults, ...draft }
    const errors: NumberErrors = {}
    for (const field of NUMBER_FIELDS) {
      const value = readNumber(field, numbers[field], base.defaults[field], m)
      if (typeof value === 'number') next[field] = value
      else errors[field] = value
    }
    const firstError = NUMBER_FIELDS.find(field => errors[field])
    if (firstError) {
      setRowErrors(errors)
      button.closest('.dispatch-settings')?.querySelector<HTMLInputElement>(`#dispatch-${firstError}`)?.focus()
      return
    }

    // Keep the focused button mounted and focusable while the write is pending.
    button.focus({ preventScroll: true })
    savingRef.current = true
    setSaving(true)
    setSaveError('')
    setSaved(false)
    try {
      const result = await client.request('dispatch-config.save', { defaults: next })
      if (!aliveRef.current || snapshotRef.current !== base) return
      if (!isDispatchSnapshot(result)) throw new Error(m.invalidSaveResponse)
      applySnapshot(result)
      setSaved(true)
    } catch (error) {
      if (aliveRef.current && snapshotRef.current === base) {
        setSaveError(errorText(m.saveFailed, m.saveFailedDetail, error))
      }
    } finally {
      if (aliveRef.current) setSaving(false)
      savingRef.current = false
    }
  }

  const markEdited = () => {
    setSaved(false)
    setSaveError('')
  }

  const changeChoice = (patch: Partial<ChoiceDraft>) => {
    if (savingRef.current) return
    setDraft(current => current && { ...current, ...patch })
    markEdited()
  }

  const chooseAgent = (agent: DispatchAgent) => {
    if (!draft || !savedDefaults || agent === draft.agent) return
    // The picker may display a fallback for the new executor. Keep the stored model.
    changeChoice({ agent, model: savedDefaults.model })
  }

  const changeNumber = (field: NumericField, value: string) => {
    if (savingRef.current) return
    setNumbers(current => ({ ...current, [field]: value }))
    setRowErrors(current => ({ ...current, [field]: '' }))
    markEdited()
  }

  const commitNumber = (field: NumericField) => {
    if (!savedDefaults || savingRef.current) return
    const value = readNumber(field, numbers[field], savedDefaults[field], m)
    setRowErrors(current => ({ ...current, [field]: typeof value === 'string' ? value : '' }))
  }

  const restoreDefaults = () => {
    if (!draft || savingRef.current) return
    const { agent, model, effort } = DISPATCH_DEFAULTS
    setDraft({ agent, model, effort })
    setNumbers(toNumberDraft(DISPATCH_DEFAULTS))
    setRowErrors({})
    markEdited()
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
              onChange={event => chooseAgent(event.target.value as DispatchAgent)}>
              <option value="codex">{m.agentCodex}</option>
              <option value="agy">{m.agentAntigravity}</option>
              <option value="dsh">{m.agentDsh}</option>
            </select>
          </div>
        </div>

        <div className="dispatch-settings__row">
          <label className="dispatch-settings__row-label" htmlFor="dispatch-model">{m.model}</label>
          <div className="dispatch-settings__row-body">
            {dsh
              ? <input id="dispatch-model" className="dispatch-settings__readonly" aria-label={m.model} value={DSH_MODEL} readOnly disabled={saving} aria-describedby="dispatch-model-hint" />
              : <select id="dispatch-model" aria-label={m.model} value={shownModel} disabled={saving}
                aria-describedby={unknownModel ? 'dispatch-model-hint' : undefined}
                onChange={event => changeChoice({ model: event.target.value })}>
                {modelOptions.map(model => <option key={model} value={model}>{modelLabel(model, m)}</option>)}
              </select>}
            {dsh && <p className="dispatch-settings__hint" id="dispatch-model-hint">{m.modelDshNote}</p>}
            {!dsh && unknownModel && <p className="dispatch-settings__hint" id="dispatch-model-hint">{m.modelUnknownNote}</p>}
          </div>
        </div>

        <div className="dispatch-settings__row">
          <label className="dispatch-settings__row-label" htmlFor="dispatch-effort">{m.effort}</label>
          <div className="dispatch-settings__row-body">
            <select id="dispatch-effort" aria-label={m.effort} value={shownEffort} disabled={saving || orchestrator}
              aria-describedby={effortHintId(dsh, dshEffortClamped, orchestrator)}
              onChange={event => changeChoice({ effort: event.target.value as DispatchEffort })}>
              {effortOptions.map(effort => <option key={effort} value={effort}>{effortLabel(effort, m)}</option>)}
            </select>
            {dsh && <p className="dispatch-settings__hint" id="dispatch-effort-hint">{m.effortDshNote}</p>}
            {dsh && dshEffortClamped && <p className="dispatch-settings__hint" id="dispatch-effort-clamp">{m.effortDshClamped(effortText(draft.effort))}</p>}
            {orchestrator && <p className="dispatch-settings__hint" id="dispatch-effort-hint">{m.effortOrchestratorNote}</p>}
          </div>
        </div>

        {numberFieldRows(m, numbers, rowErrors, saving, changeNumber, commitNumber)}
        <div className="dispatch-settings__actions">
          <button type="button" disabled={saving} onClick={restoreDefaults}>{m.restoreDefaults}</button>
          <button type="button" className="primary" disabled={!dirty && !saving && !saved}
            aria-disabled={!canSave} onClick={event => void saveDraft(event.currentTarget)}>
            {saving ? m.saving : m.save}
          </button>
          {saved && <span className="dispatch-settings__saved" role="status">{m.saved}</span>}
          {saveError && <p className="dispatch-settings__row-error" role="alert">{saveError}</p>}
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
  rowErrors: NumberErrors,
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
        <input id={`dispatch-${field.key}`} type="text" inputMode={field.key === 'retries' ? 'numeric' : 'decimal'}
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

function effortHintId(dsh: boolean, clamped: boolean, orchestrator: boolean) {
  if (clamped) return 'dispatch-effort-hint dispatch-effort-clamp'
  return dsh || orchestrator ? 'dispatch-effort-hint' : undefined
}

function readNumber(field: NumericField, text: string, saved: number, m: DispatchMessages): number | string {
  const raw = text.trim()
  const integerField = field === 'retries'
  // Existing values can be outside the editor's limits or use scientific notation.
  if (raw === String(saved)) return saved
  if (!/^\d+(?:\.\d+)?$/u.test(raw)) return integerField ? m.invalidInteger : m.invalidNumber

  const value = Number(raw)
  if (value === saved) return saved
  if (integerField && !Number.isInteger(value)) return m.invalidInteger
  const { min, max } = NUMBER_LIMITS[field]
  if (value < min || value > max) {
    return integerField ? m.invalidIntegerRange(min, max) : m.invalidRange(min, max)
  }
  return value
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
