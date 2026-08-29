export function SessionHeader(props: {
  parentTitle?: string
  sessionCount: number
  canForkSibling: boolean
  disabled?: boolean
  onAddShell(): void
  onAddForkSibling(): void
}) {
  const { parentTitle, sessionCount, canForkSibling, disabled = false, onAddShell, onAddForkSibling } = props
  return <header className="session-level-header">
    <div className="session-level-context">
      <strong>{parentTitle ? `${parentTitle} 的子会话` : '根会话'}</strong>
      <span>{sessionCount} 个会话</span>
    </div>
    <div className="session-level-actions">
      {canForkSibling && <button type="button" disabled={disabled} aria-label="创建同级 Claude 分支"
        onClick={onAddForkSibling}>⑂ Fork</button>}
      <button type="button" disabled={disabled} aria-label="新增同级 Shell"
        title="新增同级 Shell" onClick={onAddShell}>＋</button>
    </div>
  </header>
}
