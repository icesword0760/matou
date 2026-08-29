export function SessionHeader(props: {
  parentTitle?: string
  sessionCount: number
  canForkSibling: boolean
  historicalCount?: number
  showHistory?: boolean
  disabled?: boolean
  onAddShell(): void
  onAddForkSibling(): void
  onToggleHistory(): void
}) {
  const {
    parentTitle, sessionCount, canForkSibling, historicalCount = 0, showHistory = false,
    disabled = false, onAddShell, onAddForkSibling, onToggleHistory
  } = props
  return <header className="session-level-header">
    <div className="session-level-context">
      <strong>{parentTitle ? `${parentTitle} 的子会话` : '根会话'}</strong>
      <span>{sessionCount} 个会话</span>
    </div>
    <div className="session-level-actions">
      {historicalCount > 0 && <button type="button" aria-pressed={showHistory}
        aria-label={showHistory ? '隐藏历史会话' : `显示历史会话 (${historicalCount})`}
        onClick={onToggleHistory}>{showHistory ? '隐藏历史' : `历史 ${historicalCount}`}</button>}
      {canForkSibling && <button type="button" disabled={disabled} aria-label="创建同级 Claude 分支"
        onClick={onAddForkSibling}>⑂ Fork</button>}
      <button type="button" disabled={disabled} aria-label="新增同级 Shell"
        title="新增同级 Shell" onClick={onAddShell}>＋</button>
    </div>
  </header>
}
