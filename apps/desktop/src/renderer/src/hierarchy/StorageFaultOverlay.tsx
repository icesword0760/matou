import { useRef, useState } from 'react'

import type { StorageFaultCode } from '@matou/contracts'

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
    aria-label={`终端记录写入异常：${sessionTitle}`}>
    <div className="storage-fault-overlay__content">
      <span className="storage-fault-overlay__icon" aria-hidden="true">!</span>
      <strong>终端已暂停：输出记录写入失败</strong>
      <p>{faultLabel(fault.code)}</p>
      <small>已暂存 {formatBytes(fault.retainedBytes)} 输出，其他会话不受影响</small>
      {pendingAction === 'retry' && <p>正在验证存储并补写输出…</p>}
      {pendingAction === 'end' && <p>正在安全结束会话…</p>}
      {error && <p role="alert">{error}</p>}
      <div className="storage-fault-overlay__actions">
        <button type="button" disabled={pendingAction !== null}
          onClick={() => void run('retry', onRetry)}>重试写入</button>
        <button type="button" disabled={pendingAction !== null}
          onClick={() => void run('end', onEnd)}>结束会话</button>
      </div>
    </div>
  </div>
}

function faultLabel(code: StorageFaultCode): string {
  if (code === 'STORAGE_QUOTA_EXCEEDED') return '磁盘空间或存储配额不足'
  if (code === 'STORAGE_READ_ONLY') return '终端记录目录当前只读或没有写入权限'
  return '存储设备暂时不可用'
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
