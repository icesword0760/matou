import type { ForkProgress } from '@matou/domain'

import { useMessages } from '../i18n/LocaleProvider'

export function ForkProgressOverlay({ progress }: { progress: ForkProgress }) {
  const m = useMessages().sessionCanvas
  const completed = Math.max(0, Math.min(progress.completedSteps, progress.totalSteps))
  const total = Math.max(1, progress.totalSteps)
  const percent = Math.round((completed / total) * 100)
  const stage = m.forkStage[progress.stage]
  return <div className="fork-progress-overlay" role="status"
    aria-label={m.fork.label(stage)}
    onPointerDown={(event) => event.stopPropagation()}>
    <div className="fork-progress-overlay__content">
      <span className="fork-progress-overlay__spinner" aria-hidden="true" />
      <strong>{stage}</strong>
      <p>{m.forkStageDescription[progress.stage]}</p>
      <div className="fork-progress-overlay__bar" role="progressbar"
        aria-valuemin={0} aria-valuemax={total} aria-valuenow={completed}>
        <span style={{ width: `${percent}%` }} />
      </div>
      <small>{m.fork.stageCounter(completed + (completed < total ? 1 : 0), total)}</small>
    </div>
  </div>
}

export function activeForkProgress(progress: ForkProgress | undefined): ForkProgress | undefined {
  if (!progress || progress.stage === 'succeeded' || progress.stage === 'failed') return undefined
  return progress
}
