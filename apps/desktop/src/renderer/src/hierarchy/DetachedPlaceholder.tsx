export function DetachedPlaceholder(props: {
  title: string
  windowId: string
  onReturn: (windowId: string) => void
}) {
  return <section className="detached-placeholder" data-testid="detached-placeholder">
    <div>
      <strong>{props.title}</strong>
      <span>已脱出</span>
    </div>
    <p>终端正在独立窗口中运行，进程与会话保持不变。</p>
    <button onClick={() => props.onReturn(props.windowId)}>
      归还到当前页签
    </button>
  </section>
}
