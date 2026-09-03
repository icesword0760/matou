import { useCallback, useEffect, useRef, useState, type MouseEvent } from 'react'
import { createPortal } from 'react-dom'

import {
  TerminalSurface,
  type RuntimeStatus,
  type TerminalSearchRequest,
  type TerminalStorageFaultMessage
} from '../terminal/TerminalSurface'
import type { RemoveNodeScope, SessionView } from './hierarchy-types'
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
import { RemoveNodeDialog } from '../session-canvas/RemoveNodeDialog'
import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
import { sessionDeleteFlow } from './terminal-close-flow'
import { AppIcon } from '../ui/AppIcon'

export function TerminalPane(props: {
  session: SessionView
  active: boolean
  visible?: boolean
  foreground?: boolean
  viewportMoving?: boolean
  workspaceSessionCount?: number
  taskName?: string
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
  diagnosticsProbe?: boolean
  onDiagnosticsStatusChange?(status: RuntimeStatus): void
  onDiagnosticsSmokeMarker?(marker: string): void
  onDiagnosticsReplayComplete?(marker: string): void
  onStorageFaultChange?(sessionId: string, faulted: boolean): void
  onActivate(sessionId: string): unknown
  onDelete?(sessionId: string, confirmed?: boolean): unknown
  resumable?: boolean
  forkReady?: boolean
  providerRestoreState?: 'none' | 'restoring' | 'failed'
  restoreError?: string
  recoveryState?: 'queued' | 'restoring' | 'ready' | 'failed'
  recoveryError?: string
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
  onStartFreshProvider?(sessionId: string): unknown
  onRetryRecovery?(sessionId: string): unknown
  onRetryWork?(sessionId: string): unknown
  onRetryFork?(sessionId: string): unknown
  childNodes?: SessionGraphNodeView[]
  descendantNodes?: SessionGraphNodeView[]
  parentSessionId?: string
  workStatus?: SessionGraphNodeView['workStatus']
  latestLines?: string[]
  onOpenChildren?(sessionId: string): unknown
  onLoadSession?(sessionId: string): unknown
  onFork?(sessionId: string): unknown
  onForkPeer?(sessionId: string): unknown
  onForkSibling?(sessionId: string): unknown
  onDetach?(sessionId: string): unknown
  onRemoveBranch?(sessionId: string, scope: RemoveNodeScope): unknown
  onRestoreEnvironment?(sessionId: string): unknown
  onLocateEnvironment?(sessionId: string): unknown
  onHandoffEnvironment?(sessionId: string, target: SessionEnvironmentTarget): unknown
  onRename?(sessionId: string, title: string): unknown
  onRestoreAutoTitle?(sessionId: string): unknown
}) {
  const {
    session, active, visible = true, foreground = true, viewportMoving = false,
    workspaceSessionCount = 0, taskName = '当前事项',
    pathValid = true, readOnly = false, workspaceId, sceneId, resumable = false, forkReady,
    providerRestoreState = 'none', restoreError, forkState, forkError, forkProgress, cwd, git,
    recoveryState = 'ready', recoveryError,
    sharedWorkingDirectory = false, environment, hasOwnedWorktree,
    spawnRevision = 0, onRetryRestore, onStartFreshProvider, onRetryRecovery, onRetryWork, onRetryFork,
    childNodes = [], descendantNodes = [], parentSessionId, workStatus = 'idle', latestLines = [], onOpenChildren, onLoadSession,
    themeKey = 'light', fontSize = 11, onFontSizeChange, closeRequest = 0,
    searchRequest, onSearchResults, focusRequest = 0,
    onDiagnosticsStatusChange,
    onStorageFaultChange,
    onActivate, onDelete, onFork, onForkPeer, onForkSibling, onDetach,
    onRemoveBranch, onRestoreEnvironment, onLocateEnvironment, onHandoffEnvironment,
    onRename, onRestoreAutoTitle
  } = props
  const [confirmationOpen, setConfirmationOpen] = useState(false)
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null)
  const contextMenuRef = useRef<HTMLDivElement>(null)
  const [removalOpen, setRemovalOpen] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [renameFailure, setRenameFailure] = useState<{ title: string; message: string } | null>(null)
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
    if (status === 'exited') {
      setStorageFault(null)
      onStorageFaultChange?.(session.id, false)
    }
  }, [onStorageFaultChange, session.id])
  const handleStorageFault = useCallback((fault: TerminalStorageFaultMessage) => {
    setStorageFault(fault)
    onStorageFaultChange?.(session.id, true)
  }, [onStorageFaultChange, session.id])
  const handleStorageRecovered = useCallback(() => {
    setStorageFault(null)
    onStorageFaultChange?.(session.id, false)
  }, [onStorageFaultChange, session.id])
  useEffect(() => () => onStorageFaultChange?.(session.id, false), [onStorageFaultChange, session.id])
  const handleRuntimeAndDiagnosticsStatus = useCallback((status: RuntimeStatus) => {
    handleRuntimeStatus(status)
    onDiagnosticsStatusChange?.(status)
  }, [handleRuntimeStatus, onDiagnosticsStatusChange])
  const runtimeClient = useRuntimeClient()
  const notificationStore = useNotificationStore()
  useNotificationSnapshot()
  const closeFlow = sessionDeleteFlow({
    isWorkspaceFinal: workspaceSessionCount === 1,
    taskName, sessionTitle: session.title, workStatus,
    childCount: childNodes.length
  })
  const profile = session.kind === 'claude-code' || session.kind === 'codex'
    ? session.kind : 'shell'
  const showFork = session.kind === 'claude-code' && onFork !== undefined
  const canFork = showFork && (forkReady ?? resumable)
  const environmentUnavailable = environment !== undefined && environment.state !== 'ready'
  const recoveryBlocking = recoveryState !== 'ready'
  const recoveryBusy = recoveryState === 'queued' || recoveryState === 'restoring'
  const storageBlocked = storageFault !== null
  const actionBlocked = readOnly || storageBlocked || environmentUnavailable || recoveryBlocking
  const forkRepairBlocked = readOnly || storageBlocked || environmentUnavailable
  const freshStartBlocked = readOnly || storageBlocked || environmentUnavailable
  const environmentRepairBlocked = readOnly || storageBlocked
  const actionBlockedReason = readOnly ? READ_ONLY_REASON : storageBlocked
    ? STORAGE_FAULT_REASON : environmentUnavailable
    ? '当前运行环境需要先恢复或交接'
    : recoveryBlocking ? '当前终端仍在恢复' : undefined
  const freshStartBlockedReason = readOnly ? READ_ONLY_REASON : storageBlocked
    ? STORAGE_FAULT_REASON : environmentUnavailable
      ? '当前运行环境需要先恢复或交接' : undefined
  const environmentRepairBlockedReason = readOnly ? READ_ONLY_REASON : storageBlocked
    ? STORAGE_FAULT_REASON : undefined
  const deleteSession = useCallback((confirmed: boolean) => {
    setConfirmationOpen(false)
    if (onDelete) void Promise.resolve(onDelete(session.id, confirmed)).catch(NOOP)
  }, [onDelete, session.id])
  const requestRemove = useCallback(() => {
    if (actionBlocked) return
    if (onDelete) {
      if (closeFlow.action === 'silent') deleteSession(false)
      else setConfirmationOpen(true)
      return
    }
    if (onRemoveBranch) setRemovalOpen(true)
  }, [actionBlocked, closeFlow.action, deleteSession, onDelete, onRemoveBranch])
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
  const effectiveRestoreState = providerRestoreState
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
    data-active={active} hidden={!visible} aria-busy={recoveryBusy || undefined}
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
        {hasNotification && <span className="pane-notification-badge" role="status">新通知</span>}
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
        {onRetryFork && <button type="button" aria-label="重试创建分支" disabled={forkRepairBlocked}
          title={forkRepairBlocked ? actionBlockedReason : undefined} onClick={(event) => {
          event.stopPropagation()
          void onRetryFork(session.id)
        }}>重试</button>}
        {onRemoveBranch && <button type="button" aria-label="移除节点…" disabled={forkRepairBlocked}
          title={forkRepairBlocked ? actionBlockedReason : undefined} onClick={(event) => {
          event.stopPropagation()
          setRemovalOpen(true)
        }}>移除节点…</button>}
      </div>
    </div>}
    {effectiveRestoreState === 'failed' && forkState !== 'failed' && visible && restoreNoticeVisible &&
      <div className="provider-restore-banner" role="status">
      <div><strong>{restoreIdentityExpired && session.kind === 'shell'
        ? '原 Claude Code 对话已失效' : 'Claude Code 恢复失败'}</strong>
        <span className="provider-restore-reason">{restoreIdentityExpired && session.kind === 'shell'
          ? '当前已切换到 Shell，可继续使用终端'
          : restoreError}</span>
      </div>
      {onRetryRestore && (!restoreIdentityExpired || session.kind === 'claude-code') && <button type="button"
        disabled={actionBlocked || restoreRetryPending} title={actionBlockedReason} onClick={(event) => {
        event.stopPropagation()
        if (restoreRetryPending) return
        setRestoreRetryPending(true)
        void Promise.resolve(onRetryRestore(session.id)).finally(() => setRestoreRetryPending(false))
      }}>{restoreRetryPending ? '正在恢复…' : '重试恢复'}</button>}
      {session.kind === 'claude-code' && onStartFreshProvider && <button type="button"
        disabled={freshStartBlocked || restoreRetryPending} title={freshStartBlockedReason}
        onClick={(event) => {
          event.stopPropagation()
          setRestoreRetryPending(true)
          void Promise.resolve(onStartFreshProvider(session.id)).finally(() => setRestoreRetryPending(false))
        }}
      >新开 Claude Code</button>}
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
          {onDelete ? <button type="button" disabled={actionBlocked} title={actionBlockedReason} onClick={(event) => {
            event.stopPropagation()
            void Promise.resolve(onDelete(session.id, true)).catch(NOOP)
          }}>移除失败会话</button> : onRemoveBranch && <button type="button" disabled={actionBlocked} title={actionBlockedReason} onClick={(event) => {
            event.stopPropagation()
            setRemovalOpen(true)
          }}>移除节点…</button>}
        </div>
      </div>}
    {isTeamMember && forkState !== 'failed' && <AgentTeamMemberSummary
      workStatus={workStatus} latestLines={latestLines} />}
    {!isTeamMember && forkState !== 'failed' &&
      !(effectiveRestoreState === 'failed' && session.kind === 'claude-code') && foreground &&
      (recoveryState === 'ready' || storageFault !== null) && <TerminalSurface sessionId={session.id}
      executionContextId={session.executionContextId ?? 'local-default'}
      profile={profile} visible={visible} active={active} foreground={foreground}
      viewportMoving={viewportMoving}
      inputDisabled={actionBlocked || !pathValid || storageFault !== null}
      readOnly={actionBlocked}
      themeKey={themeKey} fontSize={fontSize}
      {...(onFontSizeChange ? { onFontSizeChange } : {})}
      {...(searchRequest ? { searchRequest } : {})}
      {...(onSearchResults ? { onSearchResults } : {})}
      focusRequest={focusRequest}
      {...(props.diagnosticsProbe ? { diagnosticsProbe: true } : {})}
      spawnRevision={spawnRevision + startupRetry}
      onStatusChange={handleRuntimeAndDiagnosticsStatus}
      {...(props.onDiagnosticsSmokeMarker
        ? { onSmokeMarker: props.onDiagnosticsSmokeMarker }
        : {})}
      {...(props.onDiagnosticsReplayComplete
        ? { onReplayComplete: props.onDiagnosticsReplayComplete }
        : {})}
      onRuntimeError={setRuntimeError}
      onStorageFault={handleStorageFault}
      onStorageRecovered={handleStorageRecovered}
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
    {!isTeamMember && forkState !== 'failed' && !foreground && recoveryState === 'ready' &&
      <div className="background-session-placeholder" data-testid={`background-session-${session.id}`}
        aria-hidden="true" />}
    {storageFault && !environmentUnavailable && !currentForkProgress && visible && <StorageFaultOverlay
      sessionTitle={session.title}
      fault={{
        code: storageFault.code,
        retainedBytes: storageFault.retainedBytes,
        message: storageFault.message
      }}
      onRetry={() => runtimeClient?.retryTerminalStorage(session.id)}
      onEnd={() => runtimeClient?.endTerminalAfterStorageFault(session.id)} />}
    {currentForkProgress && visible && <ForkProgressOverlay progress={currentForkProgress} />}
    {recoveryState !== 'ready' && forkState !== 'failed' && !storageFault &&
      !currentForkProgress && !environmentUnavailable &&
      <div className={`session-recovery-overlay state-${recoveryState}`}
      role="status" aria-label={recoveryState === 'failed'
        ? `终端恢复失败：${session.title}` : `正在恢复终端：${session.title}`}
      onPointerDown={(event) => event.stopPropagation()}>
      <div className="session-recovery-overlay__content">
        {recoveryState !== 'failed' && <span className="session-recovery-overlay__spinner" aria-hidden="true" />}
        <strong>{recoveryState === 'failed'
          ? '终端恢复失败'
          : recoveryState === 'queued' ? '等待恢复终端' : '正在恢复终端'}</strong>
        <p>{recoveryState === 'failed'
          ? (recoveryError || '本会话恢复未完成，其他会话仍可继续使用。')
          : recoveryState === 'queued'
            ? '已进入恢复队列，将按当前使用位置优先恢复。'
            : '正在恢复最近的终端内容与运行状态…'}</p>
        {recoveryState === 'failed' && onRetryRecovery && <button type="button"
          aria-label={`重试恢复终端：${session.title}`}
          onClick={() => void onRetryRecovery(session.id)}>重试</button>}
      </div>
    </div>}
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
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                '正在恢复原 Worktree…', () => onRestoreEnvironment(session.id)
              )}>恢复 Worktree</button>}
            {environment!.kind === 'worktree' && onLocateEnvironment && <button type="button"
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                '正在定位 Worktree…', () => onLocateEnvironment(session.id)
              )}>定位目录</button>}
            {environment!.kind === 'worktree' && onHandoffEnvironment && <button type="button"
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                '正在交接到 Local…', () => onHandoffEnvironment(session.id, 'local')
              )}>交接到 Local</button>}
            {environment!.kind === 'local' && hasOwnedWorktree && onHandoffEnvironment && <button type="button"
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                '正在交接到 Worktree…', () => onHandoffEnvironment(session.id, 'worktree')
              )}>交接到 Worktree</button>}
          </div>}
        {(environmentAction || environmentActionError) && <small className={environmentActionError ? 'is-error' : ''}>
          {environmentActionError || environmentAction}
        </small>}
      </div>
    </div>}
    {confirmationOpen && closeFlow.action === 'hide-window' && <ConfirmDialog title="提示"
      body={'当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。'}
      confirmLabel="我知道了" showCancel={false} onCancel={() => setConfirmationOpen(false)}
      onConfirm={() => setConfirmationOpen(false)} />}
    {confirmationOpen && closeFlow.action !== 'hide-window' && <ConfirmationSequence steps={closeFlow.steps}
      onCancel={() => setConfirmationOpen(false)} onComplete={() => deleteSession(true)} />}
    {removalOpen && !actionBlocked && <RemoveNodeDialog title={session.title}
      current={{ workStatus, ...(hasOwnedWorktree === undefined ? {} : { hasOwnedWorktree }),
        ...(parentSessionId ? { parentSessionId } : {}) }}
      descendants={descendantNodes}
      onCancel={() => setRemovalOpen(false)} onConfirm={(scope) => {
        setRemovalOpen(false)
        void Promise.resolve(onRemoveBranch?.(session.id, scope)).catch(NOOP)
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
        {onRename && <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
          setContextMenu(null)
          setRenameFailure(null)
          setRenaming(true)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>重命名…</button>}
        {session.kind === 'claude-code' && session.titleSource === 'manual' && onRestoreAutoTitle &&
          <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
            title={actionBlockedReason} onClick={() => {
            setContextMenu(null)
            void Promise.resolve(onRestoreAutoTitle(session.id)).catch(NOOP)
          }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>
            恢复 Claude 自动标题
          </button>}
        {canFork && onForkPeer && <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
            setContextMenu(null)
            void onForkPeer(session.id)
          }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>⑂ Fork 会话</button>}
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
    {renaming && <RenameDialog scope="session" title="重命名会话" label="会话名称"
      placeholder="请输入会话名称" emptyError="会话名称不能为空" initialValue={session.title}
      error={(value) => renameFailure?.title === value ? renameFailure.message : undefined}
      onCancel={() => setRenaming(false)} onConfirm={(title) => {
        void Promise.resolve(onRename?.(session.id, title)).then(() => setRenaming(false))
          .catch(() => setRenameFailure({ title, message: '重命名失败，请稍后重试' }))
      }} />}
  </section>
}

function LoadSessionIcon() {
  return <AppIcon name="folder-input" />
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
const STORAGE_FAULT_REASON = '终端存储异常，请先恢复或结束当前会话'

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

function BranchChildIcon() {
  return <AppIcon name="layers-plus" />
}

function BranchSiblingIcon() {
  return <AppIcon name="copy-plus" />
}

export function RemoveNodeIcon() {
  return <AppIcon name="circle-minus" />
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
