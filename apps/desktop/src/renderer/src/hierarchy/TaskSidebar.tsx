import { useEffect, useRef, useState, type MouseEvent } from 'react'

import { ConfirmationSequence } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
import type { HierarchyCommands, HierarchyProjection, TaskView } from './hierarchy-types'
import { taskDeleteFlow } from './terminal-close-flow'
import { WorkspaceSwitcher } from './WorkspaceSwitcher'
import workbenchIcon from '../assets/kooky/terminal/dark_lujing.svg'

const TASK_TRANSFER = 'application/x-matou-task'

export function TaskSidebar({ projection, commands, pathValid = true }: {
  projection: HierarchyProjection; commands: HierarchyCommands; pathValid?: boolean
}) {
  const workspaceId = projection.navigation.activeWorkspaceId
  const placedIds = new Set(projection.taskPlacements
    .filter(({ windowId }) => windowId === projection.windowId)
    .map(({ taskId }) => taskId))
  const tasks = projection.tasks.filter((task) =>
    task.workspaceId === workspaceId && (projection.taskPlacements.length === 0 || placedIds.has(task.id))
  ).sort((left, right) => (left.sortKey ?? '').localeCompare(right.sortKey ?? '') || left.id.localeCompare(right.id))
  const focusedTaskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const activeTaskId = tasks.some(({ id }) => id === focusedTaskId) ? focusedTaskId : tasks[0]?.id
  const [menuTask, setMenuTask] = useState<TaskView | null>(null)
  const [renameTask, setRenameTask] = useState<TaskView | null>(null)
  const [renameFailure, setRenameFailure] = useState<{ title: string; message: string } | null>(null)
  const [deleteTask, setDeleteTask] = useState<TaskView | null>(null)
  const [dragTaskId, setDragTaskId] = useState<string | null>(null)
  const [dragOverTaskId, setDragOverTaskId] = useState<string | null>(null)
  const [menuPosition, setMenuPosition] = useState({ top: 0, left: 0 })
  const [toast, setToast] = useState('')
  const activeRef = useRef<HTMLSpanElement>(null)
  useEffect(() => {
    activeRef.current?.scrollIntoView?.({ block: 'nearest' })
  }, [activeTaskId])
  useEffect(() => {
    const closeMenus = (event: PointerEvent) => {
      const target = event.target
      if (target instanceof Node && !document.querySelector('.workbench-action-popover')?.contains(target)) {
        setMenuTask(null)
      }
    }
    const onEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setMenuTask(null)
    }
    window.addEventListener('pointerdown', closeMenus)
    window.addEventListener('keydown', onEscape)
    return () => {
      window.removeEventListener('pointerdown', closeMenus)
      window.removeEventListener('keydown', onEscape)
    }
  }, [])
  useEffect(() => {
    if (!toast) return
    const timer = window.setTimeout(() => setToast(''), 2_000)
    return () => window.clearTimeout(timer)
  }, [toast])

  const resetDrag = () => { setDragTaskId(null); setDragOverTaskId(null) }
  const openMenu = (task: TaskView, event: MouseEvent<HTMLElement>) => {
    if (menuTask?.id === task.id) { setMenuTask(null); return }
    const rect = event.currentTarget.getBoundingClientRect()
    setMenuPosition({ top: rect.top + rect.height / 2, left: rect.right + 6 })
    setMenuTask(task)
  }
  const unreadCount = (taskId: string) => projection.unreadByTask?.[taskId] ?? 0
  return <aside className="workbench-sidebar" aria-label="事项列表">
    <div className="workbench-sidebar__project-stack">
      <div className="workbench-sidebar__project" data-testid="workspace-name">
        <WorkspaceSwitcher projection={projection} commands={commands} />
      </div>
    </div>
    <div className="workbench-sidebar__header">
      <button className="workbench-sidebar__add-btn" disabled={!pathValid}
        title={!pathValid ? WORKSPACE_PATH_MESSAGE : undefined}
        onClick={() => workspaceId && commands.createTask(workspaceId)}>
        <PlusIcon /><span>事项</span>
      </button>
    </div>
    <div className="workbench-sidebar__list custom-scrollbar" role="list">
      <nav className="workbench-sidebar__nav">
      {tasks.map((task) => <div role="listitem" key={task.id} data-testid={`task-${task.id}`}
        className={`workbench-item${task.id === activeTaskId ? ' is-active' : ''}${task.id === dragTaskId ? ' is-dragging' : ''}${task.id === dragOverTaskId ? ' drag-over' : ''}`} tabIndex={0}
        aria-current={task.id === activeTaskId ? 'true' : undefined}
        draggable onDragStart={(event) => {
          setDragTaskId(task.id)
          event.dataTransfer.setData(TASK_TRANSFER, JSON.stringify({ workspaceId, taskId: task.id }))
          event.dataTransfer.effectAllowed = 'move'
        }}
        onDragOver={(event) => {
          event.preventDefault()
          if (dragTaskId && dragTaskId !== task.id) setDragOverTaskId(task.id)
        }}
        onDragLeave={() => { if (dragOverTaskId === task.id) setDragOverTaskId(null) }}
        onDragEnd={resetDrag}
        onClick={() => commands.activateTask(task.id)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault(); void commands.activateTask(task.id)
          }
        }}
        onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); openMenu(task, event) }}
        onDrop={(event) => {
          const source = parseTransfer(event.dataTransfer.getData(TASK_TRANSFER))
          if (source && source.workspaceId === workspaceId) {
            void commands.reorderTask(workspaceId!, source.taskId, task.id)
          }
          resetDrag()
        }}>
        <div className="workbench-item__left">
          <span className="workbench-item__icon" style={{ maskImage: `url(${workbenchIcon})`, WebkitMaskImage: `url(${workbenchIcon})` }} />
          <span className="workbench-item__name" data-testid={task.id === activeTaskId ? 'active-task' : undefined}
            ref={task.id === activeTaskId ? activeRef : undefined}>{task.title}</span>
        </div>
        <div className="workbench-item__right" onClick={(event) => event.stopPropagation()}>
          {unreadCount(task.id) > 0
            ? <span className="workbench-item__badge">{unreadCount(task.id) > 99 ? '99+' : unreadCount(task.id)}</span>
            : <button className={`workbench-item__more-btn${menuTask?.id === task.id ? ' is-open' : ''}`}
              aria-label={`事项菜单：${task.title}`} title="更多操作" onClick={(event) => openMenu(task, event)}>•••</button>}
        </div>
      </div>)}
      </nav>
    </div>
    {menuTask && <div role="menu" className="workbench-action-popover"
      style={{ top: menuPosition.top, left: menuPosition.left }} onPointerDown={(event) => event.stopPropagation()}>
      <button role="menuitem" onClick={() => { setRenameFailure(null); setRenameTask(menuTask); setMenuTask(null) }}><EditIcon />重命名</button>
      <button role="menuitem" className="is-delete" onClick={() => { setDeleteTask(menuTask); setMenuTask(null) }}><TrashIcon />删除</button>
    </div>}
    {renameTask && <RenameDialog label="事项名称" placeholder="请输入事项名称" emptyError="工作台名称不能为空" initialValue={renameTask.title}
      error={(value) => tasks.some((task) => task.id !== renameTask.id && task.title === value)
        ? `当前工作区下已存在名为"${value}"的工作台`
        : renameFailure?.title === value ? renameFailure.message : undefined}
      onCancel={() => setRenameTask(null)} onConfirm={(title) => {
        void Promise.resolve(commands.renameTask(renameTask.id, title)).then(() => {
          setRenameTask(null); setToast('工作台已重命名')
        }).catch(() => setRenameFailure({ title, message: '重命名失败：名称为空或已存在' }))
      }} />}
    {deleteTask && <ConfirmationSequence
      steps={taskDeleteFlow({
        taskName: deleteTask.title,
        sessionCount: projection.sessions.filter(({ taskId }) => taskId === deleteTask.id).length
      }).steps}
      onCancel={() => setDeleteTask(null)} onComplete={() => {
        const taskId = deleteTask.id
        setDeleteTask(null)
        void Promise.resolve(commands.deleteTask(taskId)).catch(NOOP)
      }} />}
    {toast && <div className="kooky-toast" role="status">{toast}</div>}
  </aside>
}

function PlusIcon() {
  return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>
}

function EditIcon() {
  return <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"/></svg>
}

function TrashIcon() {
  return <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><path d="M3 6h18"/><path d="M8 6V4h8v2"/><path d="m19 6-1 14H6L5 6"/></svg>
}

const WORKSPACE_PATH_MESSAGE = '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
function NOOP(): void {}

function parseTransfer(value: string): { workspaceId: string; taskId: string } | undefined {
  try {
    const parsed = JSON.parse(value) as Record<string, unknown>
    return typeof parsed.workspaceId === 'string' && typeof parsed.taskId === 'string'
      ? { workspaceId: parsed.workspaceId, taskId: parsed.taskId } : undefined
  } catch { return undefined }
}
