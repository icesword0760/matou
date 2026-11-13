import { useEffect, useId, useRef, useState, type RefObject } from 'react'
import type {
  EgressCheckWire,
  EgressExit,
  EgressFamily,
  EgressStatusWire
} from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import {
  buildEgressReport,
  checkCopy,
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

  const familyGroups: readonly { family: EgressFamily; heading: string }[] = [
    {
      family: 'anthropic',
      heading: zh ? 'Claude（Anthropic）' : 'Claude (Anthropic)'
    },
    {
      family: 'openai',
      heading: zh ? 'Codex（OpenAI）' : 'Codex (OpenAI)'
    }
  ]

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
