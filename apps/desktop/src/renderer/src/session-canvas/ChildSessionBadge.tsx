import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'
import { AppIcon } from '../ui/AppIcon'

type ChildBadgeMessages = Messages['sessionCanvas']['childBadge']

export function ChildSessionBadge(props: {
  children: SessionGraphNodeView[]
  onOpen(): void
}) {
  const m = useMessages().sessionCanvas.childBadge
  const { children, onOpen } = props
  if (children.length === 0) return null
  const counts = statusCounts(children)
  const claude = children.filter(({ currentMode }) => currentMode === 'claude-code').length
  const shell = children.filter(({ currentMode }) => currentMode === 'shell').length
  const highest = highestStatus(counts)
  const detail = [
    `Claude ${claude} · Shell ${shell}`,
    statusDetail(counts, m)
  ].filter((value) => Boolean(value)).join(m.detailJoin)
  const status = summaryStatus(counts, m)
  const summary = `${m.branches(children.length)}${status ? ` · ${status}` : ''}`
  const accessibleLabel = m.viewChildren(children.length)
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

function summaryStatus(counts: ReturnType<typeof statusCounts>, m: ChildBadgeMessages): string {
  if (counts.error > 0) return m.summaryError(counts.error)
  if (counts.needsInput > 0) return m.summaryNeedsInput(counts.needsInput)
  if (counts.running > 0) return m.summaryRunning(counts.running)
  if (counts.starting > 0) return m.summaryStarting(counts.starting)
  return ''
}

function statusDetail(counts: ReturnType<typeof statusCounts>, m: ChildBadgeMessages): string {
  return [
    counts.running > 0 ? m.detailRunning(counts.running) : '',
    counts.starting > 0 ? m.detailStarting(counts.starting) : '',
    counts.needsInput > 0 ? m.detailNeedsInput(counts.needsInput) : '',
    counts.error > 0 ? m.detailError(counts.error) : ''
  ].filter(Boolean).join(' · ')
}
