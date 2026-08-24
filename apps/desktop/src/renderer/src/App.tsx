import { useState } from 'react'

import { TerminalSurface, type RuntimeStatus } from './terminal/TerminalSurface'

export function App() {
  const [status, setStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [smokeMarker, setSmokeMarker] = useState('')
  const [replayMarker, setReplayMarker] = useState('')

  return (
    <main className="app-shell">
      <header className="app-header">
        <div>
          <p className="eyebrow">MATOU / TERMINAL FOUNDATION</p>
          <h1>Direct Runtime Channel</h1>
        </div>
        <output className="runtime-status" data-testid="runtime-status">
          {status}
        </output>
      </header>

      <section className="terminal-panel" aria-label="Terminal session">
        <TerminalSurface
          onStatusChange={setStatus}
          onSmokeMarker={setSmokeMarker}
          onReplayComplete={setReplayMarker}
        />
      </section>

      <output className="smoke-marker" data-testid="smoke-marker">
        {smokeMarker}
      </output>
      <output className="smoke-marker" data-testid="replay-marker">
        {replayMarker}
      </output>
    </main>
  )
}
