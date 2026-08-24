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
  onDetach?(sessionId: string): unknown
}) {
  const {
    session, active, visible = true, workspaceSessionCount, taskName,
    pathValid = true, onActivate, onDelete, onDetach
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
    <header className="terminal-pane-header" draggable={onDetach !== undefined}
      onDragEnd={(event) => {
        const outside = event.screenX <= window.screenX || event.screenY <= window.screenY ||
          event.screenX >= window.screenX + window.outerWidth ||
          event.screenY >= window.screenY + window.outerHeight
        if (outside) void onDetach?.(session.id)
      }}>
      <div><strong>{session.title}</strong><span>{typeLabel(profile)}</span></div>
      <div className="terminal-pane-actions">
        {onDetach && <button aria-label={`脱出终端：${session.title}`} onClick={(event) => {
          event.stopPropagation(); void onDetach(session.id)
        }}>↗</button>}
        <button aria-label={`删除终端：${session.title}`} onClick={(event) => {
          event.stopPropagation()
          if (flow.action === 'silent') remove(false)
          else setConfirmationOpen(true)
        }}>×</button>
      </div>
    </header>
    {!pathValid && visible && <div role="status">工作区目录不可用，请先在本地恢复原路径，或移出该工作区</div>}
    <TerminalSurface sessionId={session.id}
      executionContextId={session.executionContextId ?? 'local-default'}
      profile={profile} visible={visible} active={active} inputDisabled={!pathValid} />
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
