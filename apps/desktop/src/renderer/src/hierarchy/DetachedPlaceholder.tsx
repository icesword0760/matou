import { useMessages } from '../i18n/LocaleProvider'

export function DetachedPlaceholder(props: {
  title: string
  windowId: string
  onReturn: (windowId: string) => void
}) {
  const m = useMessages().hierarchyTerminal.detachedPlaceholder
  return <section className="detached-placeholder" data-testid="detached-placeholder">
    <div>
      <strong>{props.title}</strong>
      <span>{m.detached}</span>
    </div>
    <p>{m.body}</p>
    <button onClick={() => props.onReturn(props.windowId)}>
      {m.returnToTab}
    </button>
  </section>
}
