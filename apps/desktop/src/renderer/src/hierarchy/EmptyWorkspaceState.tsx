import { useMessages } from '../i18n/LocaleProvider'

export function EmptyWorkspaceState({ onCreate }: { onCreate(): void }) {
  const m = useMessages().hierarchyShell.emptyWorkspace
  return <section><h2>{m.title}</h2><p>{m.hint}</p><button onClick={onCreate}>{m.create}</button></section>
}
