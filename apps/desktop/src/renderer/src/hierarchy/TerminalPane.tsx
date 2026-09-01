import { useCallback, useEffect, useRef, useState, type MouseEvent } from 'react'
import { createPortal } from 'react-dom'

import { TerminalSurface, type RuntimeStatus, type TerminalSearchRequest } from '../terminal/TerminalSurface'
import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
import type { SessionView } from './hierarchy-types'
import { sessionDeleteFlow } from './terminal-close-flow'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import { toOscNotification } from '../notifications/osc-notification'
import type { TerminalThemeKey } from '../terminal/terminal-themes'
import { ChildSessionBadge } from '../session-canvas/ChildSessionBadge'
import type { SessionGraphNodeView } from './hierarchy-types'
import { AgentTeamMemberSummary } from './AgentTeamMemberSummary'

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
  cwd?: string
  git?: { branch: string; dirty: boolean }
  sharedWorkingDirectory?: boolean
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
  onRename?(sessionId: string, title: string): unknown
  onRestoreAutoTitle?(sessionId: string): unknown
}) {
  const {
    session, active, visible = true, workspaceSessionCount, taskName,
    pathValid = true, workspaceId, sceneId, resumable = false, forkReady,
    providerRestoreState = 'none', restoreError, forkState, forkError, cwd, git,
    sharedWorkingDirectory = false,
    spawnRevision = 0, onRetryRestore, onRetryWork, onRetryFork, onRemoveFailedFork,
    childNodes = [], workStatus = 'idle', latestLines = [], onOpenChildren, onLoadSession,
    themeKey = 'light', fontSize = 11, onFontSizeChange, closeRequest = 0,
    searchRequest, onSearchResults, focusRequest = 0,
    onActivate, onDelete, onFork, onForkSibling, onDetach,
    descendantCount = 0, descendantImpact = { running: 0, needsInput: 0 },
    onRemoveBranch, onRename, onRestoreAutoTitle
  } = props
  const [confirmationOpen, setConfirmationOpen] = useState(false)
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null)
  const contextMenuRef = useRef<HTMLDivElement>(null)
  const [removalOpen, setRemovalOpen] = useState(false)
  const [renaming, setRenaming] = useState(false)
  const [renameFailure, setRenameFailure] = useState<{ title: string; message: string } | null>(null)
  const [runtimeStatus, setRuntimeStatus] = useState<RuntimeStatus>('waiting-for-port')
  const [runtimeError, setRuntimeError] = useState('')
  const [startupRetry, setStartupRetry] = useState(0)
  const [restoreRetryPending, setRestoreRetryPending] = useState(false)
  const [dismissedRestoreNotice, setDismissedRestoreNotice] = useState<string | null>(null)
  const [forkReadinessHint, setForkReadinessHint] = useState(false)
  const previousPathValid = useRef(pathValid)
  const handleRuntimeStatus = useCallback((status: RuntimeStatus) => {
    setRuntimeStatus(status)
    if (status === 'streaming') setRuntimeError('')
  }, [])
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
  useEffect(() => {
    if (canFork) setForkReadinessHint(false)
  }, [canFork])
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
    event.preventDefault()
    event.stopPropagation()
    setContextMenu({ x: event.clientX, y: event.clientY })
  }
  return <section className={`terminal-pane split-leaf${active ? ' active-pane' : ''}${hasNotification ? ' has-notification' : ''}`} data-testid="terminal-pane"
    data-active={active} hidden={!visible}
    onPointerDown={(event) => {
      if ((event.target as HTMLElement).closest('button,[role="menuitem"]')) return
      notificationStore.dismissSessionIndicator(session.id)
      if (!active) onActivate(session.id)
    }}>
    <header className="terminal-pane-header split-pane-header" draggable={canDetach}
      onContextMenu={openPaneMenu}
      onDragEnd={(event) => {
        const outside = event.screenX <= window.screenX || event.screenY <= window.screenY ||
          event.screenX >= window.screenX + window.outerWidth ||
          event.screenY >= window.screenY + window.outerHeight
        if (outside && canDetach) void onDetach?.(session.id)
      }}>
      <div className="pane-header-content"><strong className="pane-title" title={session.title}>{session.title}</strong>
        {git && <span className="pane-environment-badge" title={`Git 分支 ${git.branch}${git.dirty ? '，有未提交修改' : ''}`}>
          {git.branch}{git.dirty ? '*' : ''}
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
          aria-label={`载入 Claude Code 会话到“${session.title}”`} title="载入 Claude Code 会话"
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            void onLoadSession(session.id)
          }}><LoadSessionIcon /></button>}
        {showFork && <button className="pane-fork" type="button" draggable={false}
          aria-label={`从“${session.title}”创建子分支`} aria-disabled={!canFork}
          title={canFork ? '创建子分支' : '完成首轮对话后可创建分支'}
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
          aria-label={`从共同父会话创建“${session.title}”的兄弟分支`} title="从共同父会话 Fork 兄弟分支"
          onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}
          onClick={(event) => {
            event.stopPropagation()
            void onForkSibling?.(session.id)
          }}><BranchSiblingIcon /></button>}
        {onRemoveBranch && <button className="pane-fork pane-remove" type="button" draggable={false}
          aria-label={`移出节点：${session.title}`} title="移出节点"
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
    {effectiveRestoreState === 'failed' && forkState !== 'failed' && visible && restoreNoticeVisible &&
      <div className="provider-restore-banner" role="status">
      <div><strong>{restoreIdentityExpired ? '原 Claude Code 对话已失效' : 'Claude Code 恢复失败'}</strong>
        <span className="provider-restore-reason">{restoreIdentityExpired
          ? '当前已切换到 Shell，可继续使用终端'
          : restoreError}</span>
      </div>
      {onRetryRestore && !restoreIdentityExpired && <button type="button" disabled={restoreRetryPending} onClick={(event) => {
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
        {onRetryWork && <button type="button" aria-label="重试本轮任务" onClick={(event) => {
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
          <button type="button" onClick={(event) => {
            event.stopPropagation()
            setRuntimeError('')
            setRuntimeStatus('starting-session')
            setStartupRetry((value) => value + 1)
          }}>重试启动</button>
          <button type="button" onClick={(event) => {
            event.stopPropagation()
            void Promise.resolve(onDelete(session.id, true)).catch(NOOP)
          }}>移除失败会话</button>
        </div>
      </div>}
    {isTeamMember && forkState !== 'failed' && <AgentTeamMemberSummary
      workStatus={workStatus} latestLines={latestLines} />}
    {!isTeamMember && forkState !== 'failed' && <TerminalSurface sessionId={session.id}
      executionContextId={session.executionContextId ?? 'local-default'}
      profile={profile} visible={visible} active={active} inputDisabled={!pathValid}
      themeKey={themeKey} fontSize={fontSize}
      {...(onFontSizeChange ? { onFontSizeChange } : {})}
      {...(searchRequest ? { searchRequest } : {})}
      {...(onSearchResults ? { onSearchResults } : {})}
      focusRequest={focusRequest}
      spawnRevision={spawnRevision + startupRetry}
      onStatusChange={handleRuntimeStatus}
      onRuntimeError={setRuntimeError}
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
    {confirmationOpen && flow.action === 'hide-window' && <ConfirmDialog title="提示"
      body={'当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。'}
      confirmLabel="我知道了" showCancel={false} onCancel={() => setConfirmationOpen(false)}
      onConfirm={() => setConfirmationOpen(false)} />}
    {confirmationOpen && flow.action !== 'hide-window' && <ConfirmationSequence steps={flow.steps}
      onCancel={() => setConfirmationOpen(false)} onComplete={() => remove(true)} />}
    {removalOpen && <ConfirmDialog title={`移除“${session.title}”及其整个分支？`}
      body={removalBody(session.title, childNodes.length, descendantCount, descendantImpact)}
      confirmLabel={removalConfirmLabel(descendantImpact)} cancelLabel="取消" scope="session"
      onCancel={() => setRemovalOpen(false)} onConfirm={() => {
        setRemovalOpen(false)
        void Promise.resolve(onRemoveBranch?.(session.id, descendantCount > 0)).catch(NOOP)
      }} />}
    {forkReadinessHint && createPortal(<div className="fork-readiness-toast" role="status"
      aria-label="创建子分支条件说明">
      在当前会话输入一次，并等待 Claude Code 完成回复后，即可创建子分支
    </div>, document.body)}
    {contextMenu && createPortal(<>
      <div className="detach-context-overlay" onClick={() => setContextMenu(null)}
        onContextMenu={(event) => { event.preventDefault(); setContextMenu(null) }} />
      <div ref={contextMenuRef} className="detach-context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }}
        onClick={(event) => event.stopPropagation()}>
        {onRename && <button className="detach-menu-item" role="menuitem" onClick={() => {
          setContextMenu(null)
          setRenameFailure(null)
          setRenaming(true)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>重命名…</button>}
        {session.kind === 'claude-code' && session.titleSource === 'manual' && onRestoreAutoTitle &&
          <button className="detach-menu-item" role="menuitem" onClick={() => {
            setContextMenu(null)
            void Promise.resolve(onRestoreAutoTitle(session.id)).catch(NOOP)
          }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>
            恢复 Claude 自动标题
          </button>}
        {canFork && <button className="detach-menu-item" role="menuitem" onClick={() => {
          setContextMenu(null)
          void onFork?.(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>⑂ Fork 会话</button>}
        {canForkSibling && <button className="detach-menu-item" role="menuitem" onClick={() => {
          setContextMenu(null)
          void onForkSibling?.(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>⑂ Fork 兄弟分支</button>}
        {canDetach && <button className="detach-menu-item" role="menuitem" onClick={() => {
          setContextMenu(null)
          void onDetach(session.id)
        }} onPointerDown={(event) => { event.preventDefault(); event.stopPropagation() }}>↗ 独立窗口</button>}
        {onRemoveBranch && <button className="detach-menu-item is-danger" role="menuitem" onClick={() => {
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
  return <svg viewBox="0 0 20 20" aria-hidden="true" fill="none">
    <path d="M3.5 5.5h5l1.4 1.8h6.6v8.2h-13z" />
    <path d="M10 9v4m-2-2 2 2 2-2" />
  </svg>
}

function NOOP(): void {}

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
  const scope = descendantCount > 0
    ? `该节点包含 ${directChildCount} 个直接子节点，共 ${descendantCount} 个后代节点。`
    : '该节点没有子节点。'
  const process = activity ? `其中 ${activity}的会话将先停止。` : ''
  return `${scope}${process}移除后，“${title}”及受影响节点会同时从会话列表和 DAG 中消失。项目文件和工作树不会被删除。`
}

export function removalConfirmLabel(impact: { running: number; needsInput: number }): string {
  const count = impact.running + impact.needsInput
  return count > 0 ? `停止 ${count} 个会话并移除` : '移除整个分支'
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
