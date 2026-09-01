import { useState } from 'react'

import type { RuntimeRecoveryCommandResult } from '../../../shared/desktop-api'
import './read-only-recovery.css'

const READ_ONLY_REASON = '数据库处于只读恢复模式'

export function ReadOnlyRecoveryBanner(props: {
  exportBundle(): Promise<RuntimeRecoveryCommandResult>
}) {
  const [exporting, setExporting] = useState(false)
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')

  const exportBundle = async () => {
    setExporting(true)
    setMessage('')
    setError('')
    try {
      const result = await props.exportBundle()
      setMessage(result.exportedPath
        ? `数据库资料已导出到 ${result.exportedPath}`
        : '数据库资料已导出')
    } catch (reason) {
      setError(`导出失败：${reason instanceof Error ? reason.message : String(reason)}`)
    } finally {
      setExporting(false)
    }
  }

  return <section className="read-only-recovery-banner" role="status" aria-live="polite">
    <div>
      <strong>{READ_ONLY_REASON}</strong>
      <span>现有工作空间、事项和会话仍可浏览、搜索与复制；可能改动数据的操作已暂停。</span>
    </div>
    <button type="button" disabled={exporting} onClick={() => void exportBundle()}>
      {exporting ? '正在导出…' : '导出数据库资料'}
    </button>
    {message && <output>{message}</output>}
    {error && <span role="alert">{error}</span>}
  </section>
}

export { READ_ONLY_REASON }
