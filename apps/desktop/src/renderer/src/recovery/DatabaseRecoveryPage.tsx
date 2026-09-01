import { useEffect, useMemo, useRef, useState } from 'react'

import type {
  RuntimeLifecyclePresentation,
  RuntimeRecoveryCommandResult
} from '../../../shared/desktop-api'
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
        setMessage(`恢复资料已导出到 ${result.exportedPath}`)
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
  const title = ownershipRecovery ? '数据库占用状态需要处理' : '数据库需要恢复'
  const description = ownershipRecovery
    ? '数据库占用记录或接管状态异常，原数据库仍保留在原位置。处理前不会将其当作损坏文件移动。'
    : recovery?.reason === 'wal-recovery-required'
      ? '数据库日志状态不完整。Matou 已保留原数据库和日志文件，请选择备份恢复或导出资料。'
      : '数据库完整性检查未通过。Matou 已保留原文件，不会直接进入全新空工作区。'

  return <main className="database-recovery-page" aria-labelledby="database-recovery-title">
    <section className="database-recovery-card">
      <header>
        <p className="database-recovery-eyebrow">Matou 数据恢复</p>
        <h1 id="database-recovery-title">{title}</h1>
        <p>{description}</p>
      </header>

      {recovery?.error && <p role="alert" className="database-recovery-error">{recovery.error}</p>}
      {(error ?? state.operation?.error) &&
        <p role="alert" className="database-recovery-error">{error ?? state.operation?.error}</p>}
      {message && <p role="status" className="database-recovery-success">{message}</p>}
      {(state.operation?.pending || reopening) && <p role="status" className="database-recovery-progress">
        正在处理数据库恢复，请保持 Matou 开启…
      </p>}

      <section aria-labelledby="database-backups-title" className="database-recovery-backups">
        <div className="database-recovery-section-title">
          <h2 id="database-backups-title">可用备份</h2>
          <span>{backups.length} 份</span>
        </div>
        {backups.length === 0
          ? <p className="database-recovery-empty">暂未找到通过完整性校验的备份。</p>
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
                <small>{formatTime(backup.createdAt)} · 数据版本 {backup.schemaVersion} · {formatSize(backup.size)}</small>
              </span>
            </label>)}
          </div>}
      </section>

      <div className="database-recovery-primary-actions">
        <button
          className="primary"
          disabled={busy || !selectedBackupId || !recoveryId}
          onClick={() => void perform('restore', () => actions.restore(selectedBackupId, recoveryId))}
        >{pending === 'restore' ? '正在恢复…' : '恢复所选备份'}</button>
        <button disabled={busy || !recoveryId}
          onClick={() => void perform('retry', () => actions.retry(recoveryId))}>
          {pending === 'retry' ? '正在检查…' : '重新检查数据库'}
        </button>
        <button disabled={busy} onClick={() => void perform('export', actions.exportBundle)}>
          {pending === 'export' ? '正在导出…' : '导出恢复资料'}
        </button>
      </div>

      <footer>
        <button ref={emptyTriggerRef} className="danger-link" disabled={busy}
          onClick={() => setConfirmationRecoveryId(recoveryId)}>
          创建全新空数据库
        </button>
        <p>此入口只在你明确确认后执行；现有隔离文件和备份继续保留。</p>
      </footer>
    </section>

    {confirmationRecoveryId && <div className="database-recovery-dialog-backdrop">
      <section role="dialog" aria-modal="true" aria-label="确认创建全新空数据库"
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
        <h2>确认创建全新空数据库？</h2>
        <p>Matou 将显示一个全新的空工作区。当前损坏或异常文件和备份仍会保留，便于后续导出与排查。</p>
        <div>
          <button ref={dialogBackRef} onClick={closeEmptyConfirmation}>返回</button>
          <button ref={dialogConfirmRef} className="danger" onClick={() => {
            const frozenRecoveryId = confirmationRecoveryId
            setConfirmationRecoveryId(undefined)
            void perform('empty', () => actions.startEmpty(frozenRecoveryId))
          }}>确认创建空数据库</button>
        </div>
      </section>
    </div>}
  </main>
}

function formatTime(createdAt: number): string {
  return new Intl.DateTimeFormat('zh-CN', {
    year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit'
  }).format(new Date(createdAt))
}

function formatSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
