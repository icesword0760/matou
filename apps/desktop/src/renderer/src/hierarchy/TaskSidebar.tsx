import { useEffect, useMemo, useRef, useState, type MouseEvent } from 'react'

import { ConfirmDialog, ConfirmationSequence } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
import type { HierarchyCommands, HierarchyProjection, TaskView, WorkspaceView } from './hierarchy-types'
import { taskDeleteFlow } from './terminal-close-flow'
import { NotificationCenter } from '../notifications/NotificationCenter'
import type { AgentNotification } from '../notifications/AgentNotificationStore'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import notificationIcon from '../assets/kooky/terminal/dark_toongzhi.svg'
import notificationAnimatedIcon from '../assets/kooky/terminal/rongzhi_ani.gif'
import workbenchIcon from '../assets/kooky/terminal/dark_lujing.svg'

const TASK_TRANSFER = 'application/x-matou-pinned-task'
const WORKSPACE_TRANSFER = 'application/x-matou-pinned-workspace'

export function TaskSidebar({ projection, commands, onRevealSession }: {
  projection: HierarchyProjection
  commands: HierarchyCommands
  pathValid?: boolean
  onRevealSession?(sceneId: string, sessionId: string): void
}) {
  const workspaces = useMemo(() => orderNavigation(projection.workspaces), [projection.workspaces])
  const placedIds = new Set(projection.taskPlacements
    .filter(({ windowId }) => windowId === projection.windowId).map(({ taskId }) => taskId))
  const visibleTasks = (workspaceId: string) => orderNavigation(projection.tasks.filter((task) =>
    task.workspaceId === workspaceId && (projection.taskPlacements.length === 0 || placedIds.has(task.id))))
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set())
  const [menuTask, setMenuTask] = useState<TaskView | null>(null)
  const [closedTaskId, setClosedTaskId] = useState<string | null>(null)
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
  const [notificationCenterOpen, setNotificationCenterOpen] = useState(false)
  const notificationStore = useNotificationStore()
  const notificationSnapshot = useNotificationSnapshot()
  const activeRef = useRef<HTMLSpanElement>(null)
  const activeWorkspaceId = projection.navigation.activeWorkspaceId
  const activeTaskId = activeWorkspaceId ? projection.navigation.taskByWorkspace[activeWorkspaceId] : undefined

  useEffect(() => { activeRef.current?.scrollIntoView?.({ block: 'nearest' }) }, [activeTaskId])
  useEffect(() => {
    const closeMenus = (event: PointerEvent) => {
      const target = event.target
      if (target instanceof Node && !document.querySelector('.workbench-action-popover')?.contains(target)) {
        setMenuTask(null); setMenuWorkspace(null); setClosedTaskId(null)
      }
      if (target instanceof Node && notificationCenterOpen &&
        !document.querySelector('.notification-center')?.contains(target) &&
        !document.querySelector('.flat-sidebar__notify')?.contains(target)) setNotificationCenterOpen(false)
    }
    const onEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { setMenuTask(null); setMenuWorkspace(null); setClosedTaskId(null); setNotificationCenterOpen(false) }
    }
    window.addEventListener('pointerdown', closeMenus); window.addEventListener('keydown', onEscape)
    return () => { window.removeEventListener('pointerdown', closeMenus); window.removeEventListener('keydown', onEscape) }
  }, [notificationCenterOpen])
  useEffect(() => {
    if (!toast) return
    const timer = window.setTimeout(() => setToast(''), 2_000)
    return () => window.clearTimeout(timer)
  }, [toast])

  const chooseDirectory = async () => {
    const path = await window.matouDesktop?.selectWorkspaceDirectory()
    if (path) await commands.createWorkspace(path)
  }
  const openTaskMenu = (task: TaskView, event: MouseEvent<HTMLElement>) => {
    event.stopPropagation()
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuPosition({ top: rect.top + rect.height / 2, left: rect.right + 6 })
    setMenuWorkspace(null); setClosedTaskId(null); setMenuTask(menuTask?.id === task.id ? null : task)
  }
  const openWorkspaceMenu = (workspace: WorkspaceView, event: MouseEvent<HTMLElement>) => {
    event.stopPropagation()
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuPosition({ top: rect.top + rect.height / 2, left: rect.right + 6 })
    setMenuTask(null); setMenuWorkspace(menuWorkspace?.id === workspace.id ? null : workspace)
  }
  const unreadCount = (taskId: string) => notificationStore.unreadForTask(taskId) || projection.unreadByTask?.[taskId] || 0
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

  return <aside className="workbench-sidebar flat-sidebar" aria-label="事项列表">
    <header className="flat-sidebar__topbar">
      <button className="flat-sidebar__new-workspace" aria-label="新增工作空间" onClick={() => void chooseDirectory()}>
        <ComposeIcon /><span>新增工作空间</span>
      </button>
      <button className="flat-sidebar__notify" aria-label="通知中心" aria-expanded={notificationCenterOpen}
        onClick={() => setNotificationCenterOpen((value) => !value)}>
        <img src={notificationSnapshot.unreadCount > 0 ? notificationAnimatedIcon : notificationIcon} alt="" />
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
          draggable={Boolean(workspace.isPinned)}
          onDragStart={(event) => {
            if (!workspace.isPinned) return
            setDragWorkspaceId(workspace.id); event.dataTransfer.setData(WORKSPACE_TRANSFER, workspace.id); event.dataTransfer.effectAllowed = 'move'
          }}
          onDragOver={(event) => { if (dragWorkspaceId && workspace.isPinned && dragWorkspaceId !== workspace.id) { event.preventDefault(); setDragOverId(`workspace:${workspace.id}`) } }}
          onDrop={(event) => {
            const sourceId = event.dataTransfer.getData(WORKSPACE_TRANSFER)
            if (sourceId && workspace.isPinned) void commands.reorderPinnedWorkspace(sourceId, workspace.id)
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
            <button className="workspace-group__add" aria-label={`在 ${workspace.name} 中新增事项`} title={invalid ? WORKSPACE_PATH_MESSAGE : '新增事项'}
              disabled={invalid} onClick={() => void commands.createTask(workspace.id)}><PlusIcon /></button>
            <button className="workspace-group__more" aria-label={`工作空间菜单：${workspace.name}`}
              onClick={(event) => openWorkspaceMenu(workspace, event)}>•••</button>
          </div>
          {!isCollapsed && <div className="workspace-group__tasks" role="list">
            {tasks.map((task) => <div role="listitem" key={task.id} data-testid={`task-${task.id}`}
              className={`workbench-item${task.id === activeTaskId ? ' is-active' : ''}${task.id === dragTaskId ? ' is-dragging' : ''}${dragOverId === `task:${task.id}` ? ' drag-over' : ''}`}
              tabIndex={0} aria-current={task.id === activeTaskId ? 'true' : undefined} draggable={Boolean(task.isPinned)}
              onDragStart={(event) => {
                if (!task.isPinned) return
                setDragTaskId(task.id); event.dataTransfer.setData(TASK_TRANSFER, JSON.stringify({ workspaceId: workspace.id, taskId: task.id })); event.dataTransfer.effectAllowed = 'move'
              }}
              onDragOver={(event) => { if (dragTaskId && task.isPinned && dragTaskId !== task.id) { event.preventDefault(); setDragOverId(`task:${task.id}`) } }}
              onDrop={(event) => {
                const source = parseTransfer(event.dataTransfer.getData(TASK_TRANSFER))
                if (source?.workspaceId === workspace.id && task.isPinned) void commands.reorderPinnedTask(workspace.id, source.taskId, task.id)
                resetDrag()
              }} onDragEnd={resetDrag}
              onClick={() => { notificationStore.markWorkspaceRead(workspace.id); void commands.activateTask(task.id) }}
              onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); void commands.activateTask(task.id) } }}
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
                    aria-label={`事项菜单：${task.title}`} title="更多操作" onClick={(event) => openTaskMenu(task, event)}>•••</button>
                </span>}
              </div>
            </div>)}
          </div>}
        </section>
      })}
    </nav>
    {menuWorkspace && <div role="menu" className="workbench-action-popover" style={{ top: menuPosition.top, left: menuPosition.left }} onPointerDown={(event) => event.stopPropagation()}>
      <button role="menuitem" onClick={() => {
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
      {!menuWorkspace.isDefault && <button role="menuitem" className="is-delete" onClick={() => { setRemoveWorkspace(menuWorkspace); setMenuWorkspace(null) }}><TrashIcon />移出码头</button>}
    </div>}
    {menuTask && <div role="menu" className="workbench-action-popover" style={{ top: menuPosition.top, left: menuPosition.left }} onPointerDown={(event) => event.stopPropagation()}>
      {closedTaskId === menuTask.id ? <>
        <button role="menuitem" onClick={() => setClosedTaskId(null)}>‹ 返回事项菜单</button>
        {(projection.closedScenes ?? []).filter(({ taskId }) => taskId === menuTask.id).map((scene) =>
          <button key={scene.id} role="menuitem" aria-label={`重新打开画布：${scene.name}`} onClick={() => {
            setClosedTaskId(null); setMenuTask(null)
            void Promise.resolve(commands.reopenScene?.(scene.id)).catch(NOOP)
          }}><span>↻</span>{scene.name}</button>)}
      </> : <>
        <button role="menuitem" onClick={() => { void commands.setTaskPinned(menuTask.id, !menuTask.isPinned); setMenuTask(null) }}><PinIcon />{menuTask.isPinned ? '取消置顶' : '置顶'}</button>
        <button role="menuitem" onClick={() => { setRenameFailure(null); setRenameTask(menuTask); setMenuTask(null) }}><EditIcon />重命名</button>
        {(projection.closedScenes ?? []).some(({ taskId }) => taskId === menuTask.id) && <button role="menuitem"
          onClick={() => setClosedTaskId(menuTask.id)}>已关闭画布 {(projection.closedScenes ?? []).filter(({ taskId }) => taskId === menuTask.id).length}</button>}
        <button role="menuitem" className="is-delete" onClick={() => { setDeleteTask(menuTask); setMenuTask(null) }}><TrashIcon />删除</button>
      </>}
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
    {toast && <div className="kooky-toast" role="status">{toast}</div>}
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
function PlusIcon() { return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M5 12h14"/><path d="M12 5v14"/></svg> }
function ComposeIcon() { return <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg> }
function ChevronIcon({ collapsed }: { collapsed: boolean }) { return <svg className="workspace-group__chevron" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d={collapsed ? 'm9 18 6-6-6-6' : 'm6 9 6 6 6-6'}/></svg> }
function FolderIcon({ home }: { home?: boolean }) { return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">{home ? <><path d="m3 11 9-8 9 8"/><path d="M5 10v10h14V10"/></> : <path d="M3 6h7l2 2h9v10H3Z"/>}</svg> }
function PinIcon() { return <svg className="pin-icon" data-icon="pushpin" aria-hidden="true" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M12 17v5"/><path d="M5 17h14"/><path d="M15 2.5a1 1 0 0 0-1 1V7a3 3 0 0 0 3 3v2H7v-2a3 3 0 0 0 3-3V3.5a1 1 0 0 0-1-1Z"/></svg> }
function EditIcon() { return <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg> }
function TrashIcon() { return <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="m19 6-1 14H6L5 6"/></svg> }
const WORKSPACE_PATH_MESSAGE = '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
function NOOP(): void {}
function parseTransfer(value: string): { workspaceId: string; taskId: string } | undefined {
  try {
    const parsed = JSON.parse(value) as Record<string, unknown>
    return typeof parsed.workspaceId === 'string' && typeof parsed.taskId === 'string'
      ? { workspaceId: parsed.workspaceId, taskId: parsed.taskId } : undefined
  } catch { return undefined }
}
