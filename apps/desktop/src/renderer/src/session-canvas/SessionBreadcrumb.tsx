export function SessionBreadcrumb(props: {
  parentTitle?: string
  sessionCount: number
  onReturnParent?(): void
}) {
  const { parentTitle, sessionCount, onReturnParent } = props
  const content = <>
    <strong>{parentTitle ? `${parentTitle} 的子会话` : '根会话'}</strong>
    <span> · {sessionCount} 个会话</span>
  </>
  return <nav className="session-level-breadcrumb" aria-label="会话层级">
    {parentTitle && onReturnParent
      ? <button type="button" className="session-return-parent"
        aria-label="返回父会话" title="返回父会话" onClick={onReturnParent}
        onKeyDown={(event) => {
          if (event.key !== 'ArrowLeft' && event.key !== 'ArrowUp') return
          event.preventDefault()
          onReturnParent()
        }}><span aria-hidden="true">←</span>{content}</button>
      : <div className="session-level-breadcrumb__root">{content}</div>}
  </nav>
}
