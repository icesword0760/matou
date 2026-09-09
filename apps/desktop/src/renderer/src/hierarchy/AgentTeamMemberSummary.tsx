import { useMessages } from '../i18n/LocaleProvider'
import type { SessionGraphNodeView } from './hierarchy-types'

export function AgentTeamMemberSummary({
  workStatus,
  latestLines
}: {
  workStatus: SessionGraphNodeView['workStatus']
  latestLines: string[]
}) {
  const m = useMessages().hierarchyTerminal.teamMember
  return <div className="agent-team-member-summary" role="status" aria-label={m.summary}>
    <div className="agent-team-member-summary__heading">
      <strong>{m.heading}</strong>
      <span data-work-status={workStatus}>{m.workStatus[workStatus]}</span>
    </div>
    <div className="agent-team-member-summary__lines">
      {(latestLines.length > 0 ? latestLines : [m.waiting]).map((line, index) =>
        <div key={`${index}:${line}`}>{line}</div>
      )}
    </div>
    <small>{m.hint}</small>
  </div>
}
