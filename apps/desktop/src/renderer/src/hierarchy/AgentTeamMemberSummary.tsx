import type { SessionGraphNodeView } from './hierarchy-types'

export function AgentTeamMemberSummary({
  workStatus,
  latestLines
}: {
  workStatus: SessionGraphNodeView['workStatus']
  latestLines: string[]
}) {
  return <div className="agent-team-member-summary" role="status" aria-label="队友会话摘要">
    <div className="agent-team-member-summary__heading">
      <strong>Claude Code 队友</strong>
      <span data-work-status={workStatus}>{statusLabel(workStatus)}</span>
    </div>
    <div className="agent-team-member-summary__lines">
      {(latestLines.length > 0 ? latestLines : ['等待队友更新…']).map((line, index) =>
        <div key={`${index}:${line}`}>{line}</div>
      )}
    </div>
    <small>队友会话由 Claude Code 团队管理，在此查看状态与最新摘要。</small>
  </div>
}

function statusLabel(status: SessionGraphNodeView['workStatus']): string {
  if (status === 'running' || status === 'starting') return '运行中'
  if (status === 'needs-input') return '待输入'
  if (status === 'error' || status === 'interrupted') return '异常'
  if (status === 'exited') return '已结束'
  return '空闲'
}
