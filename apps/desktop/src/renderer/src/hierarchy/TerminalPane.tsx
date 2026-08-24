import { useState } from 'react'

import { TerminalSurface } from '../terminal/TerminalSurface'
import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
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
  return <section className={`terminal-pane split-leaf${active ? ' active-pane' : ''}`} data-testid="terminal-pane"
    data-active={active} hidden={!visible} onPointerDown={() => onActivate(session.id)}>
    <header className="terminal-pane-header split-pane-header" draggable={onDetach !== undefined}
      onDragEnd={(event) => {
        const outside = event.screenX <= window.screenX || event.screenY <= window.screenY ||
          event.screenX >= window.screenX + window.outerWidth ||
          event.screenY >= window.screenY + window.outerHeight
        if (outside) void onDetach?.(session.id)
      }}>
      <div className="pane-header-content"><strong className="pane-title">{session.title}</strong></div>
      <div className="terminal-pane-actions">
        <button className="pane-close" aria-label={`删除终端：${session.title}`} onClick={(event) => {
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
    {confirmationOpen && flow.action === 'hide-window' && <ConfirmDialog title="提示"
      body={'当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。'}
      confirmLabel="我知道了" showCancel={false} onCancel={() => setConfirmationOpen(false)}
      onConfirm={() => setConfirmationOpen(false)} />}
    {confirmationOpen && flow.action !== 'hide-window' && <ConfirmationSequence steps={flow.steps}
      onCancel={() => setConfirmationOpen(false)} onComplete={() => remove(true)} />}
  </section>
}

function NOOP(): void {}
