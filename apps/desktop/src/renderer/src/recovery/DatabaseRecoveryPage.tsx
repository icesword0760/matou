import { useEffect, useMemo, useRef, useState } from 'react'

import type { Locale } from '@matou/contracts'

import type {
  RuntimeLifecyclePresentation,
  RuntimeRecoveryCommandResult
} from '../../../shared/desktop-api'
import { useLocale, useMessages } from '../i18n/LocaleProvider'
import './recovery.css'

export interface DatabaseRecoveryActions {
  restore(backupId: string, expectedRecoveryId: string): Promise<unknown>
  exportBundle(): Promise<RuntimeRecoveryCommandResult>
  retry(expectedRecoveryId: string): Promise<unknown>
  startEmpty(expectedRecoveryId: string): Promise<unknown>
}

interface Props {
  state: RuntimeLifecyclePresentation
  actions: DatabaseRecoveryActions
}

export function DatabaseRecoveryPage({ state, actions }: Props) {
  const m = useMessages().recovery.page
  const locale = useLocale()
  const backups = useMemo(
    () => [...(state.recovery?.backups ?? [])]
      .sort((left, right) => right.createdAt - left.createdAt)
      .slice(0, 7),
    [state.recovery?.backups]
  )
  const [selectedBackupId, setSelectedBackupId] = useState(backups[0]?.id ?? '')
  const [pending, setPending] = useState<string>()
  const [error, setError] = useState<string>()
  const [message, setMessage] = useState<string>()
  const recovery = state.recovery
  const recoveryId = recovery?.recoveryId ?? ''
  const recoveryIdRef = useRef(recoveryId)
  const [confirmationRecoveryId, setConfirmationRecoveryId] = useState<string>()
  const emptyTriggerRef = useRef<HTMLButtonElement>(null)
  const dialogBackRef = useRef<HTMLButtonElement>(null)
  const dialogConfirmRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    if (!backups.some(({ id }) => id === selectedBackupId)) {
      setSelectedBackupId(backups[0]?.id ?? '')
    }
  }, [backups, selectedBackupId])

  useEffect(() => {
    if (confirmationRecoveryId) dialogBackRef.current?.focus()
  }, [confirmationRecoveryId])

  useEffect(() => {
    if (recoveryIdRef.current === recoveryId) return
    recoveryIdRef.current = recoveryId
    setConfirmationRecoveryId(undefined)
    setPending(undefined)
    setError(undefined)
    setMessage(undefined)
  }, [recoveryId])

  const closeEmptyConfirmation = () => {
    setConfirmationRecoveryId(undefined)
    queueMicrotask(() => emptyTriggerRef.current?.focus())
  }

  const reopening = state.snapshot.mode !== 'recovery-required'
  const busy = Boolean(pending || state.operation?.pending || reopening)
  const perform = async (name: string, operation: () => Promise<unknown>) => {
    if (busy) return
    const operationRecoveryId = recoveryIdRef.current
    setPending(name)
    setError(undefined)
    setMessage(undefined)
    try {
      const result = await operation() as RuntimeRecoveryCommandResult | undefined
      if (recoveryIdRef.current === operationRecoveryId && result?.exportedPath) {
        setMessage(m.exportedTo(result.exportedPath))
      }
    } catch (reason) {
      if (recoveryIdRef.current === operationRecoveryId) {
        setError(reason instanceof Error ? reason.message : String(reason))
      }
    } finally {
      if (recoveryIdRef.current === operationRecoveryId) setPending(undefined)
    }
  }

  const ownershipRecovery = recovery?.reason === 'ownership-recovery-required'
  const title = ownershipRecovery ? m.ownershipTitle : m.title
  const description = ownershipRecovery
    ? m.ownershipDescription
    : recovery?.reason === 'wal-recovery-required'
      ? m.walDescription
      : m.integrityDescription

  return <main className="database-recovery-page" aria-labelledby="database-recovery-title">
    <section className="database-recovery-card">
      <header>
        <p className="database-recovery-eyebrow">{m.eyebrow}</p>
        <h1 id="database-recovery-title">{title}</h1>
        <p>{description}</p>
      </header>

      {recovery?.error && <p role="alert" className="database-recovery-error">{recovery.error}</p>}
      {(error ?? state.operation?.error) &&
        <p role="alert" className="database-recovery-error">{error ?? state.operation?.error}</p>}
      {message && <p role="status" className="database-recovery-success">{message}</p>}
      {(state.operation?.pending || reopening) && <p role="status" className="database-recovery-progress">
        {m.working}
      </p>}

      <section aria-labelledby="database-backups-title" className="database-recovery-backups">
        <div className="database-recovery-section-title">
          <h2 id="database-backups-title">{m.backups}</h2>
          <span>{m.backupCount(backups.length)}</span>
        </div>
        {backups.length === 0
          ? <p className="database-recovery-empty">{m.noBackups}</p>
          : <div className="database-recovery-list">
            {backups.map((backup) => <label key={backup.id}>
              <input
                type="radio"
                name="database-backup"
                value={backup.id}
                checked={selectedBackupId === backup.id}
                disabled={busy}
                onChange={() => setSelectedBackupId(backup.id)}
              />
              <span>
                <strong>{backup.id}</strong>
                <small>{formatTime(backup.createdAt, locale)} · {m.schemaVersion(backup.schemaVersion)} · {formatSize(backup.size)}</small>
              </span>
            </label>)}
          </div>}
      </section>

      <div className="database-recovery-primary-actions">
        <button
          className="primary"
          disabled={busy || !selectedBackupId || !recoveryId}
          onClick={() => void perform('restore', () => actions.restore(selectedBackupId, recoveryId))}
        >{pending === 'restore' ? m.restoring : m.restoreSelected}</button>
        <button disabled={busy || !recoveryId}
          onClick={() => void perform('retry', () => actions.retry(recoveryId))}>
          {pending === 'retry' ? m.checking : m.recheckDatabase}
        </button>
        <button disabled={busy} onClick={() => void perform('export', actions.exportBundle)}>
          {pending === 'export' ? m.exporting : m.exportBundle}
        </button>
      </div>

      <footer>
        <button ref={emptyTriggerRef} className="danger-link" disabled={busy}
          onClick={() => setConfirmationRecoveryId(recoveryId)}>
          {m.startEmpty}
        </button>
        <p>{m.startEmptyHint}</p>
      </footer>
    </section>

    {confirmationRecoveryId && <div className="database-recovery-dialog-backdrop">
      <section role="dialog" aria-modal="true" aria-label={m.confirmEmptyLabel}
        className="database-recovery-dialog" onKeyDown={(event) => {
          if (event.key === 'Escape') {
            event.preventDefault()
            closeEmptyConfirmation()
            return
          }
          if (event.key !== 'Tab') return
          if (event.shiftKey && document.activeElement === dialogBackRef.current) {
            event.preventDefault()
            dialogConfirmRef.current?.focus()
          } else if (!event.shiftKey && document.activeElement === dialogConfirmRef.current) {
            event.preventDefault()
            dialogBackRef.current?.focus()
          }
        }}>
        <h2>{m.confirmEmptyTitle}</h2>
        <p>{m.confirmEmptyBody}</p>
        <div>
          <button ref={dialogBackRef} onClick={closeEmptyConfirmation}>{m.back}</button>
          <button ref={dialogConfirmRef} className="danger" onClick={() => {
            const frozenRecoveryId = confirmationRecoveryId
            setConfirmationRecoveryId(undefined)
            void perform('empty', () => actions.startEmpty(frozenRecoveryId))
          }}>{m.confirmEmpty}</button>
        </div>
      </section>
    </div>}
  </main>
}

function formatTime(createdAt: number, locale: Locale): string {
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', hour12: false
  }).format(new Date(createdAt))
}

function formatSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
