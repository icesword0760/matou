import { useState } from 'react'

import type { RemoveNodeScope, SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ConfirmDialog } from '../hierarchy/ConfirmDialog'

export function RemoveNodeDialog(props: {
  title: string
  current: Pick<SessionGraphNodeView, 'workStatus' | 'hasOwnedWorktree' | 'parentSessionId'>
  descendants: readonly SessionGraphNodeView[]
  onCancel(): void
  onConfirm(scope: RemoveNodeScope): void
}) {
  const [scope, setScope] = useState<RemoveNodeScope>('node-only')
  const branchNodes = [props.current, ...props.descendants]
  const leaf = props.descendants.length === 0
  const nodeOnly = impactSummary([props.current])
  const branch = impactSummary(branchNodes)
  const selected = scope === 'node-only' ? nodeOnly : branch

  return <ConfirmDialog title={`移除节点“${props.title}”？`}
    body={leaf
      ? <div className="remove-node-leaf-copy">
          <p>{impactLabel(nodeOnly)}。移除后，该会话会从会话列表和 DAG 中消失。</p>
          <p>项目文件和自有 Worktree 保持原样。</p>
        </div>
      : <div className="remove-node-dialog">
          <p>请选择移除范围。项目文件和自有 Worktree 保持原样。</p>
          <fieldset aria-label="移除范围">
            <RemovalChoice scope="node-only" selected={scope} onSelect={setScope}
              title="仅移除当前节点" impact={nodeOnly}
              description={props.current.parentSessionId
                ? '后代会话将重连到当前节点的父级。'
                : '直接后代会话将成为根节点。'} />
            <RemovalChoice scope="node-and-descendants" selected={scope} onSelect={setScope}
              title="移除当前节点及全部后代" impact={branch}
              description={`当前节点与 ${props.descendants.length} 个后代会话会全部移除。`} />
          </fieldset>
          {selected.active > 0 && <p className="remove-node-dialog__warning">
            其中 {activityLabel(selected)}的会话将先停止。
          </p>}
        </div>}
    confirmLabel={leaf ? '移除' : scope === 'node-only' ? '移除当前节点' : `移除 ${branch.sessions} 个会话`}
    confirmTone="danger" cancelLabel="取消" scope="session"
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
  return <label className={`remove-node-choice${props.selected === props.scope ? ' is-selected' : ''}`}>
    <input type="radio" name="remove-node-scope" value={props.scope}
      checked={props.selected === props.scope} onChange={() => props.onSelect(props.scope)} />
    <span><strong>{props.title}</strong><small>{impactLabel(props.impact)}</small>
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

function impactLabel(impact: RemovalImpact): string {
  const activity = activityLabel(impact)
  const worktrees = impact.ownedWorktrees === undefined ? '' : `、${impact.ownedWorktrees} 个自有 Worktree`
  return `影响 ${impact.sessions} 个会话${worktrees}${activity ? `；其中 ${activity}` : ''}`
}

function activityLabel(impact: RemovalImpact): string {
  return [
    impact.running > 0 ? `${impact.running} 个运行中` : '',
    impact.needsInput > 0 ? `${impact.needsInput} 个待输入` : ''
  ].filter(Boolean).join('、')
}
