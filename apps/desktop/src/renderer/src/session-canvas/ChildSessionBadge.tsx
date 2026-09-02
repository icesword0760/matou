import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { AppIcon } from '../ui/AppIcon'

export function ChildSessionBadge(props: {
  children: SessionGraphNodeView[]
  onOpen(): void
}) {
  const { children, onOpen } = props
  if (children.length === 0) return null
  const counts = statusCounts(children)
  const claude = children.filter(({ currentMode }) => currentMode === 'claude-code').length
  const shell = children.filter(({ currentMode }) => currentMode === 'shell').length
  const highest = highestStatus(counts)
  const detail = [
    `Claude ${claude} · Shell ${shell}`,
    statusDetail(counts)
  ].filter((value) => Boolean(value)).join('；')
  const status = summaryStatus(counts)
  const summary = `${children.length} 分支${status ? ` · ${status}` : ''}`
  const accessibleLabel = `查看 ${children.length} 个子会话`
  return <span className="child-session-badge-wrap">
    <button type="button" className={`child-session-badge status-${highest}`}
      aria-label={accessibleLabel}
      onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }} onClick={(event) => {
        event.stopPropagation()
        onOpen()
      }}>
      <AppIcon name="layers" className="child-session-badge__icon" size={14} />
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
    else if (node.workStatus === 'running') counts.running += 1
    else if (node.workStatus === 'starting') counts.starting += 1
    return counts
  }, { error: 0, needsInput: 0, running: 0, starting: 0 })
}

function highestStatus(counts: ReturnType<typeof statusCounts>): string {
  if (counts.error > 0) return 'error'
  if (counts.needsInput > 0) return 'needs-input'
  if (counts.running > 0) return 'running'
  if (counts.starting > 0) return 'starting'
  return 'idle'
}

function summaryStatus(counts: ReturnType<typeof statusCounts>): string {
  if (counts.error > 0) return `${counts.error} 异常`
  if (counts.needsInput > 0) return `${counts.needsInput} 待输入`
  if (counts.running > 0) return `${counts.running} 运行中`
  if (counts.starting > 0) return `${counts.starting} 准备中`
  return ''
}

function statusDetail(counts: ReturnType<typeof statusCounts>): string {
  return [
    counts.running > 0 ? `运行中 ${counts.running}` : '',
    counts.starting > 0 ? `准备中 ${counts.starting}` : '',
    counts.needsInput > 0 ? `待输入 ${counts.needsInput}` : '',
    counts.error > 0 ? `错误 ${counts.error}` : ''
  ].filter(Boolean).join(' · ')
}
