import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'

export function HistoricalSessionCard(props: {
  node: SessionGraphNodeView
  onReopen(sessionId: string): void
}) {
  const { node, onReopen } = props
  const isClaude = node.currentMode === 'claude-code'
  return <div className="historical-session-card">
    <div className="historical-session-card__heading">
      <strong>{node.title}</strong><span>历史会话</span>
    </div>
    <p>{isClaude ? 'Claude Code 对话已结束，关系和摘要继续保留。' : 'Shell 已结束，可在原工作目录重新打开。'}</p>
    {node.latestLines.length > 0 && <pre>{node.latestLines.slice(-4).join('\n')}</pre>}
    <button type="button" onClick={() => onReopen(node.sessionId)}>
      {isClaude ? '继续会话' : '重新打开 Shell'}
    </button>
  </div>
}
