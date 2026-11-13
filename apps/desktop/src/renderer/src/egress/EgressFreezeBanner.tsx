import { useEffect, useState } from 'react'
import type { EgressFamily, EgressStatusWire } from '@matou/contracts'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { useLocale } from '../i18n/LocaleProvider'
import {
  formatBannerFreezeMessage,
  selectSessionEgressBannerView,
  type SessionEgressBannerView
} from './use-egress-status'
import './egress.css'

export interface EgressFreezeBannerProps {
  readonly sessionId: string
  readonly banner?: SessionEgressBannerView
  readonly status?: EgressStatusWire
  readonly onRecheck: () => void | Promise<void>
  readonly onOverride: (family: EgressFamily) => void | Promise<void>
}

const RESUMED_BANNER_DURATION_MS = 3000

/** Card-top banner displayed when the session's provider egress is frozen or recently resumed. */
export function EgressFreezeBanner({
  sessionId,
  banner,
  status,
  onRecheck,
  onOverride
}: EgressFreezeBannerProps) {
  const zh = useLocale() === 'zh-CN'
  const [confirmOpen, setConfirmOpen] = useState(false)
  const [rechecking, setRechecking] = useState(false)
  const [overriding, setOverriding] = useState(false)
  const [dismissedTransitionId, setDismissedTransitionId] = useState<string | null>(null)

  const view = banner ?? selectSessionEgressBannerView(status, sessionId)
  const resumedId = view.kind === 'resumed' ? view.transitionId : null

  useEffect(() => {
    if (!resumedId || resumedId === dismissedTransitionId) return
    const timer = window.setTimeout(() => {
      setDismissedTransitionId(resumedId)
    }, RESUMED_BANNER_DURATION_MS)
    return () => window.clearTimeout(timer)
  }, [dismissedTransitionId, resumedId])

  if (view.kind === 'frozen') {
    const message = formatBannerFreezeMessage(view, zh)

    return (
      <>
        <div
          className="egress-freeze-banner"
          role="status"
          data-violation={view.violation}
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
              void Promise.resolve(onOverride(view.family)).finally(() => setOverriding(false))
            }}
          />
        )}
      </>
    )
  }

  if (view.kind === 'resumed' && view.transitionId !== dismissedTransitionId) {
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
