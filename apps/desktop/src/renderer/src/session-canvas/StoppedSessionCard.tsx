import { useEffect, useState } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { RemoveNodeIcon, removalBody, removalConfirmLabel, removalTitle } from '../hierarchy/TerminalPane'

export function StoppedSessionCard(props: {
  node: SessionGraphNodeView
  descendantNodes?: SessionGraphNodeView[]
  disabled?: boolean
  disabledReason?: string
  onRemoveBranch?(sessionId: string, scope: RemoveNodeScope): unknown
}) {
  const {
    node, descendantNodes = [], disabled = false,
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
        aria-label={`移除节点…：${node.title}`} disabled={disabled}
        title={disabledReason ?? '移除节点…'}
        onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
        onClick={(event) => {
          event.stopPropagation()
          if (!disabled) setRemovalOpen(true)
        }}><RemoveNodeIcon /></button>}
    </div></header>
    {node.latestLines.length > 0 && <pre>{node.latestLines.slice(-8).join('\n')}</pre>}
    {removalOpen && <ConfirmDialog title={removalTitle(node.title, descendantCount)}
      body={removalBody(node.title, directChildCount, descendantCount, descendantImpact)}
      confirmLabel={removalConfirmLabel(descendantImpact, descendantCount)} confirmTone="danger"
      cancelLabel="取消" scope="session"
      onCancel={() => setRemovalOpen(false)} onConfirm={() => {
        setRemovalOpen(false)
        void Promise.resolve(onRemoveBranch?.(node.sessionId, scope)).catch(NOOP)
      }} />}
  </div>
}

function NOOP(): void {}
