import { useCallback, useRef, useState, type RefObject } from 'react'
import { createPortal } from 'react-dom'
import type { EgressStatusWire } from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import { chipCopy, type ChipCopy } from './egress-copy'
import { EgressPanel } from './EgressPanel'
import {
  findEgressFamily,
  sessionEgressFamily,
  useEgressActions,
  useEgressSelector,
  useEgressStatus
} from './use-egress-status'
import './egress.css'

export interface EgressChipProps {
  readonly sessionId: string
}

function selectChipCopy(
  status: EgressStatusWire | undefined,
  sessionId: string,
  zh: boolean
): ChipCopy | null {
  if (!status) return null
  const family = sessionEgressFamily(status, sessionId)
  if (!family) return null
  const familyState = findEgressFamily(status, family)
  return chipCopy(
    status.state,
    family,
    familyState,
    status.checks,
    status.checkedAt,
    zh
  )
}

function equalChipCopy(a: ChipCopy | null, b: ChipCopy | null): boolean {
  if (a === b) return true
  if (!a || !b) return false
  return a.label === b.label && a.tone === b.tone
}

function ConnectedEgressPanel({
  anchorRef,
  onClose
}: {
  anchorRef: RefObject<HTMLElement | null>
  onClose: () => void
}) {
  const { status, recheck, setRule, acceptCurrent, setProtection } = useEgressStatus()
  if (!status) return null
  return (
    <EgressPanel
      status={status}
      anchorRef={anchorRef}
      onClose={onClose}
      onRecheck={recheck}
      onSetRule={setRule}
      onAcceptCurrent={acceptCurrent}
      onSetProtection={setProtection}
    />
  )
}

/** Compact HUD exit-status chip for official Claude and Codex cards; hidden for shell/third-party cards. */
export function EgressChip({ sessionId }: EgressChipProps) {
  const zh = useLocale() === 'zh-CN'
  const selector = useCallback(
    (status: EgressStatusWire | undefined) => selectChipCopy(status, sessionId, zh),
    [sessionId, zh]
  )
  const copy = useEgressSelector(selector, equalChipCopy)
  // Keep actions ref stable without pulling full status updates.
  useEgressActions()
  const [open, setOpen] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)

  if (!copy) return null

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
          <ConnectedEgressPanel
            anchorRef={triggerRef}
            onClose={() => setOpen(false)}
          />,
          document.body
        )}
    </>
  )
}
