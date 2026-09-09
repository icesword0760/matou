import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react'

import type { Locale } from '@matou/contracts'

import type { AppUpdateErrorStage, AppUpdateState } from '../../../shared/desktop-api'
import { useLocale, useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'
import desktopPackage from '../../../../package.json'

type UpdateMessages = Messages['updates']

const INITIAL_STATE: AppUpdateState = { status: 'idle', currentVersion: desktopPackage.version }
const LAST_VERSION_KEY = 'matou:last-seen-app-version'

export function AppUpdateControl({ activeSessionCount }: { activeSessionCount: number }) {
  const m = useMessages().updates
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
    if (!waitingForIdle || activeSessionCount > 0 || state.status !== 'downloaded'
      || state.installMode !== 'automatic' || installRequestedRef.current) return
    installRequestedRef.current = true
    setInstalling(true)
    void window.matouDesktop?.installAppUpdate?.()
  }, [activeSessionCount, state.status, waitingForIdle])

  const label = useMemo(() => updateButtonLabel(state, m), [state, m])
  const progress = state.status === 'downloading' ? Math.round(state.progress.percent) : undefined

  const installNow = () => {
    if (installRequestedRef.current) return
    const manualInstall = state.status === 'downloaded' && state.installMode === 'manual'
    installRequestedRef.current = true
    setInstalling(true)
    const request = window.matouDesktop?.installAppUpdate?.()
    if (manualInstall) {
      void request?.finally(() => {
        installRequestedRef.current = false
        setInstalling(false)
      })
    }
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

    {open && <section role="dialog" aria-label={m.popover} className="app-update-popover">
      <UpdateHeader state={state} onClose={() => setOpen(false)} />
      <div className="app-update-popover__body">
        {state.status === 'idle' && <p>{m.currentVersion(state.currentVersion || '—')}</p>}
        {state.status === 'checking' && <p>{state.retryAttempt
          ? m.retrying(state.retryAttempt, state.maxRetryAttempts ?? state.retryAttempt)
          : m.checking}</p>}
        {state.status === 'not-available' && <p>{m.upToDate(state.currentVersion)}</p>}
        {state.status === 'error' && <div className="app-update-error">
          <span>{friendlyError(state, m)}</span>
        </div>}
        {state.status === 'available' && state.installMode === 'manual' &&
          <div className="app-update-waiting">{m.manualDmgNotice}</div>}
        {isReleaseState(state) && state.status !== 'downloading' && state.releaseNotes.length > 0 &&
          <ul className="app-update-notes">{state.releaseNotes.slice(0, 3).map((note) => <li key={note}>{note}</li>)}</ul>}
        {state.status === 'downloading' && <DownloadProgress state={state} />}
        {state.status === 'downloaded' && state.installMode === 'automatic' && activeSessionCount > 0 && <div className="app-update-session-warning">
          <i /><span><strong>{m.activeSessions(activeSessionCount)}</strong>
            <small>{m.idleUpdateKeepsState}</small></span>
        </div>}
        {waitingForIdle && state.status === 'downloaded' && state.installMode === 'automatic' &&
          <div className="app-update-waiting">{m.idleUpdateScheduled}</div>}
      </div>
      <div className="app-update-popover__actions">
        {(state.status === 'idle' || state.status === 'not-available') &&
          <button className="is-primary" onClick={() => void window.matouDesktop?.checkForAppUpdates?.()}>
            {m.actions.check}
          </button>}
        {state.status === 'error' && state.manualDownloadUrl && <>
          <button className="is-primary" onClick={() => void window.matouDesktop?.downloadAppUpdate?.()}>{m.actions.downloadDmg}</button>
          <button className="is-quiet" onClick={() => void window.matouDesktop?.checkForAppUpdates?.()}>{m.actions.recheck}</button>
        </>}
        {state.status === 'error' && !state.manualDownloadUrl &&
          <button className="is-primary" onClick={() => void window.matouDesktop?.checkForAppUpdates?.()}>{m.actions.recheck}</button>}
        {state.status === 'available' && <>
          <button className="is-primary" onClick={() => void window.matouDesktop?.downloadAppUpdate?.()}>
            {state.installMode === 'manual' ? m.actions.download : m.actions.downloadInBackground}
          </button>
          <button className="is-quiet" onClick={() => setOpen(false)}>{m.actions.remindLater}</button>
        </>}
        {state.status === 'downloading' && <button className="is-quiet" onClick={() => setOpen(false)}>{m.actions.keepInBackground}</button>}
        {state.status === 'downloaded' && state.installMode === 'manual' && <>
          <button className="is-primary" disabled={installing} onClick={installNow}>
            {installing ? m.actions.opening : m.actions.openDmg}
          </button>
          <button className="is-quiet" onClick={() => setOpen(false)}>{m.actions.installLater}</button>
        </>}
        {state.status === 'downloaded' && state.installMode === 'automatic' && activeSessionCount > 0 && <>
          <button className={waitingForIdle ? '' : 'is-primary'} disabled={installing}
            onClick={() => setWaitingForIdle((waiting) => !waiting)}>
            {waitingForIdle ? m.actions.cancelIdleUpdate : m.actions.updateWhenIdle}
          </button>
          <div className="app-update-action-row">
            <button disabled={installing} onClick={installNow}>{m.actions.restartNow}</button>
            <button onClick={() => setOpen(false)}>{m.actions.installOnQuit}</button>
          </div>
        </>}
        {state.status === 'downloaded' && state.installMode === 'automatic' && activeSessionCount === 0 && <>
          <button className="is-primary" disabled={installing} onClick={installNow}>{installing ? m.actions.preparing : m.actions.restart}</button>
          <button className="is-quiet" onClick={() => setOpen(false)}>{m.actions.installOnQuit}</button>
        </>}
      </div>
    </section>}

    {showUpdatedToast && <div className="app-update-toast" role="status">
      <span>✓</span><div><strong>{m.updatedTo(state.currentVersion)}</strong><small>{m.updatedHint}</small></div>
    </div>}
  </div>
}

function UpdateHeader({ state, onClose }: { state: AppUpdateState; onClose: () => void }) {
  const m = useMessages().updates
  const locale = useLocale()
  const title = state.status === 'available' ? m.header.available(state.version)
    : state.status === 'downloading' ? m.header.downloading
    : state.status === 'downloaded' ? state.installMode === 'manual' ? m.header.dmgReady : m.header.ready
    : state.status === 'checking' ? m.header.checking
    : state.status === 'error' ? errorTitle(state.errorStage, m)
    : state.status === 'not-available' ? m.header.upToDate
    : m.popover
  const subtitle = isReleaseState(state)
    ? [state.status === 'available' ? m.header.stableChannel : `Matou ${state.version}`, state.sizeBytes ? formatBytes(state.sizeBytes) : '', formatDate(state.releaseDate, locale)].filter(Boolean).join(' · ')
    : m.currentVersion(state.currentVersion || '—')
  return <header className="app-update-popover__header">
    <span className="app-update-release-icon"><DownloadIcon /></span>
    <span><strong>{title}</strong><small>{subtitle}</small></span>
    <button type="button" aria-label={m.header.close} onClick={onClose}>×</button>
  </header>
}

function DownloadProgress({ state }: { state: Extract<AppUpdateState, { status: 'downloading' }> }) {
  const m = useMessages().updates
  const progress = state.progress
  return <div className="app-update-download">
    <div><span>{formatBytes(progress.transferredBytes)} / {formatBytes(progress.totalBytes)}</span>
      <span>{Math.round(progress.percent)}%{progress.remainingSeconds === undefined ? '' : ` · ${m.remainingSeconds(progress.remainingSeconds)}`}</span></div>
    <span className="app-update-download__track"><i style={{ width: `${Math.max(0, Math.min(100, progress.percent))}%` }} /></span>
    <small>{m.downloadRate(formatBytes(progress.bytesPerSecond))}</small>
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

function updateButtonLabel(state: AppUpdateState, m: UpdateMessages): string {
  if (state.status === 'checking') return m.buttonLabel.checking
  if (state.status === 'available') return m.buttonLabel.available(state.version)
  if (state.status === 'downloading') return m.buttonLabel.downloading(Math.round(state.progress.percent))
  if (state.status === 'downloaded') return state.installMode === 'manual'
    ? m.buttonLabel.awaitingInstaller
    : m.buttonLabel.awaitingInstall
  if (state.status === 'error') return m.buttonLabel.error(errorTitle(state.errorStage, m))
  return m.buttonLabel.idle
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB'
  return `${(bytes / 1_000_000).toFixed(1)} MB`
}

function formatDate(value: string | undefined, locale: Locale): string {
  if (!value) return ''
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : new Intl.DateTimeFormat(locale, { month: 'numeric', day: 'numeric' }).format(date)
}

function errorTitle(stage: AppUpdateErrorStage, m: UpdateMessages): string {
  if (stage === 'download') return m.errorTitle.download
  if (stage === 'verify') return m.errorTitle.verify
  if (stage === 'install') return m.errorTitle.install
  return m.errorTitle.check
}

function friendlyError(state: Extract<AppUpdateState, { status: 'error' }>, m: UpdateMessages): string {
  if (state.errorStage === 'verify') {
    return m.error.signatureMissing
  }
  if (state.errorStage === 'install') return m.error.installFailed
  if (/ENOTFOUND|EAI_AGAIN|ERR_NAME_NOT_RESOLVED|\bDNS\b|getaddrinfo/i.test(state.errorMessage)) {
    return m.error.dnsFailed
  }
  if (/ETIMEDOUT|ERR_TIMED_OUT|timed?\s*out/i.test(state.errorMessage)) {
    return m.error.timedOut
  }
  if (/ENETUNREACH|ERR_INTERNET_DISCONNECTED|network is unreachable/i.test(state.errorMessage)) {
    return m.error.offline
  }
  if (/ECONNREFUSED|ERR_CONNECTION_REFUSED/i.test(state.errorMessage)) {
    return m.error.connectionRefused
  }
  if (/ECONNRESET|ERR_CONNECTION_RESET|socket hang up/i.test(state.errorMessage)) {
    return m.error.connectionReset
  }
  if (/CERT_|SSL|TLS|certificate/i.test(state.errorMessage)) {
    return m.error.tlsFailed
  }
  const httpStatus = state.errorMessage.match(/(?:HTTP|status(?: code)?)\D*(\d{3})/i)?.[1]
  if (httpStatus) return m.error.httpStatus(httpStatus)
  if (/stable-mac\.yml|latest[^\s]*\.yml|YAML|parse/i.test(state.errorMessage)) {
    return m.error.manifestInvalid
  }
  if (/network|server|fetch|ECONN|timeout/i.test(state.errorMessage)) return m.error.networkUnavailable
  if (state.errorStage === 'download') return m.error.downloadInterrupted
  return m.error.unknown
}
