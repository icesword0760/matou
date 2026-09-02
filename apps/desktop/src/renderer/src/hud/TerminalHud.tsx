import { useEffect, useRef, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'

import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { GitControlMenu, type GitControlContext, type GitRequestClient } from './GitControlMenu'
import type { HudPermissionMode, SessionHudView } from '../hierarchy/hierarchy-types'

const PERMISSION_MODES: Array<{ value: HudPermissionMode; label: string }> = [
  { value: 'default', label: 'Default' },
  { value: 'auto', label: 'Auto' },
  { value: 'acceptEdits', label: 'Accept Edits' },
  { value: 'plan', label: 'Plan Mode' },
  { value: 'bypassPermissions', label: 'Bypass Permissions' }
]
export function TerminalHud(props: {
  hud: SessionHudView | undefined
  onPermissionMode?(sessionId: string, mode: HudPermissionMode, respawn: boolean): unknown
  gitContext?: GitControlContext
  runtimeClient?: GitRequestClient
}) {
  const { hud } = props
  const contextClient = useRuntimeClient()
  const gitClient = props.runtimeClient ?? contextClient
  const [permissionMode, setPermissionMode] = useState<HudPermissionMode>(hud?.permissionMode ?? 'default')
  const [menu, setMenu] = useState<'permission' | null>(null)
  const [menuStyle, setMenuStyle] = useState<CSSProperties>({})
  const [confirmTarget, setConfirmTarget] = useState<HudPermissionMode | null>(null)
  const [switching, setSwitching] = useState(false)
  const [switchError, setSwitchError] = useState('')
  const [gitOpen, setGitOpen] = useState(false)
  const [elapsed, setElapsed] = useState(() => formatElapsed(hud?.startedAt))
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => setPermissionMode(hud?.permissionMode ?? 'default'), [hud?.sessionId, hud?.permissionMode])
  useEffect(() => {
    setElapsed(formatElapsed(hud?.startedAt))
    const timer = window.setInterval(() => setElapsed(formatElapsed(hud?.startedAt)), 10_000)
    return () => window.clearInterval(timer)
  }, [hud?.startedAt])
  useEffect(() => {
    const closeOutside = (event: Event) => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (!rootRef.current?.contains(target) && !(target instanceof Element && target.closest('.perm-menu'))) {
        setMenu(null)
      }
    }
    const closeEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || !menu) return
      setMenu(null)
      event.preventDefault()
      event.stopPropagation()
    }
    document.addEventListener('pointerdown', closeOutside, true)
    document.addEventListener('keydown', closeEscape, true)
    return () => {
      document.removeEventListener('pointerdown', closeOutside, true)
      document.removeEventListener('keydown', closeEscape, true)
    }
  }, [menu])
  if (!hud) return null
  const shortCwd = cwdShortName(hud.cwd)
  const gitDisplay = hud.gitBranch ? `${hud.gitBranch}${hud.gitDirty ? '*' : ''}` : ''
  const openPermissionMenu = (event: React.MouseEvent<HTMLElement>) => {
    if (switching || !props.onPermissionMode) return
    if (menu === 'permission') {
      setMenu(null)
      return
    }
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuStyle({ left: rect.left, top: rect.top - 8, transform: 'translateY(-100%)' })
    setMenu('permission')
  }

  return <div className="status-info" data-hud-mode={hud.mode} data-session-id={hud.sessionId} ref={rootRef}>
    {hud.mode === 'agent' ? <>
      <button type="button" className={`status-field status-perm-badge is-clickable perm-${permissionMode}`}
        disabled={switching || !props.onPermissionMode}
        title={`当前权限模式：${permissionLabel(permissionMode)}，点击切换`}
        aria-label={`当前权限模式：${permissionLabel(permissionMode)}，点击切换`}
        onClick={openPermissionMenu}>{permissionLabel(permissionMode)}</button>
      {modelLabel(hud) && <span className="status-field status-model status-priority-8"
        title={`当前模型：${modelLabel(hud)}`}>{modelLabel(hud)}</span>}
      {hud.contextPercent !== undefined && <ContextRing percent={hud.contextPercent} />}
      {(hud.usageWindows ?? []).map((window, index) => <span
        className="status-field status-usage status-priority-6"
        title={usageTitle(window)} key={`${window.label}:${index}`}>
        {window.label} <strong>{window.percent}%</strong>{formatReset(window.resetsAt) && <small> · {formatReset(window.resetsAt)}</small>}
      </span>)}
      {taskStatusLabel(hud.taskStatus) && <span className="status-field status-priority-6">{taskStatusLabel(hud.taskStatus)}</span>}
      {(hud.subagentCount ?? 0) > 0 && <span className="status-field status-priority-5">Agent:{hud.subagentCount}</span>}
      {hud.teamRole && <span className={`team-role-badge status-priority-5 team-${teamTone(hud.teamStatus)}`}>{hud.teamRole}</span>}
      {configDisplays(hud).map((item) => <span className="status-field status-config status-priority-5" key={item}>{item}</span>)}
      {(hud.mcpErrors ?? []).map((name) => <span className="status-field status-mcp-error status-priority-5" key={name}>⚠ {name}</span>)}
      {runningTools(hud).map((tool, index) => <span className="status-field status-tool-running status-priority-4"
        key={`${tool.name}:${tool.target ?? ''}:${index}`}>
        <span className="tool-icon tool-icon-running">◐</span><span className="tool-name">{tool.name}</span>
        {tool.target && <span className="tool-target">: {truncatePath(tool.target)}</span>}
      </span>)}
      {recentTool(hud) && <span className={`status-field status-last-tool status-priority-4 is-${recentTool(hud)!.status}`}>
        <span className="tool-icon">{recentTool(hud)!.status === 'error' ? '⚠' : '✓'}</span>
        <span className="tool-name">{toolDisplayName(recentTool(hud)!.name)}</span>
        {recentTool(hud)!.target && recentTool(hud)!.name.toLowerCase() !== 'bash' &&
          <span className="tool-target">: {truncateText(recentTool(hud)!.target!, 24)}</span>}
      </span>}
      {completedTools(hud).map((tool) => <span className="status-field status-tool-done status-priority-4" key={tool.name}>
        <span className="tool-icon tool-icon-done">✓</span><span className="tool-name">{tool.name}</span>
        <span className="tool-count">×{tool.count}</span>
      </span>)}
      {todoDisplay(hud) && <span className="status-field status-todos status-priority-4">
        <span className={`tool-icon ${todoDisplay(hud)!.done ? 'tool-icon-done' : 'tool-icon-running'}`}>{todoDisplay(hud)!.icon}</span>
        <span>{todoDisplay(hud)!.text}</span><span className="tool-count">{todoDisplay(hud)!.progress}</span>
      </span>}
      {hasAgentInfo(hud) && (shortCwd || gitDisplay || elapsed) && <span className="status-divider status-priority-3" />}
      {shortCwd && <span className="status-field status-priority-3">{shortCwd}</span>}
      {gitDisplay && <button type="button" className="status-field status-git status-priority-2 is-clickable"
        disabled={!gitClient} aria-label="打开 Git 控制" title="Git"
        onClick={() => { setMenu(null); setGitOpen((open) => !open) }}>{gitDisplay}</button>}
      {elapsed && <span className="status-field status-priority-1">⏱{elapsed}</span>}
    </> : <>
      {hud.shell && <span className="status-field">{hud.shell}</span>}
      {shortCwd && <span className="status-field status-priority-3">{shortCwd}</span>}
      {gitDisplay && <button type="button" className="status-field status-git status-priority-2 is-clickable"
        disabled={!gitClient} aria-label="打开 Git 控制" title="Git"
        onClick={() => { setMenu(null); setGitOpen((open) => !open) }}>{gitDisplay}</button>}
      {elapsed && <span className="status-field status-priority-1">⏱{elapsed}</span>}
    </>}
    {menu && createPortal(<div className="perm-menu-overlay" onPointerDown={(event) => {
      if (event.currentTarget === event.target) setMenu(null)
    }}><div className="perm-menu" style={menuStyle} role="menu" aria-label="权限模式">
      <div className="perm-menu__title">权限模式</div>
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
          void Promise.resolve(props.onPermissionMode(hud.sessionId, option.value, false)).catch(() => {})
        }}><span className={`perm-menu__dot perm-${option.value}`} />
        <span className="perm-menu__label">{option.label}</span>
        {permissionMode === option.value && <span className="perm-menu__check">✓</span>}
      </button>)}
    </div></div>, document.body)}
    {confirmTarget && props.onPermissionMode && createPortal(<ConfirmDialog
      title={confirmTarget === 'bypassPermissions' ? '切换到高权限模式' : '退出高权限模式'}
      body={bypassCopy(confirmTarget, hud.resumable === true)}
      confirmLabel={confirmTarget === 'bypassPermissions' ? '确认切换' : '确认退出'}
      onCancel={() => setConfirmTarget(null)}
      onConfirm={() => {
        const target = confirmTarget
        setConfirmTarget(null)
        setSwitching(true)
        void Promise.resolve(props.onPermissionMode!(hud.sessionId, target, true)).then(() => {
          setPermissionMode(target)
        }).catch((error: unknown) => {
          setSwitchError(`切换失败：${error instanceof Error ? error.message : '未知错误'}`)
          window.setTimeout(() => setSwitchError(''), 3_000)
        }).finally(() => setSwitching(false))
      }} />, document.body)}
    {switchError && createPortal(<div className="terminal-toast is-error" role="status">{switchError}</div>, document.body)}
    {gitOpen && gitClient && hud.cwd && createPortal(<GitControlMenu client={gitClient}
      cwd={hud.cwd} sessionId={hud.sessionId} {...(props.gitContext ? { context: props.gitContext } : {})}
      onClose={() => setGitOpen(false)} />, document.body)}
  </div>
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
function permissionLabel(mode: HudPermissionMode): string {
  return PERMISSION_MODES.find(({ value }) => value === mode)?.label ?? 'Default'
}
function modelLabel(hud: SessionHudView): string {
  const name = hud.model?.trim()
  let label = name?.replace(/^Claude\s+/i, '') ?? ''
  if (!label && hud.modelStrategy === 'claude-opus-4-6') label = 'Opus 4.6'
  if (!label && hud.modelStrategy === 'claude-sonnet-4-6') label = 'Sonnet 4.6'
  if (!label && hud.modelStrategy === 'opusplan') label = 'Opus Plan'
  const context = formatContextWindow(hud.contextWindowSize)
  return label && context ? `${label} (${context} context)` : label
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
function configDisplays(hud: SessionHudView): string[] {
  const counts = hud.configCounts
  if (!counts) return []
  return [
    counts.instructionFiles > 0 ? `${counts.instructionFiles} CLAUDE.md` : '',
    counts.mcpServers > 0 ? `${counts.mcpServers} MCPs` : '',
    counts.hooks > 0 ? `${counts.hooks} hooks` : ''
  ].filter(Boolean)
}
function completedTools(hud: SessionHudView): NonNullable<SessionHudView['toolCounts']> {
  return [...(hud.toolCounts ?? [])]
    .filter(({ count }) => count > 0)
    .sort((left, right) => right.count - left.count)
    .slice(0, 4)
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
function recentTool(hud: SessionHudView): SessionHudView['lastTool'] {
  return hud.lastTool?.status === 'running' ? undefined : hud.lastTool
}
function toolDisplayName(name: string): string {
  const match = /^mcp__(.+?)__(.+)$/.exec(name)
  return match ? `${match[1]}: ${match[2]}` : name
}
function truncatePath(value: string, maxLength = 20): string {
  const normalized = value.replace(/\\/g, '/')
  if (normalized.length <= maxLength) return normalized
  const file = normalized.split('/').at(-1) ?? normalized
  return file.length >= maxLength ? `${file.slice(0, maxLength - 3)}...` : `.../${file}`
}
function truncateText(value: string, maxLength: number): string {
  return value.length > maxLength ? `${value.slice(0, maxLength - 1)}…` : value
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
    (hud.usageWindows?.length ?? 0) > 0 || configDisplays(hud).length || (hud.mcpErrors?.length ?? 0) > 0 ||
    completedTools(hud).length || recentTool(hud) || (hud.subagentCount ?? 0) > 0 || hud.teamRole ||
    runningTools(hud).length || todoDisplay(hud))
}

function bypassCopy(target: HudPermissionMode, resumable: boolean): string {
  if (!resumable) return '当前 Claude 会话还没有生成可恢复的 sessionId。继续切换会启动一个全新的 Claude 会话，当前内容将不会保留。\n\n是否确认？'
  return target === 'bypassPermissions'
    ? '切换到 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？'
    : '退出 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？'
}
