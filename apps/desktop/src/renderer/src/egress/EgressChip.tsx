import { useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useLocale } from '../i18n/LocaleProvider'
import { chipCopy } from './egress-copy'
import { EgressPanel } from './EgressPanel'
import {
  findEgressFamily,
  sessionEgressFamily,
  useEgressStatus
} from './use-egress-status'
import './egress.css'

export interface EgressChipProps {
  readonly sessionId: string
}

/** Compact HUD exit-status chip for official Claude and Codex cards; hidden for shell/third-party cards. */
export function EgressChip({ sessionId }: EgressChipProps) {
  const zh = useLocale() === 'zh-CN'
  const { status, recheck, setRule, acceptCurrent, setProtection } = useEgressStatus()
  const [open, setOpen] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)

  if (!status) return null
  const family = sessionEgressFamily(status, sessionId)
  if (!family) return null

  const familyState = findEgressFamily(status, family)
  const copy = chipCopy(
    status.state,
    family,
    familyState,
    status.checks,
    status.checkedAt,
    zh
  )

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className={`status-field egress-chip is-${copy.tone} is-clickable${open ? ' is-open' : ''}`}
        data-tone={copy.tone}
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setOpen((current) => !current)}
      >
        <span className="egress-chip__dot" aria-hidden="true" />
        <span className="egress-chip__label">{copy.label}</span>
      </button>
      {open &&
        createPortal(
          <EgressPanel
            status={status}
            anchorRef={triggerRef}
            onClose={() => setOpen(false)}
            onRecheck={recheck}
            onSetRule={setRule}
            onAcceptCurrent={acceptCurrent}
            onSetProtection={setProtection}
          />,
          document.body
        )}
    </>
  )
}
