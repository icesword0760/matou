import { useState } from 'react'

import type { RemoveNodeScope, SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'

type RemoveDialogMessages = Messages['sessionCanvas']['removeDialog']

export function RemoveNodeDialog(props: {
  title: string
  current: Pick<SessionGraphNodeView, 'workStatus' | 'hasOwnedWorktree' | 'parentSessionId'>
  descendants: readonly SessionGraphNodeView[]
  onCancel(): void
  onConfirm(scope: RemoveNodeScope): void
}) {
  const m = useMessages().sessionCanvas.removeDialog
  const [scope, setScope] = useState<RemoveNodeScope>('node-only')
  const branchNodes = [props.current, ...props.descendants]
  const leaf = props.descendants.length === 0
  const nodeOnly = impactSummary([props.current])
  const branch = impactSummary(branchNodes)
  const selected = scope === 'node-only' ? nodeOnly : branch

  return <ConfirmDialog title={m.title(props.title)}
    body={leaf
      ? <div className="remove-node-leaf-copy">
          <p>{m.leafBody(impactLabel(nodeOnly, m))}</p>
          <p>{m.filesUnchanged}</p>
        </div>
      : <div className="remove-node-dialog">
          <p>{m.chooseScope}</p>
          <fieldset aria-label={m.scopeLabel}>
            <RemovalChoice scope="node-only" selected={scope} onSelect={setScope}
              title={m.nodeOnly} impact={nodeOnly}
              description={props.current.parentSessionId
                ? m.reparentDescendants
                : m.descendantsBecomeRoots} />
            <RemovalChoice scope="node-and-descendants" selected={scope} onSelect={setScope}
              title={m.nodeAndDescendants} impact={branch}
              description={m.descendantsRemoved(props.descendants.length)} />
          </fieldset>
          {selected.active > 0 && <p className="remove-node-dialog__warning">
            {m.activeWarning(activityLabel(selected, m))}
          </p>}
        </div>}
    confirmLabel={leaf ? m.remove : scope === 'node-only' ? m.removeNodeOnly : m.removeSessions(branch.sessions)}
    confirmTone="danger" scope="session"
    onCancel={props.onCancel} onConfirm={() => props.onConfirm(scope)} />
}

function RemovalChoice(props: {
  scope: RemoveNodeScope
  selected: RemoveNodeScope
  title: string
  impact: RemovalImpact
  description: string
  onSelect(scope: RemoveNodeScope): void
}) {
  const m = useMessages().sessionCanvas.removeDialog
  return <label className={`remove-node-choice${props.selected === props.scope ? ' is-selected' : ''}`}>
    <input type="radio" name="remove-node-scope" value={props.scope}
      checked={props.selected === props.scope} onChange={() => props.onSelect(props.scope)} />
    <span><strong>{props.title}</strong><small>{impactLabel(props.impact, m)}</small>
      <em>{props.description}</em></span>
  </label>
}

interface RemovalImpact {
  sessions: number
  ownedWorktrees?: number
  running: number
  needsInput: number
  active: number
}

function impactSummary(
  nodes: readonly Pick<SessionGraphNodeView, 'workStatus' | 'hasOwnedWorktree'>[]
): RemovalImpact {
  let ownedWorktrees = 0
  let worktreeImpactKnown = true
  let running = 0
  let needsInput = 0
  for (const node of nodes) {
    if (node.hasOwnedWorktree === undefined) worktreeImpactKnown = false
    else if (node.hasOwnedWorktree) ownedWorktrees += 1
    if (node.workStatus === 'running' || node.workStatus === 'starting') running += 1
    if (node.workStatus === 'needs-input') needsInput += 1
  }
  return {
    sessions: nodes.length,
    ...(worktreeImpactKnown ? { ownedWorktrees } : {}),
    running, needsInput, active: running + needsInput
  }
}

function impactLabel(impact: RemovalImpact, m: RemoveDialogMessages): string {
  const activity = activityLabel(impact, m)
  const worktrees = impact.ownedWorktrees === undefined
    ? ''
    : m.impactWorktrees(m.ownedWorktrees(impact.ownedWorktrees))
  return `${m.impactSessions(impact.sessions)}${worktrees}${activity ? m.impactActivity(activity) : ''}`
}

function activityLabel(impact: RemovalImpact, m: RemoveDialogMessages): string {
  return [
    impact.running > 0 ? m.running(impact.running) : '',
    impact.needsInput > 0 ? m.needsInput(impact.needsInput) : ''
  ].filter(Boolean).join(m.activityJoin)
}
