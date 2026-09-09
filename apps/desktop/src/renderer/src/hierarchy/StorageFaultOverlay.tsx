import { useRef, useState } from 'react'

import type { StorageFaultCode } from '@matou/contracts'

import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'

import './storage-fault-overlay.css'

export interface TerminalStorageFault {
  code: StorageFaultCode
  retainedBytes: number
  message?: string
}

export function StorageFaultOverlay(props: {
  sessionTitle: string
  fault: TerminalStorageFault
  onRetry(): Promise<unknown> | unknown
  onEnd(): Promise<unknown> | unknown
}) {
  const m = useMessages().hierarchyTerminal.storageFault
  const { sessionTitle, fault, onRetry, onEnd } = props
  const [pendingAction, setPendingAction] = useState<'retry' | 'end' | null>(null)
  const [error, setError] = useState('')
  const pendingRef = useRef(false)

  const run = async (action: 'retry' | 'end', operation: () => Promise<unknown> | unknown) => {
    if (pendingRef.current) return
    pendingRef.current = true
    setPendingAction(action)
    setError('')
    try {
      await operation()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      pendingRef.current = false
      setPendingAction(null)
    }
  }

  return <div className="storage-fault-overlay" role="status"
    aria-label={m.label(sessionTitle)}>
    <div className="storage-fault-overlay__content">
      <span className="storage-fault-overlay__icon" aria-hidden="true">!</span>
      <strong>{m.title}</strong>
      <p>{faultLabel(fault.code, m)}</p>
      <small>{m.retained(formatBytes(fault.retainedBytes))}</small>
      {pendingAction === 'retry' && <p>{m.verifying}</p>}
      {pendingAction === 'end' && <p>{m.ending}</p>}
      {error && <p role="alert">{error}</p>}
      <div className="storage-fault-overlay__actions">
        <button type="button" disabled={pendingAction !== null}
          onClick={() => void run('retry', onRetry)}>{m.retryWrite}</button>
        <button type="button" disabled={pendingAction !== null}
          onClick={() => void run('end', onEnd)}>{m.endSession}</button>
      </div>
    </div>
  </div>
}

function faultLabel(
  code: StorageFaultCode, m: Messages['hierarchyTerminal']['storageFault']
): string {
  if (code === 'STORAGE_QUOTA_EXCEEDED') return m.quotaExceeded
  if (code === 'STORAGE_READ_ONLY') return m.readOnly
  return m.unavailable
}

function formatBytes(bytes: number): string {
  const safe = Math.max(0, bytes)
  if (safe >= 1024 * 1024) return `${trimNumber(safe / (1024 * 1024))} MB`
  if (safe >= 1024) return `${trimNumber(safe / 1024)} KB`
  return `${safe} B`
}

function trimNumber(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(1)
}
