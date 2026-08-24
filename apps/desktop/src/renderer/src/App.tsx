import { useState } from 'react'

import { HierarchyShell } from './hierarchy/HierarchyShell'
import { TerminalSurface, type RuntimeStatus } from './terminal/TerminalSurface'

export function App() {
  const [status, setStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [smokeMarker, setSmokeMarker] = useState('')
  const [replayMarker, setReplayMarker] = useState('')
  const e2e = new URLSearchParams(window.location.search).get('e2e') === '1'

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
