import { useCallback, useEffect, useState, type MouseEvent } from 'react'
import { createPortal } from 'react-dom'

import { TerminalSurface, type TerminalSearchRequest } from '../terminal/TerminalSurface'
import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
import type { SessionView } from './hierarchy-types'
import { sessionDeleteFlow } from './terminal-close-flow'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import { toOscNotification } from '../notifications/osc-notification'
import type { TerminalThemeKey } from '../terminal/terminal-themes'
import { ChildSessionBadge } from '../session-canvas/ChildSessionBadge'
import type { SessionGraphNodeView } from './hierarchy-types'

export function TerminalPane(props: {
  session: SessionView
  active: boolean
  visible?: boolean
  workspaceSessionCount: number
  taskName: string
  workspaceId?: string
  sceneId?: string
  pathValid?: boolean
  themeKey?: TerminalThemeKey
  fontSize?: number
  onFontSizeChange?(fontSize: number): void
  closeRequest?: number
  searchRequest?: TerminalSearchRequest
  onSearchResults?(result: { resultIndex: number; resultCount: number }): void
  focusRequest?: number
  onActivate(sessionId: string): unknown
  onDelete(sessionId: string, confirmed?: boolean): unknown
  resumable?: boolean
  forkReady?: boolean
  providerRestoreState?: 'none' | 'restoring' | 'failed'
  restoreError?: string
  forkState?: 'pending' | 'starting' | 'succeeded' | 'failed'
  forkError?: string
  spawnRevision?: number
  onRetryRestore?(sessionId: string): unknown
  onRetryFork?(sessionId: string): unknown
  onRemoveFailedFork?(sessionId: string): unknown
  childNodes?: SessionGraphNodeView[]
  historicalChildCount?: number
  workStatus?: SessionGraphNodeView['workStatus']
  onOpenChildren?(sessionId: string): unknown
  onFork?(sessionId: string): unknown
  onDetach?(sessionId: string): unknown
}) {
  const {
    session, active, visible = true, workspaceSessionCount, taskName,
    pathValid = true, workspaceId, sceneId, resumable = false, forkReady,
    providerRestoreState = 'none', restoreError, forkState, forkError,
    spawnRevision = 0, onRetryRestore, onRetryFork, onRemoveFailedFork,
    childNodes = [], historicalChildCount = 0, workStatus = 'idle', onOpenChildren,
    themeKey = 'light', fontSize = 11, onFontSizeChange, closeRequest = 0,
    searchRequest, onSearchResults, focusRequest = 0,
    onActivate, onDelete, onFork, onDetach
  } = props
  const [confirmationOpen, setConfirmationOpen] = useState(false)
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null)
  const notificationStore = useNotificationStore()
  useNotificationSnapshot()
  const flow = sessionDeleteFlow({
    isWorkspaceFinal: workspaceSessionCount === 1,
    taskName, sessionTitle: session.title, workStatus,
    childCount: childNodes.length + historicalChildCount
  })
  const profile = session.kind === 'claude-code' || session.kind === 'codex'
    ? session.kind : 'shell'
  const remove = useCallback((confirmed: boolean) => {
    setConfirmationOpen(false)
    void Promise.resolve(onDelete(session.id, confirmed)).catch(NOOP)
  }, [onDelete, session.id])
  const requestRemove = useCallback(() => {
    if (flow.action === 'silent') remove(false)
    else setConfirmationOpen(true)
  }, [flow.action, remove])
  useEffect(() => {
    if (closeRequest > 0) requestRemove()
  }, [closeRequest, requestRemove])
  const hasNotification = notificationStore.sessionHasVisibleIndicator(session.id)
  const showFork = session.kind === 'claude-code' && onFork !== undefined
  const canFork = showFork && (forkReady ?? resumable)
  const hasPaneMenu = canFork || onDetach !== undefined
  const openPaneMenu = (event: MouseEvent<HTMLElement>) => {
    if (!hasPaneMenu || (event.target as HTMLElement).closest('button')) return
    event.preventDefault()
    event.stopPropagation()
    setContextMenu({ x: event.clientX, y: event.clientY })
  }
  return <section className={`terminal-pane split-leaf${active ? ' active-pane' : ''}${hasNotification ? ' has-notification' : ''}`} data-testid="terminal-pane"
    data-active={active} hidden={!visible} onContextMenu={openPaneMenu} onPointerDown={(event) => {
      if ((event.target as HTMLElement).closest('button,[role="menuitem"]')) return
      notificationStore.dismissSessionIndicator(session.id)
      if (!active) onActivate(session.id)
    }}>
    <header className="terminal-pane-header split-pane-header" draggable={onDetach !== undefined}
      onDragEnd={(event) => {
        const outside = event.screenX <= window.screenX || event.screenY <= window.screenY ||
          event.screenX >= window.screenX + window.outerWidth ||
          event.screenY >= window.screenY + window.outerHeight
        if (outside) void onDetach?.(session.id)
      }}>
      <div className="pane-header-content"><strong className="pane-title">{session.title}</strong>
        {onOpenChildren && <ChildSessionBadge children={childNodes}
          historicalCount={historicalChildCount}
          onOpen={() => void onOpenChildren(session.id)} />}
      </div>
      <div className="terminal-pane-actions">
        {showFork && <button className="pane-fork" type="button" draggable={false}
          aria-label={`从“${session.title}”创建子分支`} disabled={!canFork}
          title={canFork ? '创建子分支' : '完成首轮对话后可创建分支'}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            void onFork?.(session.id)
          }}>⑂</button>}
        <button className="pane-close" draggable={false} aria-label={`删除终端：${session.title}`}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }} onClick={(event) => {
          event.stopPropagation()
          requestRemove()
        }}>×</button>
      </div>
    </header>
    {!pathValid && visible && <div role="status">工作区目录不可用，请先在本地恢复原路径，或移出该工作区</div>}
    {forkState === 'failed' && visible && <div className="fork-failure-card" role="status">
      <div><strong>分支创建失败</strong>
        {forkError && <span className="fork-failure-reason">{forkError}</span>}
      </div>
      <div className="fork-failure-actions">
        {onRetryFork && <button type="button" aria-label="重试创建分支" onClick={(event) => {
          event.stopPropagation()
          void onRetryFork(session.id)
        }}>重试</button>}
        {onRemoveFailedFork && <button type="button" aria-label="移除失败分支" onClick={(event) => {
          event.stopPropagation()
          void onRemoveFailedFork(session.id)
        }}>移除</button>}
      </div>
    </div>}
    {providerRestoreState === 'failed' && forkState !== 'failed' && visible && <div className="provider-restore-banner" role="status">
      <div><strong>Claude Code 恢复失败</strong>
        {restoreError && <span className="provider-restore-reason">{restoreError}</span>}
      </div>
      {onRetryRestore && <button type="button" onClick={(event) => {
        event.stopPropagation()
        void onRetryRestore(session.id)
      }}>重试恢复</button>}
    </div>}
    {providerRestoreState === 'restoring' && forkState !== 'failed' && visible && <div className="provider-restore-banner restoring" role="status">
      <strong>正在恢复 Claude Code 会话…</strong>
    </div>}
    {forkState !== 'failed' && <TerminalSurface sessionId={session.id}
      executionContextId={session.executionContextId ?? 'local-default'}
      profile={profile} visible={visible} active={active} inputDisabled={!pathValid}
      themeKey={themeKey} fontSize={fontSize}
      {...(onFontSizeChange ? { onFontSizeChange } : {})}
      {...(searchRequest ? { searchRequest } : {})}
      {...(onSearchResults ? { onSearchResults } : {})}
      focusRequest={focusRequest}
      spawnRevision={spawnRevision}
      onOscNotification={(oscId, content) => {
        const notification = toOscNotification(oscId, content)
        if (!notification) return
        notificationStore.push({
          ...notification, eventId: `osc-${crypto.randomUUID()}`,
          workspaceId: workspaceId ?? null, taskId: session.taskId, sceneId: sceneId ?? null, sessionId: session.id,
          isFocusedSession: active && visible
        })
      }} />}
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
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>⑂ Fork 会话</button>}
        {onDetach && <button className="detach-menu-item" role="menuitem" onClick={() => {
          setContextMenu(null)
          void onDetach(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>↗ 独立窗口</button>}
      </div>
    </>, document.body)}
  </section>
}

function NOOP(): void {}
