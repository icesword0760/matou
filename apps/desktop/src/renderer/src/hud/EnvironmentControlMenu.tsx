import { useState } from 'react'

import type {
  SessionEnvironmentActionResult,
  SessionEnvironmentOpenResult,
  SessionEnvironmentTarget
} from '@matou/contracts'
import type { SessionEnvironment } from '@matou/domain'

import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'

type HudMessages = Messages['hud']

export interface SessionEnvironmentActions {
  open(sessionId: string): Promise<SessionEnvironmentOpenResult>
  restore(sessionId: string): Promise<SessionEnvironmentActionResult>
  locate(sessionId: string, path: string): Promise<SessionEnvironmentActionResult>
  handoff(sessionId: string, target: SessionEnvironmentTarget): Promise<SessionEnvironmentActionResult>
}

export function EnvironmentControlMenu(props: {
  sessionId: string
  environment: SessionEnvironment
  hasOwnedWorktree: boolean
  actions: SessionEnvironmentActions
  mutationDisabledReason?: string
  onClose(): void
}) {
  const { environment } = props
  const messages = useMessages()
  const m = messages.hud
  const overlay = messages.hierarchyTerminal.environmentOverlay
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const mutationDisabled = Boolean(props.mutationDisabledReason) || Boolean(busy)

  const run = async (label: string, action: () => Promise<unknown>) => {
    if (busy) return
    setBusy(label)
    setError('')
    try {
      await action()
      props.onClose()
    } catch (reason) {
      setError(environmentErrorText(reason, m, messages.hierarchyTerminal.pane.environmentActionFailed))
    } finally {
      setBusy('')
    }
  }
  const open = async (kind: 'finder' | 'terminal') => {
    await run(kind === 'finder' ? m.openingFinder : m.openingSystemTerminal, async () => {
      const result = await props.actions.open(props.sessionId)
      if (kind === 'finder') await window.matouDesktop?.revealDirectory(result.path)
      else await window.matouDesktop?.openDirectoryInTerminal(result.path)
    })
  }
  const locate = async () => {
    if (mutationDisabled) return
    const path = await window.matouDesktop?.selectSessionEnvironmentDirectory()
    if (!path) return
    await run(m.confirmingWorktree, async () => {
      const result = await props.actions.locate(props.sessionId, path)
      if (result.kind === 'rejected') throw new Error(locateReasonText(result.reason, m))
    })
  }
  const restore = () => run(m.restoringWorktree, async () => {
    const result = await props.actions.restore(props.sessionId)
    if (result.kind === 'rejected') throw new Error(locateReasonText(result.reason, m))
  })
  const handoff = (target: SessionEnvironmentTarget) => run(
    target === 'local' ? overlay.handingOffToLocal : overlay.handingOffToWorktree,
    async () => {
      const result = await props.actions.handoff(props.sessionId, target)
      if (result.kind === 'rejected') throw new Error(locateReasonText(result.reason, m))
    }
  )

  const ready = environment.state === 'ready'
  const worktreeNeedsRecovery = environment.kind === 'worktree' &&
    (environment.state === 'missing' || environment.state === 'failed')

  return <div className="environment-menu-overlay" onPointerDown={(event) => {
    if (event.currentTarget === event.target) props.onClose()
  }}>
    <section className="environment-control-menu" role="dialog" aria-label={m.environmentMenu}>
      <header className="environment-control-menu__header">
        <div><span aria-hidden="true">▣</span><strong>{environmentLabel(environment, m)}</strong></div>
        <button type="button" aria-label={m.closeEnvironmentMenu} onClick={props.onClose}>×</button>
      </header>
      <div className="environment-control-menu__body">
        <div className="environment-path"><span>{m.currentPath}</span><code title={environment.path}>{environment.path}</code></div>
        {environment.error && <div className="environment-warning" role="status">{environmentReasonText(environment.error, m)}</div>}
        <div className="environment-action-grid">
          <button type="button" disabled={!ready || Boolean(busy)} onClick={() => void open('finder')}>{messages.hierarchyShell.taskSidebar.revealInFinder}</button>
          <button type="button" disabled={!ready || Boolean(busy)} onClick={() => void open('terminal')}>{m.openInSystemTerminal}</button>
        </div>
        {worktreeNeedsRecovery && <div className="environment-action-stack">
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void restore()}>{m.restoreOriginalWorktree}</button>
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void locate()}>{m.locateMovedWorktree}</button>
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void handoff('local')}>{overlay.handoffToLocal}</button>
        </div>}
        {ready && environment.kind === 'worktree' && <div className="environment-action-stack">
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void handoff('local')}>{overlay.handoffToLocal}</button>
        </div>}
        {ready && environment.kind === 'local' && props.hasOwnedWorktree && <div className="environment-action-stack">
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void handoff('worktree')}>{m.handoffToOwnedWorktree}</button>
        </div>}
        {environment.kind === 'local' && environment.state === 'failed' && props.hasOwnedWorktree &&
          <div className="environment-action-stack">
            <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
              onClick={() => void handoff('worktree')}>{m.handoffToOwnedWorktree}</button>
          </div>}
      </div>
      {(busy || error) && <footer className={`environment-control-menu__feedback${error ? ' is-error' : ''}`} role="status">
        {error || busy}
      </footer>}
    </section>
  </div>
}

export function environmentLabel(environment: SessionEnvironment, m: HudMessages): string {
  if (environment.state === 'missing') return m.environmentState.missing
  if (environment.state === 'recovering') return m.environmentState.recovering
  if (environment.state === 'handoff') return m.environmentState.handoff
  if (environment.state === 'failed') return m.environmentState.failed
  return environment.kind === 'worktree' ? m.environmentState.worktree : m.environmentState.local
}

function locateReasonText(reason: string, m: HudMessages): string {
  const labels: Record<string, string> = m.locateReason
  return labels[reason] ?? reason
}

function environmentReasonText(reason: string, m: HudMessages): string {
  return locateReasonText(reason.replace(/^(restore|locate)-failed:/, ''), m)
}

function environmentErrorText(reason: unknown, m: HudMessages, fallback: string): string {
  return reason instanceof Error ? environmentReasonText(reason.message, m) : fallback
}
