import { useEffect, useState } from 'react'

import { HierarchyShell } from './hierarchy/HierarchyShell'
import { DetachedTerminalApp } from './hierarchy/DetachedTerminalApp'
import { TerminalSurface, type RuntimeStatus } from './terminal/TerminalSurface'
import { DagWindowApp } from './dag/DagWindowApp'
import { DatabaseRecoveryPage } from './recovery/DatabaseRecoveryPage'
import type { RuntimeLifecyclePresentation } from '../../shared/desktop-api'

export function App() {
  const [status, setStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [smokeMarker, setSmokeMarker] = useState('')
  const [replayMarker, setReplayMarker] = useState('')
  const [lifecycle, setLifecycle] = useState<RuntimeLifecyclePresentation>()
  const e2e = new URLSearchParams(window.location.search).get('e2e') === '1'
  const detached = new URLSearchParams(window.location.search).get('kind') === 'detached-terminal'
  const dag = new URLSearchParams(window.location.search).get('kind') === 'dag'

  useEffect(() => {
    let active = true
    void window.matouDesktop.getRuntimeLifecycle().then((value) => {
      if (active) setLifecycle(value)
    })
    const unsubscribe = window.matouDesktop.onRuntimeLifecycle((value) => {
      if (active) setLifecycle(value)
    })
    return () => { active = false; unsubscribe() }
  }, [])

  if (!lifecycle || lifecycle.snapshot.stage !== 'ready') {
    if (lifecycle?.recovery || lifecycle?.snapshot.mode === 'recovery-required') {
      return <DatabaseRecoveryPage state={lifecycle} actions={{
        restore: (backupId, expectedRecoveryId) =>
          window.matouDesktop.restoreDatabaseBackup(backupId, expectedRecoveryId),
        exportBundle: () => window.matouDesktop.exportDatabaseRecoveryBundle(),
        retry: (expectedRecoveryId) => window.matouDesktop.retryDatabaseOpen(expectedRecoveryId),
        startEmpty: (expectedRecoveryId) =>
          window.matouDesktop.startWithEmptyDatabase(expectedRecoveryId)
      }} />
    }
    return <main className="database-recovery-page" aria-label="正在打开 Matou 数据库">
      <section className="database-recovery-card">
        <p className="database-recovery-eyebrow">Matou</p>
        <h1>正在打开工作区…</h1>
        <p>正在检查本地数据并恢复上次状态。</p>
      </section>
    </main>
  }

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
