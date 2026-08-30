import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'

export function ChildSessionBadge(props: {
  children: SessionGraphNodeView[]
  historicalCount: number
  onOpen(): void
}) {
  const { children, historicalCount, onOpen } = props
  const active = children.filter(({ archivedAt }) => archivedAt === undefined)
  const total = active.length + historicalCount
  if (total === 0) return null
  const counts = statusCounts(active)
  const claude = active.filter(({ currentMode }) => currentMode === 'claude-code').length
  const shell = active.filter(({ currentMode }) => currentMode === 'shell').length
  const highest = highestStatus(counts)
  const detail = [
    `Claude ${claude} · Shell ${shell}`,
    `运行中 ${counts.running} · 待输入 ${counts.needsInput} · 空闲 ${counts.idle}`,
    `错误 ${counts.error} · 中断 ${counts.interrupted}`,
    historicalCount > 0 ? `+${historicalCount} 历史` : ''
  ].filter(Boolean).join('；')
  const summary = counts.running > 0
    ? `${total} 分支 · ${counts.running} 运行中`
    : `${total} 分支 · 空闲`
  return <span className="child-session-badge-wrap">
    <button type="button" className={`child-session-badge status-${highest}`}
      aria-label={`查看 ${total} 个子会话`} title={detail}
      onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }} onClick={(event) => {
        event.stopPropagation()
        onOpen()
      }}>
      <span className="child-session-badge__dot" aria-hidden="true" />
      <span>{summary}</span>
      <span className="child-session-badge__chevron" aria-hidden="true">›</span>
    </button>
    <span className="child-session-badge__tooltip" role="tooltip">{detail}</span>
  </span>
}

function statusCounts(nodes: SessionGraphNodeView[]) {
  return nodes.reduce((counts, node) => {
    if (node.workStatus === 'error') counts.error += 1
    else if (node.workStatus === 'needs-input') counts.needsInput += 1
    else if (node.workStatus === 'running' || node.workStatus === 'starting') counts.running += 1
    else if (node.workStatus === 'interrupted') counts.interrupted += 1
    else counts.idle += 1
    return counts
  }, { error: 0, needsInput: 0, running: 0, interrupted: 0, idle: 0 })
}

function highestStatus(counts: ReturnType<typeof statusCounts>): string {
  if (counts.error > 0) return 'error'
  if (counts.needsInput > 0) return 'needs-input'
  if (counts.running > 0) return 'running'
  if (counts.interrupted > 0) return 'interrupted'
  return 'idle'
}
