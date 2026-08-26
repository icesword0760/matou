import { useState, type MouseEvent } from 'react'
import { createPortal } from 'react-dom'

import { TerminalSurface } from '../terminal/TerminalSurface'
import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
import type { SessionView } from './hierarchy-types'
import { sessionDeleteFlow } from './terminal-close-flow'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import { toOscNotification } from '../notifications/osc-notification'

export function TerminalPane(props: {
  session: SessionView
  active: boolean
  visible?: boolean
  workspaceSessionCount: number
  taskName: string
  workspaceId?: string
  sceneId?: string
  pathValid?: boolean
  onActivate(sessionId: string): unknown
  onDelete(sessionId: string, confirmed?: boolean): unknown
  resumable?: boolean
  onFork?(sessionId: string): unknown
  onDetach?(sessionId: string): unknown
}) {
  const {
    session, active, visible = true, workspaceSessionCount, taskName,
    pathValid = true, workspaceId, sceneId, resumable = false,
    onActivate, onDelete, onFork, onDetach
  } = props
  const [confirmationOpen, setConfirmationOpen] = useState(false)
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null)
  const notificationStore = useNotificationStore()
  useNotificationSnapshot()
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
  const hasNotification = notificationStore.sessionHasVisibleIndicator(session.id)
  const canFork = session.kind === 'claude-code' && resumable && onFork !== undefined
  const hasPaneMenu = canFork || onDetach !== undefined
  const openPaneMenu = (event: MouseEvent<HTMLElement>) => {
    if (!hasPaneMenu || (event.target as HTMLElement).closest('button')) return
    event.preventDefault()
    event.stopPropagation()
    setContextMenu({ x: event.clientX, y: event.clientY })
  }
  return <section className={`terminal-pane split-leaf${active ? ' active-pane' : ''}${hasNotification ? ' has-notification' : ''}`} data-testid="terminal-pane"
    data-active={active} hidden={!visible} onContextMenu={openPaneMenu} onPointerDown={() => {
      notificationStore.dismissSessionIndicator(session.id)
      onActivate(session.id)
    }}>
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
      profile={profile} visible={visible} active={active} inputDisabled={!pathValid}
      onOscNotification={(oscId, content) => {
        const notification = toOscNotification(oscId, content)
        if (!notification) return
        notificationStore.push({
          ...notification, eventId: `osc-${crypto.randomUUID()}`,
          workspaceId: workspaceId ?? null, taskId: session.taskId, sceneId: sceneId ?? null, sessionId: session.id,
          isFocusedSession: active && visible
        })
      }} />
    {confirmationOpen && flow.action === 'hide-window' && <ConfirmDialog title="提示"
      body={'当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。'}
      confirmLabel="我知道了" showCancel={false} onCancel={() => setConfirmationOpen(false)}
      onConfirm={() => setConfirmationOpen(false)} />}
    {confirmationOpen && flow.action !== 'hide-window' && <ConfirmationSequence steps={flow.steps}
      onCancel={() => setConfirmationOpen(false)} onComplete={() => remove(true)} />}
    {contextMenu && createPortal(<>
      <div className="detach-context-overlay" onClick={() => setContextMenu(null)}
        onContextMenu={(event) => { event.preventDefault(); setContextMenu(null) }} />
      <div className="detach-context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }}
        onClick={(event) => event.stopPropagation()}>
        {canFork && <button className="detach-menu-item" role="menuitem" onClick={() => {
          setContextMenu(null)
          void onFork?.(session.id)
        }}>⑂ Fork 会话</button>}
        {onDetach && <button className="detach-menu-item" role="menuitem" onClick={() => {
          setContextMenu(null)
          void onDetach(session.id)
        }}>↗ 独立窗口</button>}
      </div>
    </>, document.body)}
  </section>
}

function NOOP(): void {}
