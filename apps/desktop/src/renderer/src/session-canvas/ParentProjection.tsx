import type { CSSProperties } from 'react'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { useMessages } from '../i18n/LocaleProvider'

export function ParentProjection(props: {
  parent: SessionGraphNodeView
  pullDistance: number
  progress: number
  effectIntensity: number
}) {
  const m = useMessages().sessionCanvas
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
        {m.parentStatus[parent.workStatus]}
      </span>
      <strong>{parent.title}</strong>
      <span>{parent.currentMode === 'claude-code' ? 'Claude Code' : 'Shell'}</span>
      {parent.latestLines.length > 0 && <pre>{parent.latestLines.slice(-3).join('\n')}</pre>}
      <div className="parent-projection__instruction">
        <b>{ready ? m.parentPullReady : m.parentPullHint}</b>
        <span>{percent}%</span>
      </div>
      <div className="parent-projection__energy" role="progressbar" aria-label={m.parentPullProgress}
        aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
        <span /><i />
      </div>
    </div>
  </aside>
}
