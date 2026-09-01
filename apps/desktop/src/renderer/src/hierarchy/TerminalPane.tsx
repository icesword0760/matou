import { useCallback, useEffect, useRef, useState, type MouseEvent } from 'react'
import { createPortal } from 'react-dom'

import {
  TerminalSurface,
  type RuntimeStatus,
  type TerminalSearchRequest,
  type TerminalStorageFaultMessage
} from '../terminal/TerminalSurface'
import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
import type { SessionView } from './hierarchy-types'
import { sessionDeleteFlow } from './terminal-close-flow'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import { toOscNotification } from '../notifications/osc-notification'
import type { TerminalThemeKey } from '../terminal/terminal-themes'
import { ChildSessionBadge } from '../session-canvas/ChildSessionBadge'
import type { SessionGraphNodeView } from './hierarchy-types'
import { AgentTeamMemberSummary } from './AgentTeamMemberSummary'
import type { SessionEnvironment, SessionGitState } from '@matou/domain'
import type { SessionEnvironmentTarget } from '@matou/contracts'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { StorageFaultOverlay } from './StorageFaultOverlay'
import {
  activeForkProgress,
  ForkProgressOverlay
} from '../session-canvas/ForkProgressOverlay'
import '../session-canvas/fork-progress-overlay.css'

export function TerminalPane(props: {
  session: SessionView
  active: boolean
  visible?: boolean
  foreground?: boolean
  workspaceSessionCount: number
  taskName: string
  workspaceId?: string
  sceneId?: string
  pathValid?: boolean
  readOnly?: boolean
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
  forkProgress?: import('@matou/domain').ForkProgress
  cwd?: string
  git?: SessionGitState
  sharedWorkingDirectory?: boolean
  environment?: SessionEnvironment
  hasOwnedWorktree?: boolean
  spawnRevision?: number
  onRetryRestore?(sessionId: string): unknown
  onRetryWork?(sessionId: string): unknown
  onRetryFork?(sessionId: string): unknown
  onRemoveFailedFork?(sessionId: string): unknown
  childNodes?: SessionGraphNodeView[]
  workStatus?: SessionGraphNodeView['workStatus']
  latestLines?: string[]
  onOpenChildren?(sessionId: string): unknown
  onLoadSession?(sessionId: string): unknown
  onFork?(sessionId: string): unknown
  onForkSibling?(sessionId: string): unknown
  onDetach?(sessionId: string): unknown
  descendantCount?: number
  descendantImpact?: { running: number; needsInput: number }
  onRemoveBranch?(sessionId: string, includeDescendants: boolean): unknown
  onRestoreEnvironment?(sessionId: string): unknown
  onLocateEnvironment?(sessionId: string): unknown
  onHandoffEnvironment?(sessionId: string, target: SessionEnvironmentTarget): unknown
}) {
  const {
    session, active, visible = true, foreground = true, workspaceSessionCount, taskName,
    pathValid = true, readOnly = false, workspaceId, sceneId, resumable = false, forkReady,
    providerRestoreState = 'none', restoreError, forkState, forkError, forkProgress, cwd, git,
    sharedWorkingDirectory = false, environment, hasOwnedWorktree = false,
    spawnRevision = 0, onRetryRestore, onRetryWork, onRetryFork, onRemoveFailedFork,
    childNodes = [], workStatus = 'idle', latestLines = [], onOpenChildren, onLoadSession,
    themeKey = 'light', fontSize = 11, onFontSizeChange, closeRequest = 0,
    searchRequest, onSearchResults, focusRequest = 0,
    onActivate, onDelete, onFork, onForkSibling, onDetach,
    descendantCount = 0, descendantImpact = { running: 0, needsInput: 0 },
    onRemoveBranch, onRestoreEnvironment, onLocateEnvironment, onHandoffEnvironment
  } = props
  const [confirmationOpen, setConfirmationOpen] = useState(false)
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null)
  const contextMenuRef = useRef<HTMLDivElement>(null)
  const [removalOpen, setRemovalOpen] = useState(false)
  const [runtimeStatus, setRuntimeStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [runtimeError, setRuntimeError] = useState('')
  const [storageFault, setStorageFault] = useState<TerminalStorageFaultMessage | null>(null)
  const [startupRetry, setStartupRetry] = useState(0)
  const [restoreRetryPending, setRestoreRetryPending] = useState(false)
  const [dismissedRestoreNotice, setDismissedRestoreNotice] = useState<string | null>(null)
  const [forkReadinessHint, setForkReadinessHint] = useState(false)
  const [environmentAction, setEnvironmentAction] = useState('')
  const [environmentActionError, setEnvironmentActionError] = useState('')
  const previousPathValid = useRef(pathValid)
  const consumedCloseRequest = useRef(0)
  const handleRuntimeStatus = useCallback((status: RuntimeStatus) => {
    setRuntimeStatus(status)
    if (status === 'streaming') setRuntimeError('')
    if (status === 'exited') setStorageFault(null)
  }, [])
  const runtimeClient = useRuntimeClient()
  const notificationStore = useNotificationStore()
  useNotificationSnapshot()
  const flow = sessionDeleteFlow({
    isWorkspaceFinal: workspaceSessionCount === 1,
    taskName, sessionTitle: session.title, workStatus,
    childCount: childNodes.length
  })
  const profile = session.kind === 'claude-code' || session.kind === 'codex'
    ? session.kind : 'shell'
  const showFork = session.kind === 'claude-code' && onFork !== undefined
  const canFork = showFork && (forkReady ?? resumable)
  const environmentUnavailable = environment !== undefined && environment.state !== 'ready'
  const actionBlocked = readOnly || environmentUnavailable
  const actionBlockedReason = readOnly ? READ_ONLY_REASON : environmentUnavailable
    ? '当前运行环境需要先恢复或交接'
    : undefined
  const remove = useCallback((confirmed: boolean) => {
    setConfirmationOpen(false)
    void Promise.resolve(onDelete(session.id, confirmed)).catch(NOOP)
  }, [onDelete, session.id])
  const requestRemove = useCallback(() => {
    if (actionBlocked) return
    if (flow.action === 'silent') remove(false)
    else setConfirmationOpen(true)
  }, [actionBlocked, flow.action, remove])
  useEffect(() => {
    if (closeRequest <= consumedCloseRequest.current) return
    consumedCloseRequest.current = closeRequest
    if (!actionBlocked) requestRemove()
  }, [closeRequest, requestRemove])
  useEffect(() => {
    const restored = !previousPathValid.current && pathValid
    previousPathValid.current = pathValid
    if (!restored) return
    setRuntimeError('')
    setRuntimeStatus('starting-session')
    setStartupRetry((value) => value + 1)
  }, [pathValid])
  useEffect(() => {
    if (!forkReadinessHint) return
    const timer = window.setTimeout(() => setForkReadinessHint(false), 2_800)
    return () => window.clearTimeout(timer)
  }, [forkReadinessHint])
  useEffect(() => { setStorageFault(null) }, [session.id])
  useEffect(() => {
    if (canFork) setForkReadinessHint(false)
  }, [canFork])
  useEffect(() => {
    if (!actionBlocked) return
    setConfirmationOpen(false)
    setRemovalOpen(false)
    setContextMenu(null)
    setForkReadinessHint(false)
  }, [actionBlocked])
  useEffect(() => {
    if (!contextMenu) return
    const closeOutside = (event: Event) => {
      const target = event.target
      if (target instanceof Node && contextMenuRef.current?.contains(target)) return
      setContextMenu(null)
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setContextMenu(null)
    }
    window.addEventListener('pointerdown', closeOutside, true)
    window.addEventListener('contextmenu', closeOutside, true)
    window.addEventListener('keydown', closeOnEscape)
    return () => {
      window.removeEventListener('pointerdown', closeOutside, true)
      window.removeEventListener('contextmenu', closeOutside, true)
      window.removeEventListener('keydown', closeOnEscape)
    }
  }, [contextMenu])
  const hasNotification = notificationStore.sessionHasVisibleIndicator(session.id)
  const isTeamMember = session.kind === 'agent-team-member'
  // A sibling Fork resumes the common Claude parent, not this card. The
  // caller only supplies the action after validating that parent, so a Shell
  // child still exposes the approved sibling operation.
  const canForkSibling = onForkSibling !== undefined
  const canDetach = onDetach !== undefined
  const forkFailure = forkFailurePresentation(forkError)
  const currentForkProgress = activeForkProgress(forkProgress)
  const restoreIdentityExpired = providerRestoreIdentityExpired(restoreError)
  const restoreNoticeKey = restoreIdentityExpired && providerRestoreState === 'failed'
    ? `${session.id}:${restoreError ?? ''}`
    : null
  const restoreNoticeVisible = restoreNoticeKey === null || dismissedRestoreNotice !== restoreNoticeKey
  const effectiveRestoreState = providerRestoreState === 'failed' && session.kind === 'claude-code'
    ? 'none'
    : providerRestoreState
  const providerWorkFailure = session.kind === 'claude-code' && workStatus === 'error'
    ? claudeWorkFailureReason(latestLines)
    : undefined
  const openPaneMenu = (event: MouseEvent<HTMLElement>) => {
    if (actionBlocked) return
    event.preventDefault()
    event.stopPropagation()
    setContextMenu({ x: event.clientX, y: event.clientY })
  }
  const runEnvironmentAction = async (label: string, action: () => unknown) => {
    if (environmentAction) return
    setEnvironmentAction(label)
    setEnvironmentActionError('')
    try {
      const result = await Promise.resolve(action())
      if (result && typeof result === 'object' && 'kind' in result && result.kind === 'rejected') {
        throw new Error('reason' in result ? String(result.reason) : '运行环境操作未完成')
      }
    } catch (error) {
      setEnvironmentActionError(error instanceof Error ? error.message : '运行环境操作失败')
    } finally {
      setEnvironmentAction('')
    }
  }
  return <section className={`terminal-pane split-leaf${active ? ' active-pane' : ''}${hasNotification ? ' has-notification' : ''}`} data-testid="terminal-pane"
    data-active={active} hidden={!visible}
    onPointerDown={(event) => {
      if ((event.target as HTMLElement).closest('button,[role="menuitem"]')) return
      notificationStore.dismissSessionIndicator(session.id)
      if (!active) onActivate(session.id)
    }}>
    <header className="terminal-pane-header split-pane-header" draggable={!actionBlocked && canDetach}
      onContextMenu={openPaneMenu}
      onDragEnd={(event) => {
        const outside = event.screenX <= window.screenX || event.screenY <= window.screenY ||
          event.screenX >= window.screenX + window.outerWidth ||
          event.screenY >= window.screenY + window.outerHeight
        if (outside && !actionBlocked && canDetach) void onDetach?.(session.id)
      }}>
      <div className="pane-header-content"><strong className="pane-title" title={session.title}>{session.title}</strong>
        {git && <span className="pane-environment-badge" title={gitTitle(git)}>
          {gitLabel(git)}
        </span>}
        {sharedWorkingDirectory && <span className="pane-environment-badge is-shared">
          {git ? '共享工作树' : '共享目录'}
        </span>}
        {cwd && <span className="pane-cwd" title={cwd}>{cwd}</span>}
      </div>
      <div className="terminal-pane-actions">
        {onOpenChildren && <ChildSessionBadge children={childNodes}
          onOpen={() => void onOpenChildren(session.id)} />}
        {onLoadSession && <button className="pane-fork pane-load-session" type="button" draggable={false}
          aria-label={`载入 Claude Code 会话到“${session.title}”`} disabled={actionBlocked}
          title={actionBlockedReason ?? '载入 Claude Code 会话'}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            void onLoadSession(session.id)
          }}><LoadSessionIcon /></button>}
        {showFork && <button className="pane-fork" type="button" draggable={false}
          aria-label={`从“${session.title}”创建子分支`} aria-disabled={actionBlocked || !canFork}
          disabled={actionBlocked}
          title={actionBlockedReason ?? (canFork ? '创建子分支' : '完成首轮对话后可创建分支')}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            if (!canFork) {
              setForkReadinessHint(true)
              return
            }
            void onFork?.(session.id)
          }}><BranchChildIcon /></button>}
        {canForkSibling && <button className="pane-fork pane-fork-sibling" type="button" draggable={false}
          aria-label={`从共同父会话创建“${session.title}”的兄弟分支`} disabled={actionBlocked}
          title={actionBlockedReason ?? '从共同父会话 Fork 兄弟分支'}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            void onForkSibling?.(session.id)
          }}><BranchSiblingIcon /></button>}
        {onRemoveBranch && <button className="pane-fork pane-remove" type="button" draggable={false}
          aria-label={`移出节点：${session.title}`} disabled={actionBlocked}
          title={actionBlockedReason ?? '移出节点'}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            setRemovalOpen(true)
          }}><RemoveNodeIcon /></button>}
      </div>
    </header>
    {!pathValid && visible && <div role="status">工作区目录不可用，请先在本地恢复原路径，或移出该工作区</div>}
    {forkState === 'failed' && visible && <div className="fork-failure-card" role="status">
      <div><strong>{forkFailure.title}</strong>
        {forkFailure.reason && <span className="fork-failure-reason">{forkFailure.reason}</span>}
      </div>
      <div className="fork-failure-actions">
        {onRetryFork && <button type="button" aria-label="重试创建分支" disabled={actionBlocked}
          title={actionBlockedReason} onClick={(event) => {
          event.stopPropagation()
          void onRetryFork(session.id)
        }}>重试</button>}
        {onRemoveFailedFork && <button type="button" aria-label="移除失败分支" disabled={actionBlocked}
          title={actionBlockedReason} onClick={(event) => {
          event.stopPropagation()
          void onRemoveFailedFork(session.id)
        }}>移除</button>}
      </div>
    </div>}
    {effectiveRestoreState === 'failed' && forkState !== 'failed' && visible && restoreNoticeVisible &&
      <div className="provider-restore-banner" role="status">
      <div><strong>{restoreIdentityExpired ? '原 Claude Code 对话已失效' : 'Claude Code 恢复失败'}</strong>
        <span className="provider-restore-reason">{restoreIdentityExpired
          ? '当前已切换到 Shell，可继续使用终端'
          : restoreError}</span>
      </div>
      {onRetryRestore && !restoreIdentityExpired && <button type="button"
        disabled={actionBlocked || restoreRetryPending} title={actionBlockedReason} onClick={(event) => {
        event.stopPropagation()
        if (restoreRetryPending) return
        setRestoreRetryPending(true)
        void Promise.resolve(onRetryRestore(session.id)).finally(() => setRestoreRetryPending(false))
      }}>{restoreRetryPending ? '正在恢复…' : '重试恢复'}</button>}
    </div>}
    {effectiveRestoreState === 'restoring' && forkState !== 'failed' && visible && <div className="provider-restore-banner restoring" role="status">
      <strong>正在恢复 Claude Code 会话…</strong>
    </div>}
    {providerWorkFailure && effectiveRestoreState !== 'failed' && forkState !== 'failed' && visible &&
      <div className="provider-work-failure-banner" role="status" aria-label="Claude Code 任务失败">
        <div><strong>Claude Code 任务失败</strong>
          <span className="provider-work-failure-reason">{providerWorkFailure}</span>
        </div>
        {onRetryWork && <button type="button" aria-label="重试本轮任务" disabled={actionBlocked}
          title={actionBlockedReason} onClick={(event) => {
          event.stopPropagation()
          void onRetryWork(session.id)
        }}>重试</button>}
      </div>}
    {runtimeStatus === 'error' && forkState !== 'failed' && effectiveRestoreState !== 'failed' && visible &&
      <div className="session-start-failure-card" role="status">
        <div><strong>会话启动失败</strong>
          <span className="session-start-failure-reason">{runtimeError || '终端进程未能启动'}</span>
        </div>
        <div className="session-start-failure-actions">
          <button type="button" disabled={actionBlocked} title={actionBlockedReason} onClick={(event) => {
            event.stopPropagation()
            setRuntimeError('')
            setRuntimeStatus('starting-session')
            setStartupRetry((value) => value + 1)
          }}>重试启动</button>
          <button type="button" disabled={actionBlocked} title={actionBlockedReason} onClick={(event) => {
            event.stopPropagation()
            void Promise.resolve(onDelete(session.id, true)).catch(NOOP)
          }}>移除失败会话</button>
        </div>
      </div>}
    {isTeamMember && forkState !== 'failed' && <AgentTeamMemberSummary
      workStatus={workStatus} latestLines={latestLines} />}
    {!isTeamMember && forkState !== 'failed' && <TerminalSurface sessionId={session.id}
      executionContextId={session.executionContextId ?? 'local-default'}
      profile={profile} visible={visible} active={active} foreground={foreground}
      inputDisabled={actionBlocked || !pathValid || storageFault !== null}
      readOnly={actionBlocked}
      themeKey={themeKey} fontSize={fontSize}
      {...(onFontSizeChange ? { onFontSizeChange } : {})}
      {...(searchRequest ? { searchRequest } : {})}
      {...(onSearchResults ? { onSearchResults } : {})}
      focusRequest={focusRequest}
      spawnRevision={spawnRevision + startupRetry}
      onStatusChange={handleRuntimeStatus}
      onRuntimeError={setRuntimeError}
      onStorageFault={setStorageFault}
      onStorageRecovered={() => setStorageFault(null)}
      onUserInput={() => {
        if (restoreNoticeKey !== null) setDismissedRestoreNotice(restoreNoticeKey)
      }}
      onOscNotification={(oscId, content) => {
        const notification = toOscNotification(oscId, content)
        if (!notification) return
        notificationStore.push({
          ...notification, eventId: `osc-${crypto.randomUUID()}`,
          workspaceId: workspaceId ?? null, taskId: session.taskId, sceneId: sceneId ?? null, sessionId: session.id,
          isFocusedSession: active && visible
        })
      }} />}
    {storageFault && visible && <StorageFaultOverlay
      sessionTitle={session.title}
      fault={{
        code: storageFault.code,
        retainedBytes: storageFault.retainedBytes,
        message: storageFault.message
      }}
      onRetry={() => runtimeClient?.retryTerminalStorage(session.id)}
      onEnd={() => runtimeClient?.endTerminalAfterStorageFault(session.id)} />}
    {currentForkProgress && visible && <ForkProgressOverlay progress={currentForkProgress} />}
    {environmentUnavailable && !currentForkProgress && visible && <div className={`environment-card-overlay state-${environment!.state}`}
      role="status" aria-label={`运行环境${environmentOverlayTitle(environment!)}`}
      onPointerDown={(event) => event.stopPropagation()}>
      <div className="environment-card-overlay__content">
        <span className="environment-card-overlay__spinner" aria-hidden="true" />
        <strong>{environmentOverlayTitle(environment!)}</strong>
        <p>{environmentOverlayDescription(environment!)}</p>
        {(environment!.state === 'missing' || environment!.state === 'failed') &&
          <div className="environment-card-overlay__actions">
            {environment!.kind === 'worktree' && onRestoreEnvironment && <button type="button"
              disabled={Boolean(environmentAction)} onClick={() => void runEnvironmentAction(
                '正在恢复原 Worktree…', () => onRestoreEnvironment(session.id)
              )}>恢复 Worktree</button>}
            {environment!.kind === 'worktree' && onLocateEnvironment && <button type="button"
              disabled={Boolean(environmentAction)} onClick={() => void runEnvironmentAction(
                '正在定位 Worktree…', () => onLocateEnvironment(session.id)
              )}>定位目录</button>}
            {environment!.kind === 'worktree' && onHandoffEnvironment && <button type="button"
              disabled={Boolean(environmentAction)} onClick={() => void runEnvironmentAction(
                '正在交接到 Local…', () => onHandoffEnvironment(session.id, 'local')
              )}>交接到 Local</button>}
            {environment!.kind === 'local' && hasOwnedWorktree && onHandoffEnvironment && <button type="button"
              disabled={Boolean(environmentAction)} onClick={() => void runEnvironmentAction(
                '正在交接到 Worktree…', () => onHandoffEnvironment(session.id, 'worktree')
              )}>交接到 Worktree</button>}
          </div>}
        {(environmentAction || environmentActionError) && <small className={environmentActionError ? 'is-error' : ''}>
          {environmentActionError || environmentAction}
        </small>}
      </div>
    </div>}
    {confirmationOpen && !actionBlocked && flow.action === 'hide-window' && <ConfirmDialog title="提示"
      body={'当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。'}
      confirmLabel="我知道了" showCancel={false} onCancel={() => setConfirmationOpen(false)}
      onConfirm={() => setConfirmationOpen(false)} />}
    {confirmationOpen && !actionBlocked && flow.action !== 'hide-window' && <ConfirmationSequence steps={flow.steps}
      onCancel={() => setConfirmationOpen(false)} onComplete={() => remove(true)} />}
    {removalOpen && !actionBlocked && <ConfirmDialog title={removalTitle(session.title, descendantCount)}
      body={removalBody(session.title, childNodes.length, descendantCount, descendantImpact)}
      confirmLabel={removalConfirmLabel(descendantImpact, descendantCount)} confirmTone="danger"
      cancelLabel="取消" scope="session"
      onCancel={() => setRemovalOpen(false)} onConfirm={() => {
        setRemovalOpen(false)
        void Promise.resolve(onRemoveBranch?.(session.id, descendantCount > 0)).catch(NOOP)
      }} />}
    {forkReadinessHint && !actionBlocked && createPortal(<div className="fork-readiness-toast" role="status"
      aria-label="创建子分支条件说明">
      在当前会话输入一次，并等待 Claude Code 完成回复后，即可创建子分支
    </div>, document.body)}
    {contextMenu && !actionBlocked && createPortal(<>
      <div className="detach-context-overlay" onClick={() => setContextMenu(null)}
        onContextMenu={(event) => { event.preventDefault(); setContextMenu(null) }} />
      <div ref={contextMenuRef} className="detach-context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }}
        onClick={(event) => event.stopPropagation()}>
        {canFork && <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
          setContextMenu(null)
          void onFork?.(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>⑂ Fork 会话</button>}
        {canForkSibling && <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
          setContextMenu(null)
          void onForkSibling?.(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>⑂ Fork 兄弟分支</button>}
        {canDetach && <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
          setContextMenu(null)
          void onDetach(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>↗ 独立窗口</button>}
        {onRemoveBranch && <button className="detach-menu-item is-danger" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
          setContextMenu(null)
          setRemovalOpen(true)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>移除节点…</button>}
      </div>
    </>, document.body)}
  </section>
}

function LoadSessionIcon() {
  return <svg viewBox="0 0 20 20" aria-hidden="true" fill="none">
    <path d="M3.5 5.5h5l1.4 1.8h6.6v8.2h-13z" />
    <path d="M10 9v4m-2-2 2 2 2-2" />
  </svg>
}

function NOOP(): void {}

function environmentOverlayTitle(environment: SessionEnvironment): string {
  if (environment.state === 'recovering') return '正在恢复运行环境'
  if (environment.state === 'handoff') return '正在交接运行环境'
  if (environment.state === 'missing') return 'Worktree 需要恢复'
  return '运行环境需要处理'
}

function environmentOverlayDescription(environment: SessionEnvironment): string {
  if (environment.state === 'recovering') return '会话历史仍然保留，恢复完成后将自动重新进入终端。'
  if (environment.state === 'handoff') return '正在停止旧进程并进入目标目录，请稍候。'
  if (environment.kind === 'worktree') {
    return '会话和历史仍然保留。恢复、定位原 Worktree，或交接到 Local 后可继续输入。'
  }
  return '会话和历史仍然保留。请先交接到可用环境后继续输入。'
}

const READ_ONLY_REASON = '数据库处于只读恢复模式'

function gitLabel(git: SessionGitState): string {
  if (git.state === 'unavailable') return 'Git 不可用'
  const reference = git.branch ?? `HEAD ${git.detachedHead.slice(0, 7)}`
  return `${reference}${git.dirty ? '*' : ''}`
}

function gitTitle(git: SessionGitState): string {
  if (git.state === 'unavailable') return '当前目录不是可用的 Git 工作区'
  if (git.branch) return `Git 分支 ${git.branch}${git.dirty ? '，有未提交修改' : ''}`
  return `Git detached HEAD ${git.detachedHead}${git.dirty ? '，有未提交修改' : ''}`
}

export function removalBody(
  title: string,
  directChildCount: number,
  descendantCount: number,
  impact: { running: number; needsInput: number }
): string {
  const activity = [
    impact.running > 0 ? `${impact.running} 个运行中` : '',
    impact.needsInput > 0 ? `${impact.needsInput} 个待输入` : ''
  ].filter(Boolean).join('、')
  if (descendantCount === 0) {
    return `该节点没有子节点。移除后，“${title}”会从会话列表和 DAG 中消失。项目文件和工作树不会被删除。`
  }
  const scope = `该节点包含 ${directChildCount} 个直接子节点，共 ${descendantCount} 个后代节点。`
  const process = activity ? `其中 ${activity}的会话将先停止。` : ''
  return `${scope}${process}移除后，“${title}”及受影响节点会同时从会话列表和 DAG 中消失。项目文件和工作树不会被删除。`
}

export function removalTitle(title: string, descendantCount: number): string {
  return descendantCount > 0 ? `移除“${title}”及其整个分支？` : `移除“${title}”？`
}

export function removalConfirmLabel(
  impact: { running: number; needsInput: number },
  descendantCount: number
): string {
  const count = impact.running + impact.needsInput
  if (count > 0) return `停止 ${count} 个会话并移除`
  return descendantCount > 0 ? '移除整个分支' : '移除'
}

function BranchChildIcon() {
  return <svg aria-hidden="true" viewBox="0 0 24 24">
    <circle cx="6" cy="5" r="2" /><circle cx="18" cy="7" r="2" /><circle cx="6" cy="19" r="2" />
    <path d="M6 7v10M8 9c4 0 5-2 8-2" />
  </svg>
}

function BranchSiblingIcon() {
  return <svg aria-hidden="true" viewBox="0 0 24 24">
    <path d="M5 4v16M5 8h7M12 8v8M12 16h7" />
    <circle cx="5" cy="4" r="1.8" /><circle cx="19" cy="16" r="1.8" />
  </svg>
}

export function RemoveNodeIcon() {
  return <svg aria-hidden="true" viewBox="0 0 24 24">
    <circle cx="12" cy="12" r="8" /><path d="M8 12h8" />
  </svg>
}

function claudeWorkFailureReason(latestLines: string[]): string {
  const source = [...latestLines].reverse().find((line) =>
    /Connection refused|ConnectionRefused|ECONNREFUSED|API Error|authentication|invalid api key|OAuth|rate limit|overloaded|service unavailable/i.test(line)
  ) ?? ''
  if (/Connection refused|ConnectionRefused|ECONNREFUSED/i.test(source)) {
    return '连接被拒绝，请检查网络或代理后重试'
  }
  if (/authentication|invalid api key|OAuth/i.test(source)) {
    return 'Claude Code 账户认证失败，请重新登录后重试'
  }
  if (/rate limit/i.test(source)) return 'Claude 服务达到使用限额，请稍后重试'
  if (/overloaded|service unavailable/i.test(source)) return 'Claude 服务暂时不可用，请稍后重试'
  const concise = source.replace(/\s+/g, ' ').trim().slice(0, 160)
  return concise || '本轮 Claude Code 工作异常结束，请检查终端详情后重试'
}

function forkFailurePresentation(error: string | undefined): {
  title: string
  reason: string | undefined
} {
  if (error && /provider session not found|no conversation found|conversation.*not found/i.test(error)) {
    return {
      title: '父会话已失效',
      reason: '原 Claude Code 对话身份已失效，本次分支没有创建成功。请返回父会话继续，或移除此失败节点后新建空会话。'
    }
  }
  return { title: '分支创建失败', reason: error }
}

function providerRestoreIdentityExpired(error: string | undefined): boolean {
  return Boolean(error && /provider session not found|no conversation found|conversation.*not found/i.test(error))
}
