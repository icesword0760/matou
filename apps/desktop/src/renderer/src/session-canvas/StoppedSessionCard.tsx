import { useEffect, useState } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { RemoveNodeIcon, removalBody, removalConfirmLabel } from '../hierarchy/TerminalPane'

export function StoppedSessionCard(props: {
  node: SessionGraphNodeView
  directChildCount?: number
  descendantCount?: number
  descendantImpact?: { running: number; needsInput: number }
  disabled?: boolean
  disabledReason?: string
  onRemoveBranch?(sessionId: string, includeDescendants: boolean): unknown
}) {
  const {
    node, directChildCount = 0, descendantCount = 0,
    descendantImpact = { running: 0, needsInput: 0 }, disabled = false,
    disabledReason, onRemoveBranch
  } = props
  const [removalOpen, setRemovalOpen] = useState(false)
  useEffect(() => {
    if (disabled) setRemovalOpen(false)
  }, [disabled])
  return <div className="stopped-session-card">
    <header><strong>{node.title}</strong><div className="stopped-session-card__actions">
      <span>正在恢复会话…</span>
      {onRemoveBranch && <button className="pane-fork pane-remove" type="button"
        aria-label={`移出节点：${node.title}`} disabled={disabled}
        title={disabledReason ?? '移出节点'}
        onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
        onClick={(event) => {
          event.stopPropagation()
          if (!disabled) setRemovalOpen(true)
        }}><RemoveNodeIcon /></button>}
    </div></header>
    {node.latestLines.length > 0 && <pre>{node.latestLines.slice(-8).join('\n')}</pre>}
    {removalOpen && !disabled && <ConfirmDialog title={`移除“${node.title}”及其整个分支？`}
      body={removalBody(node.title, directChildCount, descendantCount, descendantImpact)}
      confirmLabel={removalConfirmLabel(descendantImpact)} cancelLabel="取消" scope="session"
      onCancel={() => setRemovalOpen(false)} onConfirm={() => {
        setRemovalOpen(false)
        void Promise.resolve(onRemoveBranch?.(node.sessionId, descendantCount > 0)).catch(NOOP)
      }} />}
  </div>
}

function NOOP(): void {}
