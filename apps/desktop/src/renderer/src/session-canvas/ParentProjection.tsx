import type { CSSProperties } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'

export function ParentProjection(props: {
  parent: SessionGraphNodeView
  pullDistance: number
  progress: number
  effectIntensity: number
}) {
  const { parent, pullDistance, progress, effectIntensity } = props
  const ready = progress >= 1
  const percent = Math.round(Math.max(0, Math.min(1, progress)) * 100)
  return <aside className="parent-projection" data-testid="parent-projection"
    data-ready={ready} aria-live="polite"
    style={{
      '--parent-pull-distance': `${pullDistance}px`,
      '--parent-pull-progress': Math.max(0, Math.min(1, progress)),
      '--parent-pull-speed': Math.max(0, Math.min(1, effectIntensity))
    } as CSSProperties}>
    <div className="parent-projection__card">
      <span className={`parent-projection__status status-${parent.workStatus}`}>
        {statusLabel(parent.workStatus)}
      </span>
      <strong>{parent.title}</strong>
      <span>{parent.currentMode === 'claude-code' ? 'Claude Code' : 'Shell'}</span>
      {parent.latestLines.length > 0 && <pre>{parent.latestLines.slice(-3).join('\n')}</pre>}
      <div className="parent-projection__instruction">
        <b>{ready ? '已到达 · 松手返回父会话' : '右拉至目标，松手返回父会话'}</b>
        <span>{percent}%</span>
      </div>
      <div className="parent-projection__energy" role="progressbar" aria-label="返回父会话进度"
        aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
        <span /><i />
      </div>
    </div>
  </aside>
}

function statusLabel(status: SessionGraphNodeView['workStatus']): string {
  if (status === 'needs-input') return '等待输入'
  if (status === 'running' || status === 'starting') return '运行中'
  if (status === 'error') return '异常'
  if (status === 'interrupted') return '已中断'
  return '空闲'
}
