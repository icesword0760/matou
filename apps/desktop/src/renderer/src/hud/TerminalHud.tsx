import { useEffect, useRef, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'

import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { GitControlMenu, type GitControlContext, type GitRequestClient } from './GitControlMenu'
import {
  EnvironmentControlMenu,
  environmentLabel,
  type SessionEnvironmentActions
} from './EnvironmentControlMenu'
import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'
import type { HudPermissionMode, SessionHudView } from '../hierarchy/hierarchy-types'
import type { SessionEnvironment, SessionGitState } from '@matou/domain'

type HudMessages = Messages['hud']

const PERMISSION_MODES: Array<{ value: HudPermissionMode; abbreviation: string }> = [
  { value: 'default', abbreviation: 'D' },
  { value: 'auto', abbreviation: 'A' },
  { value: 'acceptEdits', abbreviation: 'AE' },
  { value: 'plan', abbreviation: 'PM' },
  { value: 'bypassPermissions', abbreviation: 'BP' }
]
export function TerminalHud(props: {
  hud: SessionHudView | undefined
  sessionId?: string
  onPermissionMode?(sessionId: string, mode: HudPermissionMode, respawn: boolean): unknown
  gitContext?: GitControlContext
  runtimeClient?: GitRequestClient
  disabledReason?: string
  environmentDisabledReason?: string
  environment?: SessionEnvironment
  git?: SessionGitState
  hasOwnedWorktree?: boolean
  environmentActions?: SessionEnvironmentActions
}) {
  const { hud } = props
  const m = useMessages().hud
  const contextClient = useRuntimeClient()
  const gitClient = props.runtimeClient ?? contextClient
  const disabled = Boolean(props.disabledReason)
  const [permissionMode, setPermissionMode] = useState<HudPermissionMode>(hud?.permissionMode ?? 'default')
  const [menu, setMenu] = useState<'permission' | null>(null)
  const [menuStyle, setMenuStyle] = useState<CSSProperties>({})
  const [confirmTarget, setConfirmTarget] = useState<HudPermissionMode | null>(null)
  const [switching, setSwitching] = useState(false)
  const [switchError, setSwitchError] = useState('')
  const [gitOpen, setGitOpen] = useState(false)
  const [environmentOpen, setEnvironmentOpen] = useState(false)
  const [instructionsOpen, setInstructionsOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => setPermissionMode(hud?.permissionMode ?? 'default'), [hud?.sessionId, hud?.permissionMode])
  useEffect(() => {
    if (!disabled) return
    setMenu(null)
    setConfirmTarget(null)
    setGitOpen(false)
    setInstructionsOpen(false)
  }, [disabled])
  useEffect(() => {
    const closeOutside = (event: Event) => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (!rootRef.current?.contains(target) && !(target instanceof Element && target.closest('.perm-menu'))) {
        setMenu(null)
      }
    }
    const closeEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || (!menu && !environmentOpen)) return
      setMenu(null); setEnvironmentOpen(false)
      event.preventDefault()
      event.stopPropagation()
    }
    document.addEventListener('pointerdown', closeOutside, true)
    document.addEventListener('keydown', closeEscape, true)
    return () => {
      document.removeEventListener('pointerdown', closeOutside, true)
      document.removeEventListener('keydown', closeEscape, true)
    }
  }, [menu, environmentOpen])

  const sessionId = hud?.sessionId ?? props.sessionId
  if (!sessionId || (!hud && !props.environment && !props.git)) return null
  const shortCwd = cwdShortName(hud?.cwd)
  const git = hud
    ? legacyGitState(hud)
    : props.git ?? (props.environment ? { state: 'unavailable' as const, dirty: false } : undefined)
  const gitDisplay = git ? gitStateLabel(git, m) : ''
  const gitCwd = props.environment?.state === 'ready' ? props.environment.path : hud?.cwd
  const permissionTitle = m.permissionSwitch(m.permission[permissionMode])
  const openPermissionMenu = (event: React.MouseEvent<HTMLElement>) => {
    if (disabled || switching || !props.onPermissionMode) return
    if (menu === 'permission') { setMenu(null); return }
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuStyle({ left: rect.left, top: rect.top - 8, transform: 'translateY(-100%)' })
    setMenu('permission')
  }

  return <div className="status-info" data-hud-mode={hud?.mode ?? 'environment'} data-session-id={sessionId} ref={rootRef}>
    {hud?.mode === 'agent' && hud.configCounts?.projectInstructionFileExists === true && <button type="button"
      className="status-field status-instructions is-clickable"
      aria-label={m.editClaudeMd} disabled={!gitClient}
      title={m.editClaudeMdTitle}
      onClick={() => {
        setMenu(null); setGitOpen(false); setEnvironmentOpen(false); setInstructionsOpen(true)
      }}>ClaudeMd</button>}
    <div className="status-info__primary">
    {hud?.mode === 'agent' ? <>
      <button type="button" className={`status-field status-perm-badge is-clickable perm-${permissionMode}`}
        disabled={disabled || switching || !props.onPermissionMode}
        title={props.disabledReason ?? permissionTitle}
        aria-label={permissionTitle}
        onClick={openPermissionMenu}>{permissionAbbreviation(permissionMode)}</button>
      {modelLabel(hud, m) && <span className="status-field status-model status-priority-8"
        title={m.modelTitle(modelLabel(hud, m))}>{modelLabel(hud, m)}</span>}
      {hud.contextPercent !== undefined && <ContextRing percent={hud.contextPercent} />}
      {hud.configCounts && hud.configCounts.mcpServers > 0 && <HudDetail
        label={`${hud.configCounts.mcpServers} MCPs`} title={m.mcpServers}
        items={(hud.configCounts.mcpServerNames ?? []).map((name) => name)} />}
      {toolTotal(hud) > 0 && <HudDetail label={`${toolTotal(hud)} Tools`} title={m.toolCalls}
        items={toolDetails(hud, m)} />}
      {(hud.subagentCount ?? 0) > 0 && <HudDetail label={`${hud.subagentCount} Agents`} title={m.subagents}
        items={hud.subagents ?? []} />}
      {hud.configCounts && hud.configCounts.hooks > 0 && <HudDetail
        label={`${hud.configCounts.hooks} hooks`} title="Hooks"
        items={hud.configCounts.hookNames ?? []} />}
      {(hud.usageWindows ?? []).map((window, index) => <span
        className="status-field status-usage status-priority-6"
        title={usageTitle(window)} key={`${window.label}:${index}`}>
        {window.label} <strong>{window.percent}%</strong>{formatReset(window.resetsAt) && <small> · {formatReset(window.resetsAt)}</small>}
      </span>)}
      {hasTaskStatusLabel(hud.taskStatus) && <span className="status-field status-priority-6">{taskStatusLabel(hud.taskStatus, m)}</span>}
      {hud.teamRole && <span className={`team-role-badge status-priority-5 team-${teamTone(hud.teamStatus)}`}>{hud.teamRole}</span>}
      {(hud.mcpErrors ?? []).map((name) => <span className="status-field status-mcp-error status-priority-5" key={name}>⚠ {name}</span>)}
      {todoDisplay(hud) && <span className="status-field status-todos status-priority-4">
        <span className={`tool-icon ${todoDisplay(hud)!.done ? 'tool-icon-done' : 'tool-icon-running'}`}>{todoDisplay(hud)!.icon}</span>
        <span>{todoDisplay(hud)!.text}</span><span className="tool-count">{todoDisplay(hud)!.progress}</span>
      </span>}
    </> : hud?.mode === 'shell' ? <>
      {hud.shell && <span className="status-field">{hud.shell}</span>}
    </> : null}
    </div>
    <div className="status-info__secondary">
    {hud?.mode === 'agent' && hasAgentInfo(hud) && (shortCwd || gitDisplay) && <span className="status-divider status-priority-3" />}
    {shortCwd && <span className="status-field status-cwd status-priority-3" title={hud?.cwd}>{shortCwd}</span>}
    {git && <button type="button" className="status-field status-git is-clickable"
      disabled={disabled || !gitClient || git.state === 'unavailable'} aria-label={m.openGit}
      title={props.disabledReason ?? gitStateTitle(git, m)}
      onClick={() => { setMenu(null); setEnvironmentOpen(false); setGitOpen((open) => !open) }}>{gitDisplay}</button>}
    {props.environment && <EnvironmentButton environment={props.environment}
      disabled={!props.environmentActions} onClick={() => {
        setMenu(null); setGitOpen(false); setEnvironmentOpen((open) => !open)
      }} />}
    </div>
    {menu && !disabled && createPortal(<div className="perm-menu-overlay" onPointerDown={(event) => {
      if (event.currentTarget === event.target) setMenu(null)
    }}><div className="perm-menu" style={menuStyle} role="menu" aria-label={m.permissionMenu}>
      <div className="perm-menu__title">{m.permissionMenu}</div>
      {PERMISSION_MODES.filter(({ value }) => value !== 'auto').map((option) => <button type="button" role="menuitem"
        className={`perm-menu__item${permissionMode === option.value ? ' is-active' : ''}`} key={option.value}
        onClick={() => {
          setMenu(null)
          if (option.value === permissionMode || !props.onPermissionMode) return
          const respawn = option.value === 'bypassPermissions' || permissionMode === 'bypassPermissions'
          if (respawn) {
            setConfirmTarget(option.value)
            return
          }
          setPermissionMode(option.value)
          void Promise.resolve(props.onPermissionMode?.(sessionId, option.value, false)).catch(() => {})
        }}><span className={`perm-menu__dot perm-${option.value}`} />
        <span className="perm-menu__label">{m.permission[option.value]}</span>
        {permissionMode === option.value && <span className="perm-menu__check">✓</span>}
      </button>)}
    </div></div>, document.body)}
    {confirmTarget && hud && !disabled && props.onPermissionMode && createPortal(<ConfirmDialog
      title={confirmTarget === 'bypassPermissions' ? m.bypassEnterTitle : m.bypassExitTitle}
      body={bypassCopy(confirmTarget, hud.resumable === true, m)}
      confirmLabel={confirmTarget === 'bypassPermissions' ? m.bypassEnterConfirm : m.bypassExitConfirm}
      onCancel={() => setConfirmTarget(null)}
      onConfirm={() => {
        const target = confirmTarget
        setConfirmTarget(null)
        setSwitching(true)
        void Promise.resolve(props.onPermissionMode?.(sessionId, target, true)).then(() => {
          setPermissionMode(target)
        }).catch((error: unknown) => {
          setSwitchError(m.switchFailed(error instanceof Error ? error.message : m.unknownError))
          window.setTimeout(() => setSwitchError(''), 3_000)
        }).finally(() => setSwitching(false))
      }} />, document.body)}
    {switchError && createPortal(<div className="terminal-toast is-error" role="status">{switchError}</div>, document.body)}
    {environmentOpen && props.environment && props.environmentActions && createPortal(
      <EnvironmentControlMenu sessionId={sessionId} environment={props.environment}
        hasOwnedWorktree={props.hasOwnedWorktree === true} actions={props.environmentActions}
        {...(props.environmentDisabledReason
          ? { mutationDisabledReason: props.environmentDisabledReason }
          : {})}
        onClose={() => setEnvironmentOpen(false)} />, document.body
    )}
    {gitOpen && !disabled && git?.state === 'ready' && gitClient && gitCwd && createPortal(<GitControlMenu client={gitClient}
      cwd={gitCwd} sessionId={sessionId} {...(props.gitContext ? { context: props.gitContext } : {})}
      dialogLabel={m.gitMenu} branchRowsAsButtons
      onClose={() => setGitOpen(false)} />, document.body)}
    {instructionsOpen && gitClient && createPortal(<InstructionFileDialog client={gitClient}
      sessionId={sessionId} readOnly={disabled} onClose={() => setInstructionsOpen(false)} />, document.body)}
  </div>
}

function EnvironmentButton(props: {
  environment: SessionEnvironment
  disabled: boolean
  onClick(): void
}) {
  const m = useMessages().hud
  const label = environmentLabel(props.environment, m)
  const localIcon = props.environment.state === 'ready' && props.environment.kind === 'local'
  return <button type="button"
    className={`status-field status-environment is-clickable state-${props.environment.state}${localIcon ? ' is-local-icon' : ''}`}
    disabled={props.disabled} aria-label={m.openEnvironment(label)} title={m.environmentMenu}
    onClick={props.onClick}>{localIcon ? <svg viewBox="0 0 16 16" aria-hidden="true">
      <rect x="2" y="2.5" width="12" height="8.5" rx="1.5" fill="none" stroke="currentColor" strokeWidth="1.3" />
      <path d="M5 13.5h6M8 11v2.5" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
    </svg> : label}</button>
}

function HudDetail(props: { label: string; title: string; items: string[] }) {
  const m = useMessages().hud
  const [open, setOpen] = useState(false)
  const [style, setStyle] = useState<CSSProperties>({})
  const closeTimer = useRef<number | undefined>(undefined)
  useEffect(() => () => window.clearTimeout(closeTimer.current), [])
  const cancelClose = () => {
    window.clearTimeout(closeTimer.current)
    closeTimer.current = undefined
  }
  const scheduleClose = () => {
    cancelClose()
    closeTimer.current = window.setTimeout(() => setOpen(false), 140)
  }
  const show = (target: HTMLElement) => {
    cancelClose()
    const rect = target.getBoundingClientRect()
    setStyle({ left: rect.left + rect.width / 2, top: rect.top - 8 })
    setOpen(true)
  }
  return <>
    <span className="status-field status-detail status-config" role="button" tabIndex={0}
      aria-label={m.detailList(props.title)}
      onMouseEnter={(event) => show(event.currentTarget)} onMouseLeave={scheduleClose}
      onFocus={(event) => show(event.currentTarget)} onBlur={scheduleClose}>{props.label}</span>
    {open && createPortal(<div className="hud-detail-tooltip" role="tooltip" style={style}
      onMouseEnter={cancelClose} onMouseLeave={scheduleClose}>
      <strong>{props.title}</strong>
      <ul>{(props.items.length > 0 ? props.items : [m.detailPending]).map((item, index) =>
        <li key={`${item}:${index}`}>{item}</li>)}</ul>
    </div>, document.body)}
  </>
}

function InstructionFileDialog(props: {
  client: GitRequestClient
  sessionId: string
  readOnly: boolean
  onClose(): void
}) {
  const messages = useMessages()
  const m = messages.hud
  const [path, setPath] = useState('')
  const [content, setContent] = useState('')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    let active = true
    void props.client.request<InstructionFileResult>('session.instructions-read', {
      sessionId: props.sessionId
    }, { timeoutMs: 10_000 }).then((result) => {
      if (!active) return
      setPath(result.path)
      setContent(result.content)
    }).catch((reason: unknown) => {
      if (active) setError(errorText(reason, m.instructionsReadFailed, m))
    }).finally(() => {
      if (active) setLoading(false)
    })
    return () => { active = false }
  }, [props.client, props.sessionId, m])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      event.stopPropagation()
      props.onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [props.onClose])

  const save = async () => {
    if (saving || props.readOnly) return
    setSaving(true)
    setError('')
    try {
      await props.client.request<InstructionFileResult>('session.instructions-write', {
        sessionId: props.sessionId, content
      }, { timeoutMs: 10_000 })
      props.onClose()
    } catch (reason) {
      setError(errorText(reason, m.instructionsSaveFailed, m))
    } finally {
      setSaving(false)
    }
  }

  return <div className="instruction-file-overlay" onPointerDown={(event) => {
    if (event.currentTarget === event.target) props.onClose()
  }}><section className="instruction-file-dialog" role="dialog" aria-modal="true" aria-label={m.editClaudeMd}>
    <header><div><h2>ClaudeMd</h2><p>{path || m.instructionsFallbackPath}</p></div>
      <button type="button" aria-label={m.instructionsClose} onClick={props.onClose}>×</button></header>
    {loading ? <div className="instruction-file-state">{m.instructionsReading}</div> : <textarea
      aria-label={m.instructionsContent} value={content} readOnly={props.readOnly}
      spellCheck={false} autoFocus onChange={(event) => setContent(event.target.value)} />}
    {error && <div className="instruction-file-error" role="alert">{error}</div>}
    <footer><span>{props.readOnly ? m.instructionsReadOnly : m.instructionsSaveHint}</span>
      <div><button type="button" onClick={props.onClose}>{messages.common.cancel}</button>
        <button type="button" className="is-primary" disabled={loading || saving || props.readOnly}
          onClick={() => void save()}>{saving ? m.saving : m.save}</button></div></footer>
  </section></div>
}

interface InstructionFileResult { path: string; content: string; exists: boolean }

function errorText(reason: unknown, label: string, m: HudMessages): string {
  return reason instanceof Error && reason.message ? m.errorDetail(label, reason.message) : label
}

function ContextRing({ percent }: { percent: number }) {
  const circumference = 2 * Math.PI * 8
  const clamped = Math.max(0, Math.min(100, percent))
  const color = percent >= 85 ? '#f85149' : percent >= 70 ? '#d29922' : '#3fb950'
  return <span className="status-field status-context-ring status-priority-7" title={`Context: ${percent}%`}>
    <svg className="context-ring-svg" viewBox="0 0 20 20" aria-hidden="true">
      <circle className="context-ring-bg" cx="10" cy="10" r="8" fill="none" strokeWidth="2.5" />
      <circle className="context-ring-fg" cx="10" cy="10" r="8" fill="none" strokeWidth="2.5"
        stroke={color} strokeDasharray={`${clamped / 100 * circumference} ${circumference}`}
        strokeLinecap="round" transform="rotate(-90 10 10)" />
    </svg><span className="context-ring-label" style={{ color }}>{percent}%</span>
  </span>
}

function cwdShortName(cwd: string | undefined): string {
  const tail = cwd?.split(/[\\/]/).filter(Boolean).at(-1)
  return tail ? `~/${tail}` : ''
}
function legacyGitState(hud: SessionHudView | undefined): SessionGitState | undefined {
  return hud?.gitBranch
    ? { state: 'ready', branch: hud.gitBranch, dirty: hud.gitDirty === true }
    : undefined
}
function gitStateLabel(git: SessionGitState, m: HudMessages): string {
  if (git.state === 'unavailable') return m.gitUnavailable
  const reference = git.branch ?? `HEAD ${git.detachedHead.slice(0, 7)}`
  return `${reference}${git.dirty ? '*' : ''}`
}
function gitStateTitle(git: SessionGitState, m: HudMessages): string {
  if (git.state === 'unavailable') return m.gitTitle.unavailable
  return git.branch === undefined ? m.gitTitle.detached(git.detachedHead) : m.gitTitle.branch(git.branch)
}
function permissionAbbreviation(mode: HudPermissionMode): string {
  return PERMISSION_MODES.find(({ value }) => value === mode)?.abbreviation ?? 'D'
}
function modelLabel(hud: SessionHudView, m: HudMessages): string {
  const name = hud.model?.trim()
  let label = name?.replace(/^Claude\s+/i, '') ?? ''
  if (!label && hud.modelStrategy) label = m.model[hud.modelStrategy]
  const context = formatContextWindow(hud.contextWindowSize)
  return label && context ? m.modelWithContext(label, context) : label
}
function formatContextWindow(size: number | undefined): string {
  if (!size) return ''
  if (size >= 1_000_000) return `${Number((size / 1_000_000).toFixed(1))}M`
  if (size >= 1_000) return `${Math.round(size / 1_000)}K`
  return String(Math.round(size))
}
function formatReset(resetsAt: number | undefined): string {
  if (!resetsAt) return ''
  const minutes = Math.max(0, Math.ceil((resetsAt - Date.now()) / 60_000))
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h${minutes % 60 ? `${minutes % 60}m` : ''}`
  const days = Math.floor(hours / 24)
  return `${days}d${hours % 24 ? `${hours % 24}h` : ''}`
}
function usageTitle(window: NonNullable<SessionHudView['usageWindows']>[number]): string {
  const reset = formatReset(window.resetsAt)
  return `${window.label} usage: ${window.percent}%${reset ? ` · resets in ${reset}` : ''}`
}
function toolTotal(hud: SessionHudView): number {
  return (hud.runningTools?.length ?? 0) +
    (hud.toolCounts ?? []).reduce((total, tool) => total + tool.count, 0)
}
function toolDetails(hud: SessionHudView, m: HudMessages): string[] {
  return [
    ...(hud.runningTools ?? []).map(({ name, target }) => m.toolRunning(name, target)),
    ...(hud.toolCounts ?? []).map(({ name, count }) => m.toolCallCount(name, count))
  ]
}
function hasTaskStatusLabel(status: SessionHudView['taskStatus']): boolean {
  return status === 'needs-input' || status === 'error'
}
function taskStatusLabel(status: SessionHudView['taskStatus'], m: HudMessages): string {
  if (status === 'needs-input') return m.taskStatus['needs-input']
  if (status === 'error') return m.taskStatus.error
  return ''
}
function teamTone(status: SessionHudView['teamStatus']): string {
  if (status === 'running') return 'running'
  if (status === 'needs-input') return 'input'
  if (status === 'error') return 'error'
  return 'idle'
}
function todoDisplay(hud: SessionHudView): { icon: string; text: string; progress: string; done: boolean } | null {
  const todos = hud.todos ?? []
  if (todos.length === 0) return null
  const completed = todos.filter(({ status }) => status === 'completed').length
  const inProgress = todos.find(({ status }) => status === 'in_progress')
  if (inProgress) return {
    icon: '▸', text: inProgress.content.length > 50 ? `${inProgress.content.slice(0, 47)}...` : inProgress.content,
    progress: `(${completed}/${todos.length})`, done: false
  }
  return completed === todos.length
    ? { icon: '✓', text: 'All todos complete', progress: `(${completed}/${todos.length})`, done: true }
    : null
}
function hasAgentInfo(hud: SessionHudView): boolean {
  return Boolean(hud.modelStrategy || hud.contextPercent !== undefined || hasTaskStatusLabel(hud.taskStatus) ||
    (hud.usageWindows?.length ?? 0) > 0 || hud.configCounts || (hud.mcpErrors?.length ?? 0) > 0 ||
    (hud.subagentCount ?? 0) > 0 || hud.teamRole || todoDisplay(hud))
}

function bypassCopy(target: HudPermissionMode, resumable: boolean, m: HudMessages): string {
  if (!resumable) return m.bypassNotResumable
  return target === 'bypassPermissions' ? m.bypassEnterBody : m.bypassExitBody
}
