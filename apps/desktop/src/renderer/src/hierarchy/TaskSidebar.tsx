import { useEffect, useMemo, useRef, useState, type MouseEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { Glass } from '@samasante/liquid-glass'

import { ConfirmDialog, ConfirmationSequence } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
import type { HierarchyCommands, HierarchyProjection, TaskView, WorkspaceView } from './hierarchy-types'
import { taskDeleteFlow } from './terminal-close-flow'
import { NotificationCenter } from '../notifications/NotificationCenter'
import type { AgentNotification } from '../notifications/AgentNotificationStore'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import workbenchIcon from '../assets/terminal-reference/terminal/dark_lujing.svg'
import { AppIcon } from '../ui/AppIcon'

const TASK_TRANSFER = 'application/x-matou-pinned-task'
const WORKSPACE_TRANSFER = 'application/x-matou-pinned-workspace'
const SIDEBAR_GLASS_OPTICS = {
  strength: 0.015,
  scaleX: 0.011,
  scaleY: 0.004,
  depth: 0.12,
  dispersion: 0.022,
  frost: 18,
  saturate: 0.92,
  brightness: 0.18,
  specular: 0.72,
  sheenAngle: 270,
  glow: 0.1,
  glowSpread: 0.16,
  glowFalloff: 3.2,
  sheen: 0.7,
  sheenWidth: 1.25,
  sheenFalloff: 3.6,
  curvature: 0.032,
  splay: 0.38,
  bend: 0.48,
  bendWidth: 0.045
} as const

function SidebarGlassMaterial() {
  const supported = typeof ResizeObserver !== 'undefined' && typeof CSS !== 'undefined' && typeof CSS.supports === 'function' &&
    (CSS.supports('backdrop-filter', 'blur(1px)') || CSS.supports('-webkit-backdrop-filter', 'blur(1px)'))
  if (!supported) return <div className="flat-sidebar__glass-material" aria-hidden="true" />
  return <Glass className="flat-sidebar__glass-material" aria-hidden="true"
    optics={SIDEBAR_GLASS_OPTICS} radius={0} />
}

export function TaskSidebar({ projection, commands, readOnly = false, onRevealSession, boardActive = false, onBoardActiveChange,
  settingsActive = false, onSettingsActiveChange }: {
  projection: HierarchyProjection
  commands: HierarchyCommands
  pathValid?: boolean
  readOnly?: boolean
  onRevealSession?(sceneId: string, sessionId: string): void
  boardActive?: boolean
  onBoardActiveChange?(active: boolean): void
  settingsActive?: boolean
  onSettingsActiveChange?(active: boolean): void
}) {
  const workspaces = useMemo(() => orderNavigation(projection.workspaces), [projection.workspaces])
  const placedIds = new Set(projection.taskPlacements
    .filter(({ windowId }) => windowId === projection.windowId).map(({ taskId }) => taskId))
  const visibleTasks = (workspaceId: string) => orderNavigation(projection.tasks.filter((task) =>
    task.workspaceId === workspaceId && (projection.taskPlacements.length === 0 || placedIds.has(task.id))))
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set())
  const [menuTask, setMenuTask] = useState<TaskView | null>(null)
  const [menuWorkspace, setMenuWorkspace] = useState<WorkspaceView | null>(null)
  const [renameTask, setRenameTask] = useState<TaskView | null>(null)
  const [renameFailure, setRenameFailure] = useState<{ title: string; message: string } | null>(null)
  const [deleteTask, setDeleteTask] = useState<TaskView | null>(null)
  const [removeWorkspace, setRemoveWorkspace] = useState<WorkspaceView | null>(null)
  const [dragTaskId, setDragTaskId] = useState<string | null>(null)
  const [dragWorkspaceId, setDragWorkspaceId] = useState<string | null>(null)
  const [dragOverId, setDragOverId] = useState<string | null>(null)
  const [menuPosition, setMenuPosition] = useState({ top: 0, left: 0 })
  const [toast, setToast] = useState('')
  const [creatingWorkspace, setCreatingWorkspace] = useState(false)
  const [notificationCenterOpen, setNotificationCenterOpen] = useState(false)
  const notificationStore = useNotificationStore()
  const notificationSnapshot = useNotificationSnapshot()
  const sidebarRef = useRef<HTMLElement>(null)
  const activeRef = useRef<HTMLSpanElement>(null)
  const activeWorkspaceId = projection.navigation.activeWorkspaceId
  const activeTaskId = activeWorkspaceId ? projection.navigation.taskByWorkspace[activeWorkspaceId] : undefined

  useEffect(() => { activeRef.current?.scrollIntoView?.({ block: 'nearest' }) }, [activeTaskId])
  useEffect(() => {
    const closeMenus = (event: PointerEvent) => {
      const target = event.target
      if (target instanceof Node && !document.querySelector('.workbench-action-popover')?.contains(target)) {
        setMenuTask(null); setMenuWorkspace(null)
      }
      if (target instanceof Node && notificationCenterOpen &&
        !document.querySelector('.notification-center')?.contains(target) &&
        !document.querySelector('.flat-sidebar__notify')?.contains(target)) setNotificationCenterOpen(false)
    }
    const onEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setMenuTask(null); setMenuWorkspace(null); setNotificationCenterOpen(false) }
    }
    window.addEventListener('pointerdown', closeMenus); window.addEventListener('keydown', onEscape, true)
    return () => { window.removeEventListener('pointerdown', closeMenus); window.removeEventListener('keydown', onEscape, true) }
  }, [notificationCenterOpen])
  useEffect(() => {
    if (!toast) return
    const timer = window.setTimeout(() => setToast(''), 2_000)
    return () => window.clearTimeout(timer)
  }, [toast])

  const chooseDirectory = async () => {
    if (readOnly) return
    const path = await window.matouDesktop?.selectWorkspaceDirectory()
    if (!path) return
    setCreatingWorkspace(true)
    try {
      await Promise.resolve(commands.createWorkspace(path))
    } catch {
      setToast('工作空间添加失败，请重试')
    } finally {
      setCreatingWorkspace(false)
    }
  }
  const relinkDirectory = async (workspace: WorkspaceView) => {
    if (readOnly) return
    const path = await window.matouDesktop?.selectWorkspaceDirectory()
    if (!path) return
    try {
      await Promise.resolve(commands.relinkWorkspace(workspace.id, path))
      setToast(`已恢复 ${workspace.name} 的工作目录`)
    } catch {
      setToast('工作目录恢复失败，请重新选择')
    }
  }
  const openTaskMenu = (task: TaskView, event: MouseEvent<HTMLElement>) => {
    event.stopPropagation()
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuPosition({ top: rect.top + rect.height / 2, left: rect.right + 6 })
    setMenuWorkspace(null); setMenuTask(menuTask?.id === task.id ? null : task)
  }
  const openWorkspaceMenu = (workspace: WorkspaceView, event: MouseEvent<HTMLElement>) => {
    event.stopPropagation()
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuPosition({ top: rect.top + rect.height / 2, left: rect.right + 6 })
    setMenuTask(null); setMenuWorkspace(menuWorkspace?.id === workspace.id ? null : workspace)
  }
  // The browser notification store is the single UI source for both the Task
  // badge and the Session pulse. Mixing in a second aggregate count leaves an
  // orphan red badge after the visible Session indicator has been dismissed.
  const unreadCount = (taskId: string) => notificationStore.unreadForTask(taskId)
  const moveGlassLight = (event: ReactPointerEvent<HTMLElement>) => {
    const rect = event.currentTarget.getBoundingClientRect()
    event.currentTarget.style.setProperty('--sidebar-glass-x', `${event.clientX - rect.left}px`)
    event.currentTarget.style.setProperty('--sidebar-glass-y', `${event.clientY - rect.top}px`)
  }
  const resetGlassLight = () => {
    sidebarRef.current?.style.setProperty('--sidebar-glass-x', '44%')
    sidebarRef.current?.style.setProperty('--sidebar-glass-y', '118px')
  }
  const resetDrag = () => { setDragTaskId(null); setDragWorkspaceId(null); setDragOverId(null) }
  const navigateNotification = async (notification: AgentNotification) => {
    const workspace = projection.workspaces.find(({ id }) => id === notification.workspaceId)
    if (!workspace) return
    await Promise.resolve(commands.activateWorkspace(workspace.id))
    const fallbackTaskId = projection.navigation.taskByWorkspace[workspace.id]
    const task = projection.tasks.find(({ id, workspaceId }) => id === notification.taskId && workspaceId === workspace.id)
      ?? projection.tasks.find(({ id }) => id === fallbackTaskId)
      ?? projection.tasks.find(({ workspaceId }) => workspaceId === workspace.id)
    if (task) await Promise.resolve(commands.activateTask(task.id))
    let success = Boolean(task)
    if (notification.sessionId) {
      const snapshot = projection.sceneSnapshots?.find(({ mounts }) => mounts.some(({ sessionId }) => sessionId === notification.sessionId))
      const mount = snapshot?.mounts.find(({ sessionId }) => sessionId === notification.sessionId)
      const detached = Boolean(mount?.sceneWindowId && snapshot?.windows.some(({ id, state }) => id === mount.sceneWindowId && state === 'detached'))
      const scene = snapshot && projection.scenes.find(({ id }) => id === snapshot.scene.id)
      if (task && scene?.taskId === task.id && projection.sessions.some(({ id }) => id === notification.sessionId) && !detached) {
        await Promise.resolve(commands.activateScene(scene.id))
        await Promise.resolve(commands.activateSession(notification.sessionId))
        onRevealSession?.(scene.id, notification.sessionId)
        success = true
      } else { success = false; setToast('原面板已不存在或不在当前窗口') }
    }
    if (success) notificationStore.remove(notification.id)
    setNotificationCenterOpen(false)
  }

  return <aside ref={sidebarRef} className="workbench-sidebar flat-sidebar" aria-label="事项列表"
    onPointerMove={moveGlassLight} onPointerLeave={resetGlassLight}>
    <SidebarGlassMaterial />
    <header className="flat-sidebar__topbar">
      <button className="flat-sidebar__new-workspace" aria-label="新增工作空间"
        disabled={readOnly || creatingWorkspace} title={readOnly ? READ_ONLY_REASON : undefined}
        onClick={() => void chooseDirectory()}>
        <ComposeIcon /><span>{creatingWorkspace ? '正在添加…' : '新增工作空间'}</span>
      </button>
      <button className="flat-sidebar__notify" aria-label="通知中心" aria-expanded={notificationCenterOpen}
        onClick={() => setNotificationCenterOpen((value) => !value)}>
        <AppIcon name="bell" />
        {notificationSnapshot.unreadCount > 0 && <span className="flat-sidebar__notify-dot" aria-hidden="true" />}
      </button>
    </header>
    {notificationCenterOpen && <NotificationCenter projection={projection}
      onClose={() => setNotificationCenterOpen(false)} onNavigate={(notification) => { void navigateNotification(notification) }} />}
    <nav className="flat-sidebar__groups custom-scrollbar" aria-label="工作空间与事项">
      {workspaces.map((workspace) => {
        const tasks = visibleTasks(workspace.id)
        const invalid = projection.pathStates.find(({ workspaceId }) => workspaceId === workspace.id)?.status === 'invalid'
        const isCollapsed = collapsed.has(workspace.id)
        return <section key={workspace.id} role="group" aria-label={`${workspace.name} 工作空间`}
          data-testid="workspace-group" data-workspace-id={workspace.id}
          className={`workspace-group${workspace.id === activeWorkspaceId ? ' is-active' : ''}${dragWorkspaceId === workspace.id ? ' is-dragging' : ''}${dragOverId === `workspace:${workspace.id}` ? ' drag-over' : ''}`}
          draggable={!readOnly && Boolean(workspace.isPinned)}
          onDragStart={(event) => {
            if (readOnly || !workspace.isPinned) return
            setDragWorkspaceId(workspace.id); event.dataTransfer.setData(WORKSPACE_TRANSFER, workspace.id); event.dataTransfer.effectAllowed = 'move'
          }}
          onDragOver={(event) => { if (!readOnly && dragWorkspaceId && workspace.isPinned && dragWorkspaceId !== workspace.id) { event.preventDefault(); setDragOverId(`workspace:${workspace.id}`) } }}
          onDrop={(event) => {
            const sourceId = event.dataTransfer.getData(WORKSPACE_TRANSFER)
            if (!readOnly && sourceId && workspace.isPinned) void commands.reorderPinnedWorkspace(sourceId, workspace.id)
            resetDrag()
          }} onDragEnd={resetDrag}>
          <div className="workspace-group__header" aria-current={workspace.id === activeWorkspaceId ? 'location' : undefined}>
            <button className="workspace-group__toggle" aria-expanded={!isCollapsed}
              onClick={() => {
                notificationStore.markWorkspaceRead(workspace.id)
                if (workspace.id === activeWorkspaceId) {
                  setCollapsed((current) => toggleSet(current, workspace.id))
                } else {
                  setCollapsed((current) => { const next = new Set(current); next.delete(workspace.id); return next })
                  void commands.activateWorkspace(workspace.id)
                }
              }}>
              <ChevronIcon collapsed={isCollapsed} /><FolderIcon home={Boolean(workspace.isDefault)} />
              <span className="workspace-group__name" title={workspace.rootDirectory}>{workspace.name}</span>
              <span className="workspace-group__status">
                {workspace.isDefault && <span className="workspace-group__badge">默认</span>}
                {workspace.isPinned && <PinIcon />}
                {invalid && <span className="workspace-invalid">路径失效</span>}
              </span>
            </button>
            <button className="workspace-group__add" aria-label={`在 ${workspace.name} 中新增事项`}
              title={readOnly ? READ_ONLY_REASON : invalid ? WORKSPACE_PATH_MESSAGE : '新增事项'}
              disabled={readOnly || invalid} onClick={() => void commands.createTask(workspace.id)}><PlusIcon /></button>
            {invalid && !workspace.isDefault && <button className="workspace-group__relink"
              aria-label={`重新关联工作空间目录：${workspace.name}`}
              disabled={readOnly} title={readOnly ? READ_ONLY_REASON : '选择工作空间的新位置'}
              onClick={(event) => { event.stopPropagation(); void relinkDirectory(workspace) }}>恢复目录</button>}
            <button className="workspace-group__more" data-icon="ellipsis"
              aria-label={`工作空间菜单：${workspace.name}`}
              onClick={(event) => openWorkspaceMenu(workspace, event)} />
          </div>
          {!isCollapsed && <div className="workspace-group__tasks" role="list">
            {tasks.map((task) => <div role="listitem" key={task.id} data-testid={`task-${task.id}`}
              className={`workbench-item${task.id === activeTaskId ? ' is-active' : ''}${task.id === dragTaskId ? ' is-dragging' : ''}${dragOverId === `task:${task.id}` ? ' drag-over' : ''}`}
              tabIndex={0} aria-current={task.id === activeTaskId ? 'true' : undefined}
              draggable={!readOnly && Boolean(task.isPinned)}
              onDragStart={(event) => {
                if (readOnly || !task.isPinned) return
                setDragTaskId(task.id); event.dataTransfer.setData(TASK_TRANSFER, JSON.stringify({ workspaceId: workspace.id, taskId: task.id })); event.dataTransfer.effectAllowed = 'move'
              }}
              onDragOver={(event) => { if (!readOnly && dragTaskId && task.isPinned && dragTaskId !== task.id) { event.preventDefault(); setDragOverId(`task:${task.id}`) } }}
              onDrop={(event) => {
                const source = parseTransfer(event.dataTransfer.getData(TASK_TRANSFER))
                if (!readOnly && source?.workspaceId === workspace.id && task.isPinned) void commands.reorderPinnedTask(workspace.id, source.taskId, task.id)
                resetDrag()
              }} onDragEnd={resetDrag}
              onClick={() => {
                notificationStore.markWorkspaceRead(workspace.id)
                onBoardActiveChange?.(false)
                onSettingsActiveChange?.(false)
                void commands.activateTask(task.id)
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault(); onBoardActiveChange?.(false); onSettingsActiveChange?.(false); void commands.activateTask(task.id)
                }
              }}
              onContextMenu={(event) => { event.preventDefault(); openTaskMenu(task, event) }}>
              <div className="workbench-item__left">
                <span className="workbench-item__icon" style={{ maskImage: `url(${workbenchIcon})`, WebkitMaskImage: `url(${workbenchIcon})` }} />
                <span className="workbench-item__name" data-testid={task.id === activeTaskId ? 'active-task' : undefined}
                  ref={task.id === activeTaskId ? activeRef : undefined}>{task.title}</span>
              </div>
              <div className="workbench-item__right" onClick={(event) => event.stopPropagation()}>
                <span className="workbench-item__status">
                  {task.isPinned && <PinIcon />}
                  {unreadCount(task.id) > 0 && <span className="workbench-item__badge">{unreadCount(task.id) > 99 ? '99+' : unreadCount(task.id)}</span>}
                </span>
                {unreadCount(task.id) === 0 && <span className="workbench-item__actions">
                  <button className={`workbench-item__more-btn${menuTask?.id === task.id ? ' is-open' : ''}`}
                    data-icon="ellipsis" aria-label={`事项菜单：${task.title}`} title="更多操作"
                    onClick={(event) => openTaskMenu(task, event)} />
                </span>}
              </div>
            </div>)}
          </div>}
        </section>
      })}
    </nav>
    <footer className="flat-sidebar__toolbar" aria-label="工作空间视图">
      <button type="button" className={`flat-sidebar__board-toggle${boardActive ? ' is-active' : ''}`}
        aria-label="看板" aria-pressed={boardActive}
        disabled={readOnly} title={readOnly ? READ_ONLY_REASON : undefined}
        onClick={() => { onSettingsActiveChange?.(false); onBoardActiveChange?.(!boardActive) }}>
        <KanbanIcon /><span>看板</span><i aria-hidden="true" />
      </button>
      <button type="button" className={`flat-sidebar__settings-toggle${settingsActive ? ' is-active' : ''}`}
        aria-label="设置" aria-pressed={settingsActive}
        onClick={() => { onBoardActiveChange?.(false); onSettingsActiveChange?.(!settingsActive) }}>
        <SettingsIcon /><span>设置</span>
      </button>
    </footer>
    {menuWorkspace && <div role="menu" className="workbench-action-popover" style={{ top: menuPosition.top, left: menuPosition.left }} onPointerDown={(event) => event.stopPropagation()}>
      {projection.pathStates.find(({ workspaceId }) => workspaceId === menuWorkspace.id)?.status === 'invalid' &&
        !menuWorkspace.isDefault && <button role="menuitem" disabled={readOnly}
          title={readOnly ? READ_ONLY_REASON : undefined} onClick={() => {
          const target = menuWorkspace
          setMenuWorkspace(null)
          void relinkDirectory(target)
        }}>重新关联工作空间目录</button>}
      <button role="menuitem" disabled={readOnly} title={readOnly ? READ_ONLY_REASON : undefined} onClick={() => {
        const pinned = !menuWorkspace.isPinned
        const workspaceId = menuWorkspace.id
        setMenuWorkspace(null)
        void Promise.resolve(commands.setWorkspacePinned(workspaceId, pinned))
          .then(() => setToast(pinned ? '工作空间已置顶' : '已取消工作空间置顶'))
          .catch(() => setToast('工作空间置顶状态更新失败'))
      }}>
        <PinIcon />{menuWorkspace.isPinned ? '取消置顶' : '置顶'}</button>
      <button role="menuitem" onClick={() => { void window.matouDesktop?.revealDirectory(menuWorkspace.rootDirectory); setMenuWorkspace(null) }}>在 Finder 中显示</button>
      <button role="menuitem" onClick={() => { void navigator.clipboard?.writeText(menuWorkspace.rootDirectory); setToast('路径已复制'); setMenuWorkspace(null) }}>复制路径</button>
      {!menuWorkspace.isDefault && <button role="menuitem" className="is-delete" disabled={readOnly}
        title={readOnly ? READ_ONLY_REASON : undefined}
        onClick={() => { setRemoveWorkspace(menuWorkspace); setMenuWorkspace(null) }}><TrashIcon />移出码头</button>}
    </div>}
    {menuTask && <div role="menu" className="workbench-action-popover" style={{ top: menuPosition.top, left: menuPosition.left }} onPointerDown={(event) => event.stopPropagation()}>
        <button role="menuitem" disabled={readOnly} title={readOnly ? READ_ONLY_REASON : undefined}
          onClick={() => { void commands.setTaskPinned(menuTask.id, !menuTask.isPinned); setMenuTask(null) }}><PinIcon />{menuTask.isPinned ? '取消置顶' : '置顶'}</button>
        <button role="menuitem" disabled={readOnly} title={readOnly ? READ_ONLY_REASON : undefined}
          onClick={() => { setRenameFailure(null); setRenameTask(menuTask); setMenuTask(null) }}><EditIcon />重命名</button>
        <button role="menuitem" className="is-delete" disabled={readOnly} title={readOnly ? READ_ONLY_REASON : undefined}
          onClick={() => { setDeleteTask(menuTask); setMenuTask(null) }}><TrashIcon />删除</button>
    </div>}
    {removeWorkspace && <ConfirmDialog title="移出工作空间"
      body={`移出 "${removeWorkspace.name}" 会关闭该空间下的事项和终端会话，本地文件保持原样。 是否继续？`}
      confirmLabel="移出" onCancel={() => setRemoveWorkspace(null)} onConfirm={() => {
        const id = removeWorkspace.id; setRemoveWorkspace(null); void Promise.resolve(commands.removeWorkspace(id)).catch(NOOP)
      }} />}
    {renameTask && <RenameDialog label="事项名称" placeholder="请输入事项名称" emptyError="工作台名称不能为空" initialValue={renameTask.title}
      error={(value) => visibleTasks(renameTask.workspaceId).some((task) => task.id !== renameTask.id && task.title === value)
        ? `当前工作区下已存在名为"${value}"的工作台`
        : renameFailure?.title === value ? renameFailure.message : undefined}
      onCancel={() => setRenameTask(null)} onConfirm={(title) => {
        void Promise.resolve(commands.renameTask(renameTask.id, title)).then(() => { setRenameTask(null); setToast('工作台已重命名') })
          .catch(() => setRenameFailure({ title, message: '重命名失败：名称为空或已存在' }))
      }} />}
    {deleteTask && <ConfirmationSequence steps={taskDeleteFlow({
      taskName: deleteTask.title, sessionCount: projection.sessions.filter(({ taskId }) => taskId === deleteTask.id).length
    }).steps} onCancel={() => setDeleteTask(null)} onComplete={() => {
      const taskId = deleteTask.id; setDeleteTask(null); void Promise.resolve(commands.deleteTask(taskId)).catch(NOOP)
    }} />}
    {toast && <div className="terminal-toast" role="status">{toast}</div>}
  </aside>
}

type NavigationItem = { id: string; isPinned?: boolean; pinSortKey?: string; lastOpenedAt?: number; createdAt?: number }
export function orderNavigation<T extends NavigationItem>(items: T[]): T[] {
  return [...items].sort((left, right) => {
    if (Boolean(left.isPinned) !== Boolean(right.isPinned)) return left.isPinned ? -1 : 1
    if (left.isPinned) return (left.pinSortKey ?? '').localeCompare(right.pinSortKey ?? '') || left.id.localeCompare(right.id)
    return (right.lastOpenedAt ?? 0) - (left.lastOpenedAt ?? 0) ||
      (left.createdAt ?? 0) - (right.createdAt ?? 0) || left.id.localeCompare(right.id)
  })
}
function toggleSet(current: Set<string>, id: string): Set<string> {
  const next = new Set(current); next.has(id) ? next.delete(id) : next.add(id); return next
}
function PlusIcon() { return <AppIcon name="plus" size={14} /> }
function ComposeIcon() { return <AppIcon name="square-pen" size={17} /> }
function ChevronIcon({ collapsed }: { collapsed: boolean }) { return <AppIcon className="workspace-group__chevron" name={collapsed ? 'chevron-right' : 'chevron-down'} size={14} /> }
function FolderIcon(_props: { home?: boolean }) { return <AppIcon name="folder" /> }
function PinIcon() { return <AppIcon className="pin-icon" name="pin" size={13} /> }
function EditIcon() { return <AppIcon name="pencil" size={14} /> }
function TrashIcon() { return <AppIcon name="trash-2" size={14} /> }
function KanbanIcon() { return <AppIcon name="columns-3" /> }
function SettingsIcon() { return <AppIcon name="settings-2" /> }
const WORKSPACE_PATH_MESSAGE = '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
const READ_ONLY_REASON = '数据库处于只读恢复模式'
function NOOP(): void {}
function parseTransfer(value: string): { workspaceId: string; taskId: string } | undefined {
  try {
    const parsed = JSON.parse(value) as Record<string, unknown>
    return typeof parsed.workspaceId === 'string' && typeof parsed.taskId === 'string'
      ? { workspaceId: parsed.workspaceId, taskId: parsed.taskId } : undefined
  } catch { return undefined }
}
