import { useEffect, useState } from 'react'
import type { EgressFamily, EgressStatusWire } from '@matou/contracts'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { useLocale } from '../i18n/LocaleProvider'
import { countryName } from './egress-copy'
import {
  findEgressFamily,
  isSessionEgressFrozen,
  sessionEgressFamily
} from './use-egress-status'
import './egress.css'

export interface EgressFreezeBannerProps {
  readonly sessionId: string
  readonly status: EgressStatusWire | undefined
  readonly onRecheck: () => void | Promise<void>
  readonly onOverride: (family: EgressFamily) => void | Promise<void>
}

const RESUMED_BANNER_DURATION_MS = 3000

/** Card-top banner displayed when the session's provider egress is frozen or recently resumed. */
export function EgressFreezeBanner({
  sessionId,
  status,
  onRecheck,
  onOverride
}: EgressFreezeBannerProps) {
  const zh = useLocale() === 'zh-CN'
  const [confirmOpen, setConfirmOpen] = useState(false)
  const [rechecking, setRechecking] = useState(false)
  const [overriding, setOverriding] = useState(false)
  const [dismissedTransitionId, setDismissedTransitionId] = useState<string | null>(null)

  const family = sessionEgressFamily(status, sessionId)
  const familyEntry = findEgressFamily(status, family)
  const frozen = isSessionEgressFrozen(status, sessionId)

  const resumedTransition =
    !frozen &&
    family !== undefined &&
    status?.lastTransition?.kind === 'resumed' &&
    status.lastTransition.family === family
      ? status.lastTransition
      : undefined
  const resumedId = resumedTransition?.id ?? null

  useEffect(() => {
    if (!resumedId || resumedId === dismissedTransitionId) return
    const timer = window.setTimeout(() => {
      setDismissedTransitionId(resumedId)
    }, RESUMED_BANNER_DURATION_MS)
    return () => window.clearTimeout(timer)
  }, [dismissedTransitionId, resumedId])

  if (!status || !family || !familyEntry) return null

  if (frozen) {
    const ruleCheck = status.checks.find(
      (check) => check.family === family && check.id === 'exit-rule'
    )
    const baselineCode = familyEntry.baseline?.country ?? ruleCheck?.facts?.baselineCountry
    const currentCode = familyEntry.exit?.country ?? ruleCheck?.facts?.currentCountry
    const baseline = countryName(baselineCode, zh)
    const current = countryName(currentCode, zh)

    const message =
      familyEntry.violation === 'rule'
        ? zh
          ? `出口与规则不符（${baseline} → ${current}），已暂停。网络恢复后自动继续。`
          : `Exit doesn't match your rule (${baseline} → ${current}). Paused; resumes automatically when the network recovers.`
        : zh
          ? '出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。'
          : 'Exit problem (mainland China IP detected). Paused; resumes automatically when the network recovers.'

    return (
      <>
        <div
          className="egress-freeze-banner"
          role="status"
          data-violation={familyEntry.violation ?? 'mainland'}
          onClick={(event) => event.stopPropagation()}
        >
          <span className="egress-freeze-banner__message">{message}</span>
          <div className="egress-freeze-banner__actions">
            <button
              type="button"
              disabled={rechecking || overriding}
              onClick={() => {
                setRechecking(true)
                void Promise.resolve(onRecheck()).finally(() => setRechecking(false))
              }}
            >
              {zh ? '重新检查' : 'Recheck'}
            </button>
            <button
              type="button"
              disabled={overriding}
              onClick={() => setConfirmOpen(true)}
            >
              {zh ? '仍要继续' : 'Continue anyway'}
            </button>
          </div>
        </div>
        {confirmOpen && (
          <ConfirmDialog
            title={zh ? '仍要继续？' : 'Continue anyway?'}
            body={
              zh
                ? '出口仍然异常，继续后 agent 会用当前出口访问服务商。出口再次变化前不会再暂停。'
                : "The exit is still wrong. Agents will reach the provider through it, and they won't be paused again until the exit changes."
            }
            confirmLabel={zh ? '继续' : 'Continue'}
            cancelLabel={zh ? '取消' : 'Cancel'}
            confirmTone="danger"
            onCancel={() => setConfirmOpen(false)}
            onConfirm={() => {
              setConfirmOpen(false)
              setOverriding(true)
              void Promise.resolve(onOverride(family)).finally(() => setOverriding(false))
            }}
          />
        )}
      </>
    )
  }

  if (resumedTransition && resumedTransition.id !== dismissedTransitionId) {
    return (
      <div className="egress-freeze-banner is-resumed" role="status">
        <span className="egress-freeze-banner__message">
          {zh ? '出口已恢复，已自动继续' : 'Exit restored; resumed automatically'}
        </span>
      </div>
    )
  }

  return null
}
