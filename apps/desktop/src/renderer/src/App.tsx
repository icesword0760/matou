import { useEffect, useRef, useState } from 'react'

import { HierarchyShell } from './hierarchy/HierarchyShell'
import { DetachedTerminalApp } from './hierarchy/DetachedTerminalApp'
import type { RuntimeStatus } from './terminal/TerminalSurface'
import { DagWindowApp } from './dag/DagWindowApp'
import { DatabaseRecoveryPage } from './recovery/DatabaseRecoveryPage'
import type { RuntimeLifecyclePresentation } from '../../shared/desktop-api'

export function App() {
  const [status, setStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [smokeMarker, setSmokeMarker] = useState('')
  const [replayMarker, setReplayMarker] = useState('')
  const [lifecycle, setLifecycle] = useState<RuntimeLifecyclePresentation>()
  const e2e = new URLSearchParams(window.location.search).get('e2e') === '1'
  const terminalDiagnostics = new URLSearchParams(window.location.search)
    .get('terminalDiagnostics') === '1'
  const detached = new URLSearchParams(window.location.search).get('kind') === 'detached-terminal'
  const dag = new URLSearchParams(window.location.search).get('kind') === 'dag'
  const lastReadyLifecycle = useRef<RuntimeLifecyclePresentation | undefined>(undefined)

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

  if (lifecycle?.snapshot.stage === 'ready') lastReadyLifecycle.current = lifecycle
  const presentedLifecycle = lifecycle?.snapshot.stage !== 'ready' &&
    lifecycle?.snapshot.mode === 'normal' && !lifecycle.recovery &&
    !lifecycle.startupFailure && lastReadyLifecycle.current
    ? lastReadyLifecycle.current
    : lifecycle

  if (!presentedLifecycle || presentedLifecycle.snapshot.stage !== 'ready') {
    if (presentedLifecycle?.startupFailure) {
      return <RuntimeStartupFailurePage
        state={presentedLifecycle}
        retry={() => window.matouDesktop.retryRuntimeStart()}
      />
    }
    if (presentedLifecycle?.recovery || presentedLifecycle?.snapshot.mode === 'recovery-required') {
      return <DatabaseRecoveryPage state={presentedLifecycle} actions={{
        restore: (backupId, expectedRecoveryId) =>
          window.matouDesktop.restoreDatabaseBackup(backupId, expectedRecoveryId),
        exportBundle: () => window.matouDesktop.exportDatabaseRecoveryBundle(),
        retry: (expectedRecoveryId) => window.matouDesktop.retryDatabaseOpen(expectedRecoveryId),
        startEmpty: (expectedRecoveryId) =>
          window.matouDesktop.startWithEmptyDatabase(expectedRecoveryId)
      }} />
    }
    return <main className="hierarchy-loading" aria-busy="true" />
  }

  if (detached) return <DetachedTerminalApp runtimeMode={presentedLifecycle.snapshot.mode} />
  if (dag) return <DagWindowApp runtimeMode={presentedLifecycle.snapshot.mode} />

  return <>
    <HierarchyShell runtimeMode={presentedLifecycle.snapshot.mode}
      {...(e2e && terminalDiagnostics ? { terminalDiagnostics: {
        onStatusChange: setStatus,
        onSmokeMarker: setSmokeMarker,
        onReplayComplete: setReplayMarker
      } } : {})} />
    {e2e && terminalDiagnostics && <div className="e2e-diagnostics" aria-hidden="true">
      <output data-testid="runtime-status">{status}</output>
      <output data-testid="smoke-marker">{smokeMarker}</output>
      <output data-testid="replay-marker">{replayMarker}</output>
    </div>}
  </>
}

function RuntimeStartupFailurePage({ state, retry }: {
  state: RuntimeLifecyclePresentation
  retry(): Promise<void>
}) {
  const [pending, setPending] = useState(false)
  const [retryError, setRetryError] = useState('')
  const failure = state.startupFailure!
  const title = failure.code === 'DATABASE_SCHEMA_UNSUPPORTED'
    ? '需要更新 Matou'
    : '工作区升级未完成'
  return <main className="database-recovery-page" aria-labelledby="runtime-startup-failure-title">
    <section className="database-recovery-card">
      <header>
        <p className="database-recovery-eyebrow">Matou 启动检查</p>
        <h1 id="runtime-startup-failure-title">{title}</h1>
        <p>Matou 已停止重复启动，原数据保持原样。</p>
      </header>
      <p role="alert" className="database-recovery-error">{failure.message}</p>
      {retryError && <p role="alert" className="database-recovery-error">{retryError}</p>}
      <div className="database-recovery-primary-actions">
        <button className="primary" disabled={pending} onClick={() => {
          if (pending) return
          setPending(true)
          setRetryError('')
          void retry().catch((error: unknown) => {
            setRetryError(error instanceof Error ? error.message : String(error))
          }).finally(() => setPending(false))
        }}>{pending ? '正在检查…' : '重新检查'}</button>
      </div>
    </section>
  </main>
}
