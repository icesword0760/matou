import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'

import type { AppUpdateState } from '../../../shared/desktop-api'

const INITIAL_STATE: AppUpdateState = { status: 'idle', currentVersion: '' }
const LAST_VERSION_KEY = 'matou:last-seen-app-version'

export function AppUpdateControl({ activeSessionCount }: { activeSessionCount: number }) {
  const [state, setState] = useState<AppUpdateState>(INITIAL_STATE)
  const [open, setOpen] = useState(false)
  const [waitingForIdle, setWaitingForIdle] = useState(false)
  const [showUpdatedToast, setShowUpdatedToast] = useState(false)
  const [installing, setInstalling] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const installRequestedRef = useRef(false)

  useEffect(() => {
    const api = window.matouDesktop
    let alive = true
    const accept = (next: AppUpdateState) => {
      if (!alive) return
      setState(next)
      if (next.status === 'available' || next.status === 'downloaded') setOpen(true)
    }
    const unsubscribe = api?.onAppUpdateState?.(accept)
    void api?.getAppUpdateState?.().then(accept)
    return () => { alive = false; unsubscribe?.() }
  }, [])

  useEffect(() => {
    if (!state.currentVersion) return
    const previous = localStorage.getItem(LAST_VERSION_KEY)
    if (previous && previous !== state.currentVersion) {
      setShowUpdatedToast(true)
      const timer = window.setTimeout(() => setShowUpdatedToast(false), 5_000)
      localStorage.setItem(LAST_VERSION_KEY, state.currentVersion)
      return () => window.clearTimeout(timer)
    }
    localStorage.setItem(LAST_VERSION_KEY, state.currentVersion)
  }, [state.currentVersion])

  useEffect(() => {
    if (!open) return
    const closeOutside = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false)
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    window.addEventListener('pointerdown', closeOutside, true)
    window.addEventListener('keydown', closeOnEscape)
    return () => {
      window.removeEventListener('pointerdown', closeOutside, true)
      window.removeEventListener('keydown', closeOnEscape)
    }
  }, [open])

  useEffect(() => {
    if (!waitingForIdle || activeSessionCount > 0 || state.status !== 'downloaded' || installRequestedRef.current) return
    installRequestedRef.current = true
    setInstalling(true)
    void window.matouDesktop?.installAppUpdate?.()
  }, [activeSessionCount, state.status, waitingForIdle])

  const label = useMemo(() => updateButtonLabel(state), [state])
  const progress = state.status === 'downloading' ? Math.round(state.progress.percent) : undefined

  const installNow = () => {
    if (installRequestedRef.current) return
    installRequestedRef.current = true
    setInstalling(true)
    void window.matouDesktop?.installAppUpdate?.()
  }

  return <div className="app-update-control" ref={rootRef}>
    <button type="button" className={`app-update-trigger is-${state.status}`}
      aria-label={label} aria-expanded={open} onClick={() => setOpen((value) => !value)}>
      {progress === undefined
        ? <DownloadIcon />
        : <span className="app-update-trigger__progress" style={{ '--update-progress': `${progress * 3.6}deg` } as CSSProperties}>
            <DownloadIcon />
          </span>}
      {(state.status === 'available' || state.status === 'downloaded') && <i className="app-update-trigger__dot" />}
    </button>

    {open && <section role="dialog" aria-label="Matou 应用更新" className="app-update-popover">
      <UpdateHeader state={state} onClose={() => setOpen(false)} />
      <div className="app-update-popover__body">
        {state.status === 'idle' && <p>当前版本 {state.currentVersion || '—'}</p>}
        {state.status === 'checking' && <p>正在检查云端是否有新版本…</p>}
        {state.status === 'not-available' && <p>当前已是最新版本（{state.currentVersion}）</p>}
        {state.status === 'error' && <div className="app-update-error">
          <strong>更新检查失败</strong><span>{friendlyError(state.errorMessage)}</span>
        </div>}
        {isReleaseState(state) && state.status !== 'downloading' && state.releaseNotes.length > 0 &&
          <ul className="app-update-notes">{state.releaseNotes.slice(0, 3).map((note) => <li key={note}>{note}</li>)}</ul>}
        {state.status === 'downloading' && <DownloadProgress state={state} />}
        {state.status === 'downloaded' && activeSessionCount > 0 && <div className="app-update-session-warning">
          <i /><span><strong>当前有 {activeSessionCount} 个活动会话</strong>
            <small>空闲后更新会保留工作区、画布位置及会话恢复信息。</small></span>
        </div>}
        {waitingForIdle && <div className="app-update-waiting">已安排：空闲后自动更新</div>}
      </div>
      <div className="app-update-popover__actions">
        {(state.status === 'idle' || state.status === 'not-available' || state.status === 'error') &&
          <button className="is-primary" onClick={() => void window.matouDesktop?.checkForAppUpdates?.()}>
            {state.status === 'error' ? '重新检查' : '检查更新'}
          </button>}
        {state.status === 'available' && <>
          <button className="is-primary" onClick={() => void window.matouDesktop?.downloadAppUpdate?.()}>后台下载</button>
          <button className="is-quiet" onClick={() => setOpen(false)}>稍后提醒</button>
        </>}
        {state.status === 'downloading' && <button className="is-quiet" onClick={() => setOpen(false)}>继续在后台下载</button>}
        {state.status === 'downloaded' && activeSessionCount > 0 && <>
          <button className={waitingForIdle ? '' : 'is-primary'} disabled={installing}
            onClick={() => setWaitingForIdle((waiting) => !waiting)}>
            {waitingForIdle ? '取消空闲更新' : '空闲后自动更新'}
          </button>
          <div className="app-update-action-row">
            <button disabled={installing} onClick={installNow}>立即重启并更新</button>
            <button onClick={() => setOpen(false)}>退出时安装</button>
          </div>
        </>}
        {state.status === 'downloaded' && activeSessionCount === 0 && <>
          <button className="is-primary" disabled={installing} onClick={installNow}>{installing ? '正在准备更新…' : '重启并更新'}</button>
          <button className="is-quiet" onClick={() => setOpen(false)}>退出时安装</button>
        </>}
      </div>
    </section>}

    {showUpdatedToast && <div className="app-update-toast" role="status">
      <span>✓</span><div><strong>Matou 已更新至 {state.currentVersion}</strong><small>工作空间与会话已恢复</small></div>
    </div>}
  </div>
}

function UpdateHeader({ state, onClose }: { state: AppUpdateState; onClose: () => void }) {
  const title = state.status === 'available' ? `Matou ${state.version} 可用`
    : state.status === 'downloading' ? '正在后台下载'
    : state.status === 'downloaded' ? '更新已准备好'
    : state.status === 'checking' ? '正在检查更新'
    : state.status === 'error' ? '应用更新'
    : state.status === 'not-available' ? 'Matou 已是最新版本'
    : 'Matou 应用更新'
  const subtitle = isReleaseState(state)
    ? [state.status === 'available' ? '稳定版' : `Matou ${state.version}`, state.sizeBytes ? formatBytes(state.sizeBytes) : '', formatDate(state.releaseDate)].filter(Boolean).join(' · ')
    : `当前版本 ${state.currentVersion || '—'}`
  return <header className="app-update-popover__header">
    <span className="app-update-release-icon"><DownloadIcon /></span>
    <span><strong>{title}</strong><small>{subtitle}</small></span>
    <button type="button" aria-label="关闭更新浮层" onClick={onClose}>×</button>
  </header>
}

function DownloadProgress({ state }: { state: Extract<AppUpdateState, { status: 'downloading' }> }) {
  const progress = state.progress
  return <div className="app-update-download">
    <div><span>{formatBytes(progress.transferredBytes)} / {formatBytes(progress.totalBytes)}</span>
      <span>{Math.round(progress.percent)}%{progress.remainingSeconds === undefined ? '' : ` · 约 ${progress.remainingSeconds} 秒`}</span></div>
    <span className="app-update-download__track"><i style={{ width: `${Math.max(0, Math.min(100, progress.percent))}%` }} /></span>
    <small>{formatBytes(progress.bytesPerSecond)}/秒 · 可继续使用当前会话</small>
  </div>
}

function DownloadIcon() {
  return <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M12 3v11"/><path d="m7.5 9.5 4.5 4.5 4.5-4.5"/><path d="M5 18.5h14"/>
  </svg>
}

function isReleaseState(state: AppUpdateState): state is Extract<AppUpdateState, { status: 'available' | 'downloading' | 'downloaded' }> {
  return state.status === 'available' || state.status === 'downloading' || state.status === 'downloaded'
}

function updateButtonLabel(state: AppUpdateState): string {
  if (state.status === 'checking') return '应用更新：正在检查'
  if (state.status === 'available') return `应用更新：发现 ${state.version}`
  if (state.status === 'downloading') return `应用更新：下载中 ${Math.round(state.progress.percent)}%`
  if (state.status === 'downloaded') return '应用更新：等待安装'
  if (state.status === 'error') return '应用更新：检查失败'
  return '应用更新'
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB'
  return `${(bytes / 1_000_000).toFixed(1)} MB`
}

function formatDate(value?: string): string {
  if (!value) return ''
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : new Intl.DateTimeFormat('zh-CN', { month: 'numeric', day: 'numeric' }).format(date)
}

function friendlyError(message: string): string {
  if (/network|server|fetch|ECONN|timeout/i.test(message)) return '暂时没有连接到更新服务器，请检查网络后重试。'
  return '更新服务出现异常，请稍后重新检查。'
}
