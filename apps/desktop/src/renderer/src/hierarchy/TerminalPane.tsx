import { useState } from 'react'

import { TerminalSurface } from '../terminal/TerminalSurface'
import { ConfirmationSequence } from './ConfirmDialog'
import type { SessionView } from './hierarchy-types'
import { sessionDeleteFlow } from './terminal-close-flow'

export function TerminalPane(props: {
  session: SessionView
  active: boolean
  visible?: boolean
  workspaceSessionCount: number
  taskName: string
  pathValid?: boolean
  onActivate(sessionId: string): unknown
  onDelete(sessionId: string, confirmed?: boolean): unknown
}) {
  const {
    session, active, visible = true, workspaceSessionCount, taskName,
    pathValid = true, onActivate, onDelete
  } = props
  const [confirmationOpen, setConfirmationOpen] = useState(false)
  const flow = sessionDeleteFlow({
    isWorkspaceFinal: workspaceSessionCount === 1,
    taskName
  })
  const profile = session.kind === 'claude-code' || session.kind === 'codex'
    ? session.kind : 'shell'
  const remove = (confirmed: boolean) => {
    setConfirmationOpen(false)
    void Promise.resolve(onDelete(session.id, confirmed)).catch(NOOP)
  }
  return <section className="terminal-pane" data-testid="terminal-pane"
    data-active={active} hidden={!visible} onPointerDown={() => onActivate(session.id)}>
    <header className="terminal-pane-header">
      <div><strong>{session.title}</strong><span>{typeLabel(profile)}</span></div>
      <button aria-label={`删除终端：${session.title}`} onClick={(event) => {
        event.stopPropagation()
        if (flow.action === 'silent') remove(false)
        else setConfirmationOpen(true)
      }}>×</button>
    </header>
    {!pathValid && <div role="status">工作区目录不可用</div>}
    <TerminalSurface sessionId={session.id}
      executionContextId={session.executionContextId ?? 'local-default'}
      profile={profile} visible={visible} inputDisabled={!pathValid} />
    {confirmationOpen && <ConfirmationSequence steps={flow.steps}
      onCancel={() => setConfirmationOpen(false)} onComplete={() => remove(true)} />}
  </section>
}

function typeLabel(profile: 'shell' | 'claude-code' | 'codex'): string {
  if (profile === 'claude-code') return 'Claude Code'
  if (profile === 'codex') return 'Codex'
  return 'Shell'
}

function NOOP(): void {}
