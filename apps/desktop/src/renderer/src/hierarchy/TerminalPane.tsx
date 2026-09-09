import { useCallback, useEffect, useLayoutEffect, useRef, useState, type MouseEvent } from 'react'
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
import { SessionRecoveryWater } from './SessionRecoveryWater'
import { terminalLoadingPresentation } from './terminal-loading-state'
import { foregroundTerminalModels } from '../terminal/terminal-model-cache'
import { useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'

type TerminalMessages = Messages['hierarchyTerminal']

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
  const messages = useMessages()
  const m = messages.hierarchyTerminal
  const shell = messages.hierarchyShell
  const {
    session, active, visible = true, foreground = true, viewportMoving = false,
    workspaceSessionCount = 0, taskName = shell.sceneTabBar.currentTask,
    pathValid = true, readOnly = false, workspaceId, sceneId, resumable = false, forkReady,
    providerRestoreState = 'none', restoreError, forkState, forkError, forkProgress, cwd, git,
    recoveryState: suppliedRecoveryState, recoveryError,
    sharedWorkingDirectory = false, environment, hasOwnedWorktree,
    spawnRevision = 0, onRetryRestore, onStartFreshProvider, onRetryRecovery, onRetryFork,
    childNodes = [], descendantNodes = [], parentSessionId, workStatus = 'idle', latestLines = [], onOpenChildren, onLoadSession,
    themeKey = 'light', fontSize = 11, onFontSizeChange, closeRequest = 0,
    searchRequest, onSearchResults, focusRequest = 0,
    onDiagnosticsStatusChange,
    onStorageFaultChange,
    onActivate, onDelete, onFork, onForkPeer, onForkSibling, onDetach,
    onRemoveBranch, onRestoreEnvironment, onLocateEnvironment, onHandoffEnvironment,
    onRename, onRestoreAutoTitle
  } = props
  const recoveryState = suppliedRecoveryState ?? 'ready'
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
  const [terminalVisualReady, setTerminalVisualReady] = useState(
    () => recoveryState === 'ready' && foregroundTerminalModels.has(session.id)
  )
  const [activationLoading, setActivationLoading] = useState(false)
  const previousActive = useRef(active)
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
  const forkReadinessReason = workStatus === 'running' || workStatus === 'starting' || workStatus === 'needs-input'
    ? m.pane.forkReadyAfterReply
    : m.pane.forkNeedsReply
  const environmentUnavailable = environment !== undefined && environment.state !== 'ready'
  const recoveryBlocking = recoveryState !== 'ready'
  const recoveryBusy = recoveryState === 'queued' || recoveryState === 'restoring'
  useLayoutEffect(() => {
    const becameActive = active && !previousActive.current
    previousActive.current = active
    if (!becameActive) return
    if (!recoveryBusy && foregroundTerminalModels.has(session.id)) {
      setTerminalVisualReady(true)
      setActivationLoading(false)
      return
    }
    setTerminalVisualReady(false)
    setActivationLoading(true)
  }, [active, recoveryBusy, session.id])
  useEffect(() => {
    if (recoveryBusy) setTerminalVisualReady(false)
  }, [recoveryBusy])
  const storageBlocked = storageFault !== null
  const actionBlocked = readOnly || storageBlocked || environmentUnavailable || recoveryBlocking
  const forkRepairBlocked = readOnly || storageBlocked || environmentUnavailable
  const freshStartBlocked = readOnly || storageBlocked || environmentUnavailable
  const environmentRepairBlocked = readOnly || storageBlocked
  const actionBlockedReason = readOnly ? shell.readOnlyRecoveryReason : storageBlocked
    ? shell.shell.storageFaultMutationReason : environmentUnavailable
    ? shell.shell.environmentMutationReason
    : recoveryBlocking ? m.pane.recoveryBlockedReason : undefined
  const freshStartBlockedReason = readOnly ? shell.readOnlyRecoveryReason : storageBlocked
    ? shell.shell.storageFaultMutationReason : environmentUnavailable
      ? shell.shell.environmentMutationReason : undefined
  const environmentRepairBlockedReason = readOnly ? shell.readOnlyRecoveryReason : storageBlocked
    ? shell.shell.storageFaultMutationReason : undefined
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
  const forkFailure = forkFailurePresentation(forkError, m)
  const currentForkProgress = activeForkProgress(forkProgress)
  const loadingPresentation = terminalLoadingPresentation({
    visible, foreground, isTeamMember, pathValid, terminalVisualReady, activationLoading,
    ...(suppliedRecoveryState === undefined ? {} : { recoveryState: suppliedRecoveryState }),
    providerRestoreState, ...(forkState === undefined ? {} : { forkState }),
    hasForkProgress: currentForkProgress !== undefined,
    runtimeStatus, hasStorageFault: storageFault !== null, environmentUnavailable
  })
  const showRecoveryWater = loadingPresentation !== null
  const restoreIdentityExpired = providerRestoreIdentityExpired(restoreError)
  const restoreNoticeKey = restoreIdentityExpired && providerRestoreState === 'failed'
    ? `${session.id}:${restoreError ?? ''}`
    : null
  const restoreNoticeVisible = restoreNoticeKey === null || dismissedRestoreNotice !== restoreNoticeKey
  const effectiveRestoreState = providerRestoreState
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
        throw new Error('reason' in result ? String(result.reason) : m.pane.environmentActionIncomplete)
      }
    } catch (error) {
      setEnvironmentActionError(error instanceof Error ? error.message : m.pane.environmentActionFailed)
    } finally {
      setEnvironmentAction('')
    }
  }
  return <section className={`terminal-pane split-leaf${active ? ' active-pane' : ''}${hasNotification ? ' has-notification' : ''}`} data-testid="terminal-pane"
    data-active={active} hidden={!visible} aria-busy={showRecoveryWater || undefined}
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
        {hasNotification && <span className="pane-notification-badge" role="status">{m.pane.newNotification}</span>}
        {loadingPresentation && <span className="pane-recovery-badge" aria-hidden="true">
          <i />{m.loading[loadingPresentation.labelKey]}
        </span>}
        {git && <span className="pane-environment-badge" title={gitTitle(git, m)}>
          {gitLabel(git, m)}
        </span>}
        {sharedWorkingDirectory && <span className="pane-environment-badge is-shared">
          {git ? m.pane.sharedWorktree : m.pane.sharedDirectory}
        </span>}
        {cwd && <span className="pane-cwd" title={cwd}>{cwd}</span>}
      </div>
      <div className="terminal-pane-actions">
        {onOpenChildren && <ChildSessionBadge children={childNodes}
          onOpen={() => void onOpenChildren(session.id)} />}
        {onLoadSession && <button className="pane-fork pane-load-session" type="button" draggable={false}
          aria-label={m.pane.loadClaudeSessionInto(session.title)} disabled={actionBlocked}
          title={actionBlockedReason ?? m.pane.loadClaudeSession}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            void onLoadSession(session.id)
          }}><LoadSessionIcon /></button>}
        {showFork && <button className="pane-fork" type="button" draggable={false}
          aria-label={m.pane.forkChildFrom(session.title)} aria-disabled={actionBlocked || !canFork}
          disabled={actionBlocked}
          title={actionBlockedReason ?? (canFork ? m.pane.forkChild : forkReadinessReason)}
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
          aria-label={m.pane.forkSiblingFrom(session.title)} disabled={actionBlocked}
          title={actionBlockedReason ?? m.pane.forkSibling}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            void onForkSibling?.(session.id)
          }}><BranchSiblingIcon /></button>}
        {onRemoveBranch && <button className="pane-fork pane-remove" type="button" draggable={false}
          aria-label={m.pane.removeNodeOf(session.title)} disabled={actionBlocked}
          title={actionBlockedReason ?? m.pane.removeNode}
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            setRemovalOpen(true)
          }}><RemoveNodeIcon /></button>}
      </div>
    </header>
    {!pathValid && visible && <div role="status">{shell.workspacePathUnavailable}</div>}
    {forkState === 'failed' && visible && <div className="fork-failure-card" role="status">
      <div><strong>{forkFailure.title}</strong>
        {forkFailure.reason && <span className="fork-failure-reason">{forkFailure.reason}</span>}
      </div>
      <div className="fork-failure-actions">
        {onRetryFork && <button type="button" aria-label={m.pane.retryFork} disabled={forkRepairBlocked}
          title={forkRepairBlocked ? actionBlockedReason : undefined} onClick={(event) => {
          event.stopPropagation()
          void onRetryFork(session.id)
        }}>{messages.common.retry}</button>}
        {onRemoveBranch && <button type="button" aria-label={m.pane.removeNodeAction} disabled={forkRepairBlocked}
          title={forkRepairBlocked ? actionBlockedReason : undefined} onClick={(event) => {
          event.stopPropagation()
          setRemovalOpen(true)
        }}>{m.pane.removeNodeAction}</button>}
      </div>
    </div>}
    {effectiveRestoreState === 'failed' && forkState !== 'failed' && visible && restoreNoticeVisible &&
      <div className="provider-restore-banner" role="status">
      <div><strong>{restoreIdentityExpired && session.kind === 'shell'
        ? m.pane.parentConversationExpired : m.pane.claudeRestoreFailed}</strong>
        <span className="provider-restore-reason">{restoreIdentityExpired && session.kind === 'shell'
          ? m.pane.switchedToShell
          : restoreError}</span>
      </div>
      {onRetryRestore && (!restoreIdentityExpired || session.kind === 'claude-code') && <button type="button"
        disabled={actionBlocked || restoreRetryPending} title={actionBlockedReason} onClick={(event) => {
        event.stopPropagation()
        if (restoreRetryPending) return
        setRestoreRetryPending(true)
        void Promise.resolve(onRetryRestore(session.id)).finally(() => setRestoreRetryPending(false))
      }}>{restoreRetryPending ? m.pane.restoring : m.pane.retryRestore}</button>}
      {session.kind === 'claude-code' && onStartFreshProvider && <button type="button"
        disabled={freshStartBlocked || restoreRetryPending} title={freshStartBlockedReason}
        onClick={(event) => {
          event.stopPropagation()
          setRestoreRetryPending(true)
          void Promise.resolve(onStartFreshProvider(session.id)).finally(() => setRestoreRetryPending(false))
        }}
      >{m.pane.startFreshClaude}</button>}
    </div>}
    {effectiveRestoreState === 'restoring' && forkState !== 'failed' && visible && <div className="provider-restore-banner restoring" role="status">
      <strong>{m.pane.restoringClaudeSession}</strong>
    </div>}
    {runtimeStatus === 'error' && forkState !== 'failed' && effectiveRestoreState !== 'failed' && visible &&
      <div className="session-start-failure-card" role="status">
        <div><strong>{m.pane.sessionStartFailed}</strong>
          <span className="session-start-failure-reason">{runtimeError || m.pane.terminalProcessFailed}</span>
        </div>
        <div className="session-start-failure-actions">
          <button type="button" disabled={actionBlocked} title={actionBlockedReason} onClick={(event) => {
            event.stopPropagation()
            setRuntimeError('')
            setRuntimeStatus('starting-session')
            setStartupRetry((value) => value + 1)
          }}>{m.pane.retryStart}</button>
          {onDelete ? <button type="button" disabled={actionBlocked} title={actionBlockedReason} onClick={(event) => {
            event.stopPropagation()
            void Promise.resolve(onDelete(session.id, true)).catch(NOOP)
          }}>{m.pane.removeFailedSession}</button> : onRemoveBranch && <button type="button" disabled={actionBlocked} title={actionBlockedReason} onClick={(event) => {
            event.stopPropagation()
            setRemovalOpen(true)
          }}>{m.pane.removeNodeAction}</button>}
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
      onVisualReady={() => {
        setTerminalVisualReady(true)
        setActivationLoading(false)
      }}
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
    {loadingPresentation && <SessionRecoveryWater sessionTitle={session.title}
      phase={loadingPresentation.phase} />}
    {recoveryState === 'failed' && forkState !== 'failed' && !storageFault &&
      !currentForkProgress && !environmentUnavailable &&
      <div className="session-recovery-overlay state-failed" data-testid="session-recovery-dialog"
      role="status" aria-label={m.pane.recoveryFailedFor(session.title)}
      onPointerDown={(event) => event.stopPropagation()}>
      <div className="session-recovery-overlay__content">
        <strong>{m.pane.recoveryFailed}</strong>
        <p>{recoveryError || m.pane.recoveryFailedBody}</p>
        {onRetryRecovery && <button type="button"
          aria-label={m.pane.retryRecoveryFor(session.title)}
          onClick={() => void onRetryRecovery(session.id)}>{messages.common.retry}</button>}
      </div>
    </div>}
    {environmentUnavailable && !currentForkProgress && visible && <div className={`environment-card-overlay state-${environment!.state}`}
      role="status" aria-label={m.environmentOverlay.label(environmentOverlayTitle(environment!, m))}
      onPointerDown={(event) => event.stopPropagation()}>
      <div className="environment-card-overlay__content">
        <span className="environment-card-overlay__spinner" aria-hidden="true" />
        <strong>{environmentOverlayTitle(environment!, m)}</strong>
        <p>{environmentOverlayDescription(environment!, m)}</p>
        {(environment!.state === 'missing' || environment!.state === 'failed') &&
          <div className="environment-card-overlay__actions">
            {environment!.kind === 'worktree' && onRestoreEnvironment && <button type="button"
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                m.environmentOverlay.restoringWorktree, () => onRestoreEnvironment(session.id)
              )}>{m.environmentOverlay.restoreWorktree}</button>}
            {environment!.kind === 'worktree' && onLocateEnvironment && <button type="button"
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                m.environmentOverlay.locatingWorktree, () => onLocateEnvironment(session.id)
              )}>{m.environmentOverlay.locateDirectory}</button>}
            {environment!.kind === 'worktree' && onHandoffEnvironment && <button type="button"
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                m.environmentOverlay.handingOffToLocal, () => onHandoffEnvironment(session.id, 'local')
              )}>{m.environmentOverlay.handoffToLocal}</button>}
            {environment!.kind === 'local' && hasOwnedWorktree && onHandoffEnvironment && <button type="button"
              disabled={Boolean(environmentAction) || environmentRepairBlocked}
              title={environmentRepairBlockedReason} onClick={() => void runEnvironmentAction(
                m.environmentOverlay.handingOffToWorktree, () => onHandoffEnvironment(session.id, 'worktree')
              )}>{m.environmentOverlay.handoffToWorktree}</button>}
          </div>}
        {(environmentAction || environmentActionError) && <small className={environmentActionError ? 'is-error' : ''}>
          {environmentActionError || environmentAction}
        </small>}
      </div>
    </div>}
    {confirmationOpen && closeFlow.action === 'hide-window' && <ConfirmDialog title={shell.notice}
      body={shell.sceneTabBar.lastTabBody}
      confirmLabel={shell.sceneTabBar.lastTabConfirm} showCancel={false} onCancel={() => setConfirmationOpen(false)}
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
      aria-label={m.pane.forkReadinessHint}>
      {forkReadinessReason}
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
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>{m.pane.rename}</button>}
        {session.kind === 'claude-code' && session.titleSource === 'manual' && onRestoreAutoTitle &&
          <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
            title={actionBlockedReason} onClick={() => {
            setContextMenu(null)
            void Promise.resolve(onRestoreAutoTitle(session.id)).catch(NOOP)
          }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>
            {m.pane.restoreAutoTitle}
          </button>}
        {showFork && onForkPeer && <button className="detach-menu-item" role="menuitem"
          aria-disabled={actionBlocked || !canFork} disabled={actionBlocked}
          title={actionBlockedReason ?? (canFork ? m.pane.forkPeer : forkReadinessReason)} onClick={() => {
            setContextMenu(null)
            if (!canFork) {
              setForkReadinessHint(true)
              return
            }
            void onForkPeer(session.id)
          }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>{m.pane.forkPeerAction}</button>}
        {canDetach && <button className="detach-menu-item" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
          setContextMenu(null)
          void onDetach(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>{m.pane.detach}</button>}
        {onRemoveBranch && <button className="detach-menu-item is-danger" role="menuitem" disabled={actionBlocked}
          title={actionBlockedReason} onClick={() => {
          setContextMenu(null)
          setRemovalOpen(true)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>{m.pane.removeNodeAction}</button>}
      </div>
    </>, document.body)}
    {renaming && <RenameDialog scope="session" title={m.pane.renameSessionTitle} label={m.pane.sessionName}
      emptyError={m.pane.sessionNameEmpty} initialValue={session.title}
      error={(value) => renameFailure?.title === value ? renameFailure.message : undefined}
      onCancel={() => setRenaming(false)} onConfirm={(title) => {
        void Promise.resolve(onRename?.(session.id, title)).then(() => setRenaming(false))
          .catch(() => setRenameFailure({ title, message: m.pane.renameFailed }))
      }} />}
  </section>
}

function LoadSessionIcon() {
  return <AppIcon name="folder-input" />
}

function NOOP(): void {}

function environmentOverlayTitle(environment: SessionEnvironment, m: TerminalMessages): string {
  if (environment.state === 'recovering') return m.environmentOverlay.title.recovering
  if (environment.state === 'handoff') return m.environmentOverlay.title.handoff
  if (environment.state === 'missing') return m.environmentOverlay.title.missing
  return m.environmentOverlay.title.fallback
}

function environmentOverlayDescription(environment: SessionEnvironment, m: TerminalMessages): string {
  if (environment.state === 'recovering') return m.environmentOverlay.description.recovering
  if (environment.state === 'handoff') return m.environmentOverlay.description.handoff
  if (environment.kind === 'worktree') return m.environmentOverlay.description.worktree
  return m.environmentOverlay.description.local
}

function gitLabel(git: SessionGitState, m: TerminalMessages): string {
  if (git.state === 'unavailable') return m.git.unavailable
  const reference = git.branch ?? `HEAD ${git.detachedHead.slice(0, 7)}`
  return `${reference}${git.dirty ? '*' : ''}`
}

function gitTitle(git: SessionGitState, m: TerminalMessages): string {
  if (git.state === 'unavailable') return m.git.notARepository
  if (git.branch) return m.git.branch(git.branch, git.dirty)
  return m.git.detachedHead(git.detachedHead ?? '', git.dirty)
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

function forkFailurePresentation(error: string | undefined, m: TerminalMessages): {
  title: string
  reason: string | undefined
} {
  if (error && /provider session not found|no conversation found|conversation.*not found/i.test(error)) {
    return {
      title: m.forkFailure.parentExpiredTitle,
      reason: m.forkFailure.parentExpiredReason
    }
  }
  return { title: m.forkFailure.title, reason: error }
}

function providerRestoreIdentityExpired(error: string | undefined): boolean {
  return Boolean(error && /provider session not found|no conversation found|conversation.*not found/i.test(error))
}
