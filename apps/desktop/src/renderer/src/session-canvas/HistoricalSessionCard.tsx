import { useState } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ConfirmationSequence } from '../hierarchy/ConfirmDialog'

export function HistoricalSessionCard(props: {
  node: SessionGraphNodeView
  directChildCount: number
  descendantCount: number
  descendantImpact: { running: number; needsInput: number }
  onReopen(sessionId: string): void
  onNavigateToChildren?(sessionId: string): void
  onRemove(sessionId: string, includeDescendants: boolean): void
}) {
  const {
    node, directChildCount, descendantCount, descendantImpact,
    onReopen, onNavigateToChildren, onRemove
  } = props
  const [removeMode, setRemoveMode] = useState<'leaf' | 'branch' | null>(null)
  const isClaude = node.currentMode === 'claude-code'
  return <div className="historical-session-card">
    <div className="historical-session-card__heading">
      <strong>{node.title}</strong><span>历史会话</span>
    </div>
    <p>{isClaude ? 'Claude Code 对话已结束，关系和摘要继续保留。' : 'Shell 已结束，可在原工作目录重新打开。'}</p>
    {node.latestLines.length > 0 && <pre>{node.latestLines.slice(-4).join('\n')}</pre>}
    <div className="historical-session-card__actions">
      <button type="button" onClick={() => onReopen(node.sessionId)}>
        {isClaude ? '继续会话' : '重新打开 Shell'}
      </button>
      {directChildCount > 0 && onNavigateToChildren && <button type="button"
        onClick={() => onNavigateToChildren(node.sessionId)}>查看 {directChildCount} 个子会话</button>}
      {descendantCount === 0
        ? <button type="button" aria-label={`移除历史会话：${node.title}`}
            onClick={() => setRemoveMode('leaf')}>移除</button>
        : <button type="button" aria-label={`移除整条分支：${node.title}`}
            onClick={() => setRemoveMode('branch')}>移除整条分支</button>}
    </div>
    {removeMode === 'leaf' && <ConfirmationSequence steps={[{
      title: '移除历史会话',
      body: `只会从当前画布移除这个历史节点“${node.title}”，已有文件和工作目录保持原样。`,
      confirmLabel: '确认移除', cancelLabel: '取消'
    }]} onCancel={() => setRemoveMode(null)} onComplete={() => {
      setRemoveMode(null); onRemove(node.sessionId, false)
    }} />}
    {removeMode === 'branch' && <ConfirmationSequence steps={[{
      title: '移除整条分支',
      body: `“${node.title}”下的 ${descendantCount} 个后代节点将一起从当前画布移除。${impactText(descendantImpact)}相关会话将结束。本地工作树和未提交修改会继续保留在磁盘中，需要后续自行管理。`,
      confirmLabel: '继续', cancelLabel: '取消'
    }, {
      title: '再次确认',
      body: `确认移除“${node.title}”及其全部 ${descendantCount} 个后代节点？本地工作树、文件和未提交修改不会删除。`,
      confirmLabel: '移除整条分支', cancelLabel: '取消'
    }]} onCancel={() => setRemoveMode(null)} onComplete={() => {
      setRemoveMode(null); onRemove(node.sessionId, true)
    }} />}
  </div>
}

function impactText(impact: { running: number; needsInput: number }): string {
  const items = [
    impact.running > 0 ? `${impact.running} 个运行中` : '',
    impact.needsInput > 0 ? `${impact.needsInput} 个待输入` : ''
  ].filter(Boolean)
  return items.length > 0 ? `其中 ${items.join('、')}，` : ''
}
