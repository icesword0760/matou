export function EmptyWorkspaceState({ onCreate }: { onCreate(): void }) {
  return <section><h2>还没有工作区</h2><p>选择一个本地目录开始工作。</p><button onClick={onCreate}>新建工作区</button></section>
}
