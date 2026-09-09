import { useMessages } from '../i18n/LocaleProvider'

export function SessionBreadcrumb(props: {
  parentTitle?: string
  sessionCount: number
  onReturnParent?(): void
}) {
  const m = useMessages().sessionCanvas
  const { parentTitle, sessionCount, onReturnParent } = props
  const content = <>
    <strong>{parentTitle ? m.childSessionsOf(parentTitle) : m.rootSessions}</strong>
    <span> · {m.sessionCount(sessionCount)}</span>
  </>
  return <nav className="session-level-breadcrumb" aria-label={m.sessionLevel}>
    {parentTitle && onReturnParent
      ? <button type="button" className="session-return-parent"
        aria-label={m.returnToParent} title={m.returnToParent} onClick={onReturnParent}
        onKeyDown={(event) => {
          if (event.key !== 'ArrowLeft' && event.key !== 'ArrowUp') return
          event.preventDefault()
          onReturnParent()
        }}><span aria-hidden="true">←</span>{content}</button>
      : <div className="session-level-breadcrumb__root">{content}</div>}
  </nav>
}
