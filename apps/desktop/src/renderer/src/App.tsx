import { useEffect, useState } from 'react'

import { HierarchyShell } from './hierarchy/HierarchyShell'
import { DetachedTerminalApp } from './hierarchy/DetachedTerminalApp'
import { TerminalSurface, type RuntimeStatus } from './terminal/TerminalSurface'

export function App() {
  const [status, setStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [smokeMarker, setSmokeMarker] = useState('')
  const [replayMarker, setReplayMarker] = useState('')
  const e2e = new URLSearchParams(window.location.search).get('e2e') === '1'
  const detached = new URLSearchParams(window.location.search).get('kind') === 'detached-terminal'
  const dag = new URLSearchParams(window.location.search).get('kind') === 'dag'

  if (detached) return <DetachedTerminalApp />
  if (dag) return <DagWindowLoading />

  return <>
    <HierarchyShell />
    {e2e && <div className="e2e-diagnostics" aria-hidden="true">
      <TerminalSurface onStatusChange={setStatus}
        onSmokeMarker={setSmokeMarker} onReplayComplete={setReplayMarker} />
      <output data-testid="runtime-status">{status}</output>
      <output data-testid="smoke-marker">{smokeMarker}</output>
      <output data-testid="replay-marker">{replayMarker}</output>
    </div>}
  </>
}

function DagWindowLoading() {
  const mainWindowId = new URLSearchParams(window.location.search).get('mainWindowId') ?? ''
  useEffect(() => {
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') void window.matouDesktop?.closeDagWindow?.(mainWindowId)
    }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [mainWindowId])
  return <main className="dag-window-loading" aria-label="会话 DAG" aria-busy="true">正在载入会话关系…</main>
}
