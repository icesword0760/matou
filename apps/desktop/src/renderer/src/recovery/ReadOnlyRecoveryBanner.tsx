import { useState } from 'react'

import type { RuntimeRecoveryCommandResult } from '../../../shared/desktop-api'
import { messages } from '../i18n/current'
import { useMessages } from '../i18n/LocaleProvider'
import './read-only-recovery.css'

/** Headline of the banner, reused as the disabled reason of blocked commands. */
export function readOnlyReason(): string {
  return messages().hierarchyShell.readOnlyRecoveryReason
}

export function ReadOnlyRecoveryBanner(props: {
  exportBundle(): Promise<RuntimeRecoveryCommandResult>
  onSearch?(): void
}) {
  const all = useMessages()
  const m = all.recovery.readOnlyBanner
  const [exporting, setExporting] = useState(false)
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')

  const exportBundle = async () => {
    setExporting(true)
    setMessage('')
    setError('')
    try {
      const result = await props.exportBundle()
      setMessage(result.exportedPath ? m.exportedTo(result.exportedPath) : m.exported)
    } catch (reason) {
      setError(m.exportFailed(reason instanceof Error ? reason.message : String(reason)))
    } finally {
      setExporting(false)
    }
  }

  return <section className="read-only-recovery-banner" role="status" aria-live="polite">
    <div>
      <strong>{all.hierarchyShell.readOnlyRecoveryReason}</strong>
      <span>{m.explanation}</span>
    </div>
    <div className="read-only-recovery-banner__actions">
      {props.onSearch && <button type="button" onClick={props.onSearch}>{m.searchTerminal}</button>}
      <button type="button" disabled={exporting} onClick={() => void exportBundle()}>
        {exporting ? m.exporting : m.exportDatabase}
      </button>
    </div>
    {message && <output>{message}</output>}
    {error && <span role="alert">{error}</span>}
  </section>
}
