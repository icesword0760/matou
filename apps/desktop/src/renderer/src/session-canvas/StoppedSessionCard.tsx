import { useState } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { RemoveNodeIcon, removalBody, removalConfirmLabel, removalTitle } from '../hierarchy/TerminalPane'

export function StoppedSessionCard(props: {
  node: SessionGraphNodeView
  directChildCount?: number
  descendantCount?: number
  descendantImpact?: { running: number; needsInput: number }
  onRemoveBranch?(sessionId: string, includeDescendants: boolean): unknown
}) {
  const {
    node, directChildCount = 0, descendantCount = 0,
    descendantImpact = { running: 0, needsInput: 0 }, onRemoveBranch
  } = props
  const [removalOpen, setRemovalOpen] = useState(false)
  return <div className="stopped-session-card">
    <header><strong>{node.title}</strong><div className="stopped-session-card__actions">
      <span>正在恢复会话…</span>
      {onRemoveBranch && <button className="pane-fork pane-remove" type="button"
        aria-label={`移出节点：${node.title}`} title="移出节点"
        onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
        onClick={(event) => { event.stopPropagation(); setRemovalOpen(true) }}><RemoveNodeIcon /></button>}
    </div></header>
    {node.latestLines.length > 0 && <pre>{node.latestLines.slice(-8).join('\n')}</pre>}
    {removalOpen && <ConfirmDialog title={removalTitle(node.title, descendantCount)}
      body={removalBody(node.title, directChildCount, descendantCount, descendantImpact)}
      confirmLabel={removalConfirmLabel(descendantImpact, descendantCount)} confirmTone="danger"
      cancelLabel="取消" scope="session"
      onCancel={() => setRemovalOpen(false)} onConfirm={() => {
        setRemovalOpen(false)
        void Promise.resolve(onRemoveBranch?.(node.sessionId, descendantCount > 0)).catch(NOOP)
      }} />}
  </div>
}

function NOOP(): void {}
