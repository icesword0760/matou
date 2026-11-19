import { useCallback, useRef, useState, type RefObject } from 'react'
import { createPortal } from 'react-dom'
import type { EgressStatusWire } from '@matou/contracts'
import { useLocale } from '../i18n/LocaleProvider'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { chipCopy, globalChipCopy, type ChipCopy } from './egress-copy'
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

interface ChipSelection {
  readonly copy: ChipCopy
  /** True for the always-available network-check entry, false for a monitored family. */
  readonly isGlobal: boolean
}

function selectChipCopy(
  status: EgressStatusWire | undefined,
  sessionId: string,
  zh: boolean
): ChipSelection {
  const family = sessionEgressFamily(status, sessionId)
  if (!status || !family) {
    return { copy: globalChipCopy(status, zh), isGlobal: true }
  }
  const familyState = findEgressFamily(status, family)
  return {
    copy: chipCopy(
      status.state,
      family,
      familyState,
      status.checks,
      status.checkedAt,
      zh
    ),
    isGlobal: false
  }
}

function equalChipCopy(
  a: ChipSelection | null,
  b: ChipSelection | null
): boolean {
  if (a === b) return true
  if (!a || !b) return false
  return (
    a.isGlobal === b.isGlobal &&
    a.copy.label === b.copy.label &&
    a.copy.tone === b.copy.tone
  )
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

/** Compact HUD exit-status chip for every session; monitored cards show family copy, others a global network-check entry. */
export function EgressChip({ sessionId }: EgressChipProps) {
  const client = useRuntimeClient()
  const zh = useLocale() === 'zh-CN'
  const selector = useCallback(
    (status: EgressStatusWire | undefined) => selectChipCopy(status, sessionId, zh),
    [sessionId, zh]
  )
  const selection = useEgressSelector(selector, equalChipCopy)
  // Keep actions ref stable without pulling full status updates.
  useEgressActions()
  const [open, setOpen] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)

  // Hosts without an egress bridge cannot serve the panel, so leave the HUD empty there.
  const hasEgressBridge = typeof client?.subscribeEgressStatus === 'function'

  if (!hasEgressBridge) return null

  const { copy, isGlobal } = selection

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        className={`status-field egress-chip is-${copy.tone} is-clickable${open ? ' is-open' : ''}`}
        data-tone={copy.tone}
        aria-label={isGlobal ? (zh ? '网络检查' : 'Network check') : undefined}
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
