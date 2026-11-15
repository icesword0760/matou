import { useEffect, useId, useRef, useState, type RefObject } from 'react'
import {
  EGRESS_FAMILIES,
  type EgressCheckWire,
  type EgressExit,
  type EgressFamily,
  type EgressStatusWire
} from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import {
  buildEgressReport,
  checkCopy,
  familyHeading,
  formatCheckedTime,
  ruleLabel
} from './egress-copy'
import { findEgressFamily, type EgressRuleMode } from './use-egress-status'

export const REFERENCE_CHECK_URLS = {
  ipQuality: 'https://ip.net.coffee/',
  browserEnv: 'https://fuck-claude.vercel.app/'
} as const

export interface EgressPanelProps {
  readonly status: EgressStatusWire
  readonly anchorRef?: RefObject<HTMLElement | null>
  readonly onClose: () => void
  readonly onRecheck: () => void | Promise<void>
  readonly onSetRule: (mode: EgressRuleMode) => void | Promise<void>
  readonly onAcceptCurrent: (family: EgressFamily) => void | Promise<void>
  readonly onSetProtection: (enabled: boolean) => void | Promise<void>
}

function resultGlyph(result: EgressCheckWire['result']): string {
  switch (result) {
    case 'pass':
      return '✓'
    case 'warn':
      return '!'
    case 'fail':
      return '✕'
    case 'unknown':
      return '?'
  }
}

async function copyTextToClipboard(text: string): Promise<boolean> {
  if (window.matouDesktop?.writeClipboardText) {
    await window.matouDesktop.writeClipboardText(text)
    return true
  }
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text)
    return true
  }
  return false
}

function openReferenceUrl(url: string): void {
  void window.matouDesktop?.openExternal?.(url)
}

type DesktopClient = 'claude' | 'chatgpt'

interface SafeBrowserStatus {
  installed: boolean
  version: string | null
}

const DESKTOP_CLIENT_POLL_MS = 5000
const UPDATE_FEEDBACK_MS = 3000
const DESKTOP_CLIENT_ORDER: readonly DesktopClient[] = ['claude', 'chatgpt']

function runBridgeAction(action: (() => Promise<void>) | undefined): void {
  if (!action) return
  try {
    void action().catch(() => {})
  } catch {
    // The desktop host can disappear mid-click; the card stays a passive status entry.
  }
}

type UpdateFeedback =
  | { kind: 'latest' }
  | { kind: 'failed' }
  | { kind: 'available'; latest: string }

function updateFeedbackText(feedback: UpdateFeedback, zh: boolean): string {
  switch (feedback.kind) {
    case 'latest':
      return zh ? '已是最新' : 'Up to date'
    case 'failed':
      return zh ? '检查失败' : 'Check failed'
    case 'available':
      return zh ? `有新版本 · v${feedback.latest}` : `New version · v${feedback.latest}`
  }
}

function SafeBrowserCard({ zh }: { zh: boolean }) {
  const [status, setStatus] = useState<SafeBrowserStatus | null>(null)
  const [checkingUpdate, setCheckingUpdate] = useState(false)
  const [feedback, setFeedback] = useState<UpdateFeedback | null>(null)
  const feedbackTimer = useRef<number | undefined>(undefined)
  const mountedRef = useRef(true)

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      if (feedbackTimer.current !== undefined) window.clearTimeout(feedbackTimer.current)
    }
  }, [])

  useEffect(() => {
    let active = true
    const readStatus = window.matouDesktop?.safeBrowserStatus
    if (!readStatus) {
      setStatus({ installed: false, version: null })
      return
    }
    void readStatus()
      .then((value) => {
        if (active) setStatus(value)
      })
      .catch(() => {
        if (active) setStatus({ installed: false, version: null })
      })
    return () => {
      active = false
    }
  }, [])

  const showFeedback = (next: UpdateFeedback) => {
    if (feedbackTimer.current !== undefined) window.clearTimeout(feedbackTimer.current)
    setFeedback(next)
    feedbackTimer.current = window.setTimeout(() => {
      feedbackTimer.current = undefined
      setFeedback(null)
    }, UPDATE_FEEDBACK_MS)
  }

  const handleCheckUpdate = () => {
    if (checkingUpdate) return
    if (feedbackTimer.current !== undefined) {
      window.clearTimeout(feedbackTimer.current)
      feedbackTimer.current = undefined
    }
    setFeedback(null)
    setCheckingUpdate(true)
    const check = window.matouDesktop?.safeBrowserCheckUpdate
    let request: Promise<{ latest: string | null; failed: boolean }>
    try {
      request = check ? check() : Promise.reject(new Error('unavailable'))
    } catch {
      request = Promise.reject(new Error('unavailable'))
    }
    void request
      .then((result) => {
        if (!mountedRef.current) return
        if (!result || result.failed) {
          showFeedback({ kind: 'failed' })
        } else if (result.latest) {
          showFeedback({ kind: 'available', latest: result.latest })
        } else {
          showFeedback({ kind: 'latest' })
        }
      })
      .catch(() => {
        if (mountedRef.current) showFeedback({ kind: 'failed' })
      })
      .finally(() => {
        if (mountedRef.current) setCheckingUpdate(false)
      })
  }

  const handleOpen = () => {
    runBridgeAction(window.matouDesktop?.safeBrowserOpen)
  }

  const handleGetInstaller = () => {
    runBridgeAction(window.matouDesktop?.openSafeBrowserReleases)
  }

  const installed = status?.installed === true
  if (!status) return null

  const stateText = installed
    ? status.version
      ? zh
        ? `已安装 · ${status.version}`
        : `Installed · ${status.version}`
      : zh
        ? '已安装'
        : 'Installed'
    : zh
      ? '未安装'
      : 'Not installed'

  return (
    <section className="egress-safe-browser" aria-label={zh ? '安全浏览器' : 'Safe browser'}>
      <div className="egress-safe-browser__main">
        <div className="egress-safe-browser__title">{zh ? '安全浏览器' : 'Safe browser'}</div>
        <div className="egress-safe-browser__state">{stateText}</div>
        <div className="egress-safe-browser__hint" role={feedback ? 'status' : undefined}>
          {feedback
            ? updateFeedbackText(feedback, zh)
            : zh
              ? '登录 Claude / ChatGPT / Antigravity 时使用'
              : 'Used when signing in to Claude / ChatGPT / Antigravity'}
        </div>
      </div>
      <div className="egress-safe-browser__actions">
        {installed ? (
          <>
            <button
              type="button"
              className="egress-btn"
              disabled={checkingUpdate}
              onClick={handleCheckUpdate}
            >
              {zh ? '检查更新' : 'Check for updates'}
            </button>
            <button type="button" className="egress-btn" onClick={handleOpen}>
              {zh ? '打开' : 'Open'}
            </button>
          </>
        ) : (
          <button type="button" className="egress-btn" onClick={handleGetInstaller}>
            {zh ? '获取安装包' : 'Get installer'}
          </button>
        )}
      </div>
    </section>
  )
}

function DesktopClientNotices({ zh }: { zh: boolean }) {
  const [clients, setClients] = useState<readonly DesktopClient[]>([])

  useEffect(() => {
    let active = true
    let timer: number | undefined
    let inFlight = false
    const detect = window.matouDesktop?.runningDesktopClients
    if (!detect) return

    const poll = () => {
      if (!active || inFlight) return
      inFlight = true
      void detect()
        .then((found) => {
          if (!active) return
          const valid = Array.isArray(found)
            ? found.filter((client): client is DesktopClient => DESKTOP_CLIENT_ORDER.includes(client))
            : []
          setClients(valid)
        })
        .catch(() => {
          if (active) setClients([])
        })
        .finally(() => {
          inFlight = false
        })
    }
    poll()
    timer = window.setInterval(poll, DESKTOP_CLIENT_POLL_MS)

    return () => {
      active = false
      if (timer !== undefined) window.clearInterval(timer)
    }
  }, [])

  const visible = DESKTOP_CLIENT_ORDER.filter((client) => clients.includes(client))
  if (visible.length === 0) return null

  return (
    <div className="egress-client-notices" role="status">
      {visible.map((client) => (
        <p className="egress-client-notice" key={client}>
          <span className="egress-client-notice__mark" aria-hidden="true">
            !
          </span>
          <span>
            {client === 'claude'
              ? zh
                ? '检测到 Claude Desktop 正在运行，它的出口不在保护范围'
                : 'Claude Desktop is running; its exit is not protected.'
              : zh
                ? '检测到 ChatGPT 桌面版正在运行，它的出口不在保护范围'
                : 'ChatGPT desktop is running; its exit is not protected.'}
          </span>
        </p>
      ))}
    </div>
  )
}

function CheckRow({
  check,
  zh,
  familyExit
}: {
  check: EgressCheckWire
  zh: boolean
  familyExit?: EgressExit
}) {
  const copy = checkCopy(check, zh, familyExit)
  return (
    <li className={`egress-check is-${check.result}`}>
      <span className={`egress-check__badge is-${check.result}`} aria-hidden="true">
        {resultGlyph(check.result)}
      </span>
      <div className="egress-check__body">
        <div className="egress-check__title">{copy.title}</div>
        {copy.detail && <div className="egress-check__detail">{copy.detail}</div>}
      </div>
    </li>
  )
}

/** Popover panel showing per-provider and host egress diagnostics plus rules and protection controls. */
export function EgressPanel({
  status,
  anchorRef,
  onClose,
  onRecheck,
  onSetRule,
  onAcceptCurrent,
  onSetProtection
}: EgressPanelProps) {
  const zh = useLocale() === 'zh-CN'
  const panelRef = useRef<HTMLDivElement>(null)
  const titleId = useId()
  const ruleGroupName = useId()
  const [rechecking, setRechecking] = useState(false)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null
      if (!target) return
      if (panelRef.current?.contains(target)) return
      if (anchorRef?.current?.contains(target)) return
      onClose()
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation()
        onClose()
      }
    }
    document.addEventListener('pointerdown', onPointerDown)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('pointerdown', onPointerDown)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [anchorRef, onClose])

  useEffect(() => {
    if (!copied) return
    const timer = window.setTimeout(() => setCopied(false), 2000)
    return () => window.clearTimeout(timer)
  }, [copied])

  const checkedText =
    status.checkedAt !== undefined
      ? zh
        ? `检测于 ${formatCheckedTime(status.checkedAt)}`
        : `Checked at ${formatCheckedTime(status.checkedAt)}`
      : zh
        ? '出口检查中…'
        : 'Checking exit…'

  const handleRecheck = () => {
    setRechecking(true)
    void Promise.resolve(onRecheck()).finally(() => setRechecking(false))
  }

  const handleCopyReport = () => {
    const report = buildEgressReport(status, zh)
    void copyTextToClipboard(report)
      .then((ok) => {
        if (ok) setCopied(true)
      })
      .catch(() => {})
  }

  const familyGroups = EGRESS_FAMILIES.map((family) => ({
    family,
    heading: familyHeading(family, zh)
  }))

  const globalChecks = status.checks.filter((check) => check.family === undefined)

  return (
    <div
      ref={panelRef}
      className="egress-panel"
      role="dialog"
      aria-labelledby={titleId}
    >
      <header className="egress-panel__header">
        <div className="egress-panel__heading">
          <h2 id={titleId}>{zh ? '网络检查' : 'Network check'}</h2>
          <span className="egress-panel__checked-at">{checkedText}</span>
        </div>
        <div className="egress-panel__header-actions">
          <button
            type="button"
            className="egress-btn"
            disabled={rechecking}
            onClick={handleRecheck}
          >
            {zh ? '重新检查' : 'Recheck'}
          </button>
          <button
            type="button"
            className="egress-panel__close"
            aria-label={zh ? '关闭' : 'Close'}
            onClick={onClose}
          >
            ×
          </button>
        </div>
      </header>

      <div className="egress-panel__scroll">
        {familyGroups.map(({ family, heading }) => {
          const familyEntry = findEgressFamily(status, family)
          const checks = status.checks.filter((check) => check.family === family)
          if (!familyEntry && checks.length === 0) return null
          return (
            <section className="egress-section" key={family} aria-label={heading}>
              <div className="egress-section__header">
                <h3>{heading}</h3>
                {familyEntry?.violation === 'rule' && (
                  <button
                    type="button"
                    className="egress-btn egress-btn--subtle"
                    onClick={() => void onAcceptCurrent(family)}
                  >
                    {zh ? '以当前出口为准' : 'Use current exit'}
                  </button>
                )}
              </div>
              <ul className="egress-check-list">
                {checks.map((check) => (
                  <CheckRow
                    key={`${family}:${check.id}`}
                    check={check}
                    zh={zh}
                    {...(familyEntry?.exit ? { familyExit: familyEntry.exit } : {})}
                  />
                ))}
              </ul>
            </section>
          )
        })}

        {globalChecks.length > 0 && (
          <section className="egress-section" aria-label={zh ? '本机' : 'This Mac'}>
            <div className="egress-section__header">
              <h3>{zh ? '本机' : 'This Mac'}</h3>
            </div>
            <ul className="egress-check-list">
              {globalChecks.map((check) => (
                <CheckRow key={`global:${check.id}`} check={check} zh={zh} />
              ))}
            </ul>
          </section>
        )}

        <DesktopClientNotices zh={zh} />

        <section className="egress-controls">
          <fieldset className="egress-rule-fieldset">
            <legend>{zh ? '出口规则' : 'Exit rule'}</legend>
            <div className="egress-rule-options" role="radiogroup">
              {(['country', 'ip'] as const).map((mode) => (
                <label className="egress-radio" key={mode}>
                  <input
                    type="radio"
                    name={ruleGroupName}
                    value={mode}
                    checked={status.ruleMode === mode}
                    onChange={() => void onSetRule(mode)}
                  />
                  <span>{ruleLabel(mode, zh)}</span>
                </label>
              ))}
            </div>
          </fieldset>

          <label className="egress-toggle">
            <input
              type="checkbox"
              checked={status.protectionEnabled}
              onChange={(event) => void onSetProtection(event.target.checked)}
            />
            <span>
              {zh
                ? '出口保护：出口异常时暂停 Claude / Codex 会话'
                : 'Exit protection: pause Claude / Codex sessions when the exit is wrong'}
            </span>
          </label>
        </section>

        <SafeBrowserCard zh={zh} />

        <section className="egress-reference">
          <div className="egress-reference__title">
            {zh ? '参考检查（不影响状态）' : 'Reference checks (do not affect status)'}
          </div>
          <div className="egress-reference__links">
            <button
              type="button"
              className="egress-link-btn"
              onClick={() => openReferenceUrl(REFERENCE_CHECK_URLS.ipQuality)}
            >
              {zh ? 'IP 质量' : 'IP quality'} ↗
            </button>
            <button
              type="button"
              className="egress-link-btn"
              onClick={() => openReferenceUrl(REFERENCE_CHECK_URLS.browserEnv)}
            >
              {zh ? '浏览器环境' : 'Browser environment'} ↗
            </button>
          </div>
          <p className="egress-reference__note">
            {zh
              ? '点击后才访问第三方站点，站点会收到你的出口 IP。'
              : 'Sites are only contacted when you click, and they will see your exit IP.'}
          </p>
        </section>
      </div>

      <footer className="egress-panel__footer">
        <span className="egress-panel__sampling-note">
          {zh
            ? '检测为每分钟抽查，无法保证每条连接都使用同一出口。'
            : 'Checks are sampled every minute and cannot guarantee every connection uses the same exit.'}
        </span>
        <div className="egress-panel__copy-wrap">
          {copied && (
            <span className="egress-copied-hint" role="status">
              {zh ? '已复制' : 'Copied'}
            </span>
          )}
          <button
            type="button"
            className="egress-btn"
            onClick={handleCopyReport}
          >
            {zh ? '复制报告' : 'Copy report'}
          </button>
        </div>
      </footer>
    </div>
  )
}
