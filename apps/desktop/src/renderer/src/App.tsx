import { useState } from 'react'

import { HierarchyShell } from './hierarchy/HierarchyShell'
import { DetachedTerminalApp } from './hierarchy/DetachedTerminalApp'
import { TerminalSurface, type RuntimeStatus } from './terminal/TerminalSurface'
import { DagWindowApp } from './dag/DagWindowApp'

export function App() {
  const [status, setStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [smokeMarker, setSmokeMarker] = useState('')
  const [replayMarker, setReplayMarker] = useState('')
  const e2e = new URLSearchParams(window.location.search).get('e2e') === '1'
  const detached = new URLSearchParams(window.location.search).get('kind') === 'detached-terminal'
  const dag = new URLSearchParams(window.location.search).get('kind') === 'dag'

  if (detached) return <DetachedTerminalApp />
  if (dag) return <DagWindowApp />

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
