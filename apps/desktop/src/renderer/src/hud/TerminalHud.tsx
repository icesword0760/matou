import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'

import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { GitControlMenu, type GitControlContext, type GitRequestClient } from './GitControlMenu'
import {
  EnvironmentControlMenu,
  environmentLabel,
  type SessionEnvironmentActions
} from './EnvironmentControlMenu'
import type {
  HudModelStrategy, HudPermissionMode, SessionHudView
} from '../hierarchy/hierarchy-types'
import type { SessionEnvironment, SessionGitState } from '@matou/domain'

const PERMISSION_MODES: Array<{ value: HudPermissionMode; label: string }> = [
  { value: 'default', label: 'Default' },
  { value: 'acceptEdits', label: 'Accept Edits' },
  { value: 'plan', label: 'Plan Mode' },
  { value: 'bypassPermissions', label: 'Bypass Permissions' }
]
export function TerminalHud(props: {
  hud: SessionHudView | undefined
  sessionId?: string
  onPermissionMode(sessionId: string, mode: HudPermissionMode, respawn: boolean): unknown
  onModel?(sessionId: string, strategy: HudModelStrategy): unknown
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
  const [elapsed, setElapsed] = useState(() => formatElapsed(hud?.startedAt))
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => setPermissionMode(hud?.permissionMode ?? 'default'), [hud?.sessionId, hud?.permissionMode])
  useEffect(() => {
    setElapsed(formatElapsed(hud?.startedAt))
    const timer = window.setInterval(() => setElapsed(formatElapsed(hud?.startedAt)), 10_000)
    return () => window.clearInterval(timer)
  }, [hud?.startedAt])
  useEffect(() => {
    if (!disabled) return
    setMenu(null)
    setConfirmTarget(null)
    setGitOpen(false)
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
  const gitDisplay = git ? gitStateLabel(git) : ''
  const gitCwd = props.environment?.state === 'ready' ? props.environment.path : hud?.cwd
  const openMenu = (event: React.MouseEvent<HTMLElement>) => {
    if (disabled || switching) return
    if (menu === 'permission') { setMenu(null); return }
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuStyle({ left: rect.left, top: rect.top - 8, transform: 'translateY(-100%)' })
    setMenu('permission')
  }

  return <div className="status-info" data-hud-mode={hud?.mode ?? 'environment'} data-session-id={sessionId} ref={rootRef}>
    {hud?.mode === 'agent' ? <>
      <button type="button" className={`status-field status-perm-badge is-clickable perm-${permissionMode}`}
        disabled={disabled || switching}
        title={props.disabledReason ?? `当前权限模式：${permissionLabel(permissionMode)}，点击切换`}
        aria-label={`当前权限模式：${permissionLabel(permissionMode)}，点击切换`}
        onClick={openMenu}>{permissionLabel(permissionMode)}</button>
      {hud.contextPercent !== undefined && <ContextRing percent={hud.contextPercent} />}
      {taskStatusLabel(hud.taskStatus) && <span className="status-field status-priority-6">{taskStatusLabel(hud.taskStatus)}</span>}
      {(hud.subagentCount ?? 0) > 0 && <span className="status-field status-priority-5">Agent:{hud.subagentCount}</span>}
      {hud.teamRole && <span className={`team-role-badge status-priority-5 team-${teamTone(hud.teamStatus)}`}>{hud.teamRole}</span>}
      {runningTools(hud).map((tool, index) => <span className="status-field status-tool-running status-priority-4"
        key={`${tool.name}:${tool.target ?? ''}:${index}`}>
        <span className="tool-icon tool-icon-running">◐</span><span className="tool-name">{tool.name}</span>
        {tool.target && <span className="tool-target">: {truncatePath(tool.target)}</span>}
      </span>)}
      {todoDisplay(hud) && <span className="status-field status-todos status-priority-4">
        <span className={`tool-icon ${todoDisplay(hud)!.done ? 'tool-icon-done' : 'tool-icon-running'}`}>{todoDisplay(hud)!.icon}</span>
        <span>{todoDisplay(hud)!.text}</span><span className="tool-count">{todoDisplay(hud)!.progress}</span>
      </span>}
      {hasAgentInfo(hud) && (shortCwd || gitDisplay || elapsed) && <span className="status-divider status-priority-3" />}
      {shortCwd && <span className="status-field status-priority-3">{shortCwd}</span>}
    </> : hud?.mode === 'shell' ? <>
      {hud.shell && <span className="status-field">{hud.shell}</span>}
      {shortCwd && <span className="status-field status-priority-3">{shortCwd}</span>}
    </> : null}
    {git && <button type="button" className="status-field status-git is-clickable"
      disabled={disabled || !gitClient || git.state === 'unavailable'} aria-label="打开 Git"
      title={props.disabledReason ?? gitStateTitle(git)}
      onClick={() => { setMenu(null); setEnvironmentOpen(false); setGitOpen((open) => !open) }}>{gitDisplay}</button>}
    {props.environment && <EnvironmentButton environment={props.environment}
      disabled={!props.environmentActions} onClick={() => {
        setMenu(null); setGitOpen(false); setEnvironmentOpen((open) => !open)
      }} />}
    {elapsed && <span className="status-field status-priority-1">⏱{elapsed}</span>}
    {menu && !disabled && createPortal(<div className="perm-menu-overlay" onPointerDown={(event) => {
      if (event.currentTarget === event.target) setMenu(null)
    }}><div className="perm-menu" style={menuStyle} role="menu" aria-label="权限模式">
      <div className="perm-menu__title">权限模式</div>
      {PERMISSION_MODES.map((option) => <button type="button" role="menuitem"
        className={`perm-menu__item${permissionMode === option.value ? ' is-active' : ''}`} key={option.value}
        onClick={() => {
          setMenu(null)
          if (option.value === permissionMode) return
          const respawn = option.value === 'bypassPermissions' || permissionMode === 'bypassPermissions'
          if (respawn) { setConfirmTarget(option.value); return }
          setPermissionMode(option.value)
          void Promise.resolve(props.onPermissionMode(sessionId, option.value, false)).catch(() => {})
        }}><span className={`perm-menu__dot perm-${option.value}`} />
        <span className="perm-menu__label">{option.label}</span>
        {permissionMode === option.value && <span className="perm-menu__check">✓</span>}
      </button>)}
    </div></div>, document.body)}
    {confirmTarget && hud && !disabled && createPortal(<ConfirmDialog title={confirmTarget === 'bypassPermissions' ? '切换到高权限模式' : '退出高权限模式'}
      body={bypassCopy(confirmTarget, hud.resumable === true)}
      confirmLabel={confirmTarget === 'bypassPermissions' ? '确认切换' : '确认退出'} onCancel={() => setConfirmTarget(null)}
      onConfirm={() => {
        const target = confirmTarget
        setConfirmTarget(null)
        setSwitching(true)
        void Promise.resolve(props.onPermissionMode(sessionId, target, true)).then(() => {
          setPermissionMode(target)
        }).catch((error: unknown) => {
          setSwitchError(`切换失败：${error instanceof Error ? error.message : '未知错误'}`)
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
      dialogLabel="Git 与 Worktree" branchRowsAsButtons
      onClose={() => setGitOpen(false)} />, document.body)}
  </div>
}

function EnvironmentButton(props: {
  environment: SessionEnvironment
  disabled: boolean
  onClick(): void
}) {
  const label = environmentLabel(props.environment)
  return <button type="button"
    className={`status-field status-environment is-clickable state-${props.environment.state}`}
    disabled={props.disabled} aria-label={`打开运行环境：${label}`} title="运行环境"
    onClick={props.onClick}>{label}</button>
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

function formatElapsed(startedAt: number | undefined): string {
  if (!startedAt) return ''
  const seconds = Math.max(0, Math.floor((Date.now() - startedAt) / 1000))
  if (seconds < 60) return `${seconds}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h${Math.floor(seconds % 3600 / 60)}m`
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
function gitStateLabel(git: SessionGitState): string {
  if (git.state === 'unavailable') return 'Git 不可用'
  const reference = git.branch ?? `HEAD ${git.detachedHead.slice(0, 7)}`
  return `${reference}${git.dirty ? '*' : ''}`
}
function gitStateTitle(git: SessionGitState): string {
  if (git.state === 'unavailable') return '当前目录不是可用的 Git 工作区'
  return git.branch ? `Git 分支：${git.branch}` : `Git detached HEAD：${git.detachedHead}`
}
function permissionLabel(mode: HudPermissionMode): string {
  return PERMISSION_MODES.find(({ value }) => value === mode)?.label ?? 'Default'
}
function taskStatusLabel(status: SessionHudView['taskStatus']): string {
  if (status === 'running') return '任务中'
  if (status === 'needs-input') return '待输入'
  if (status === 'error') return '错误'
  return ''
}
function teamTone(status: SessionHudView['teamStatus']): string {
  if (status === 'running') return 'running'
  if (status === 'needs-input') return 'input'
  if (status === 'error') return 'error'
  return 'idle'
}
function runningTools(hud: SessionHudView) {
  return (hud.runningTools ?? []).filter(({ name }) => name !== 'Bash' && name !== 'Skill').slice(-2)
}
function truncatePath(value: string, maxLength = 20): string {
  const normalized = value.replace(/\\/g, '/')
  if (normalized.length <= maxLength) return normalized
  const file = normalized.split('/').at(-1) ?? normalized
  return file.length >= maxLength ? `${file.slice(0, maxLength - 3)}...` : `.../${file}`
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
  return Boolean(hud.modelStrategy || hud.contextPercent !== undefined || taskStatusLabel(hud.taskStatus) ||
    (hud.subagentCount ?? 0) > 0 || hud.teamRole || runningTools(hud).length || todoDisplay(hud))
}
function bypassCopy(target: HudPermissionMode, resumable: boolean): string {
  if (!resumable) return '当前 Claude 会话还没有生成可恢复的 sessionId。继续切换会启动一个全新的 Claude 会话，当前内容将不会保留。\n\n是否确认？'
  return target === 'bypassPermissions'
    ? '切换到 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？'
    : '退出 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？'
}
