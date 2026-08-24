import { useEffect, useRef, useState } from 'react'

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
  const [deleteTask, setDeleteTask] = useState<TaskView | null>(null)
  const activeRef = useRef<HTMLSpanElement>(null)
  useEffect(() => {
    activeRef.current?.scrollIntoView?.({ block: 'nearest' })
  }, [activeTaskId])
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
        className={`workbench-item${task.id === activeTaskId ? ' is-active' : ''}`} tabIndex={0}
        aria-current={task.id === activeTaskId ? 'true' : undefined}
        draggable onDragStart={(event) => event.dataTransfer.setData(TASK_TRANSFER, JSON.stringify({ workspaceId, taskId: task.id }))}
        onDragOver={(event) => event.preventDefault()}
        onClick={() => commands.activateTask(task.id)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault(); void commands.activateTask(task.id)
          }
        }}
        onContextMenu={(event) => { event.preventDefault(); setMenuTask(task) }}
        onDrop={(event) => {
          const source = parseTransfer(event.dataTransfer.getData(TASK_TRANSFER))
          if (source && source.workspaceId === workspaceId) {
            void commands.reorderTask(workspaceId!, source.taskId, task.id)
          }
        }}>
        <div className="workbench-item__left">
          <span className="workbench-item__icon" style={{ maskImage: `url(${workbenchIcon})`, WebkitMaskImage: `url(${workbenchIcon})` }} />
          <span className="workbench-item__name" data-testid={task.id === activeTaskId ? 'active-task' : undefined}
            ref={task.id === activeTaskId ? activeRef : undefined}>{task.title}</span>
        </div>
        <div className="workbench-item__right" onClick={(event) => event.stopPropagation()}>
          <button className={`workbench-item__more-btn${menuTask?.id === task.id ? ' is-open' : ''}`}
            aria-label={`事项菜单：${task.title}`} title="更多操作" onClick={() => setMenuTask(task)}>•••</button>
        </div>
      </div>)}
      </nav>
    </div>
    {menuTask && <div role="menu" className="workbench-action-popover">
      <button role="menuitem" onClick={() => { setRenameTask(menuTask); setMenuTask(null) }}>重命名</button>
      <button role="menuitem" className="is-delete" onClick={() => { setDeleteTask(menuTask); setMenuTask(null) }}>删除</button>
    </div>}
    {renameTask && <RenameDialog label="事项名称" placeholder="请输入事项名称" initialValue={renameTask.title}
      error={(value) => tasks.some((task) => task.id !== renameTask.id && task.title === value)
        ? `当前工作区下已存在名为"${value}"的工作台` : undefined}
      onCancel={() => setRenameTask(null)} onConfirm={(title) => {
        void commands.renameTask(renameTask.id, title); setRenameTask(null)
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
  </aside>
}

function PlusIcon() {
  return <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M5 12h14"/><path d="M12 5v14"/></svg>
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
