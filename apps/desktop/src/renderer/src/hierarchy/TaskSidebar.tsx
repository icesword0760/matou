import { useEffect, useRef, useState } from 'react'

import { ConfirmationSequence } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
import type { HierarchyCommands, HierarchyProjection, TaskView } from './hierarchy-types'
import { taskDeleteFlow } from './terminal-close-flow'

const TASK_TRANSFER = 'application/x-matou-task'

export function TaskSidebar({ projection, commands }: {
  projection: HierarchyProjection; commands: HierarchyCommands
}) {
  const workspaceId = projection.navigation.activeWorkspaceId
  const placedIds = new Set(projection.taskPlacements
    .filter(({ windowId }) => windowId === projection.windowId)
    .map(({ taskId }) => taskId))
  const tasks = projection.tasks.filter((task) =>
    task.workspaceId === workspaceId && (projection.taskPlacements.length === 0 || placedIds.has(task.id))
  )
  const focusedTaskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const activeTaskId = tasks.some(({ id }) => id === focusedTaskId) ? focusedTaskId : tasks[0]?.id
  const [menuTask, setMenuTask] = useState<TaskView | null>(null)
  const [renameTask, setRenameTask] = useState<TaskView | null>(null)
  const [deleteTask, setDeleteTask] = useState<TaskView | null>(null)
  const activeRef = useRef<HTMLButtonElement>(null)
  useEffect(() => activeRef.current?.scrollIntoView?.({ block: 'nearest' }), [activeTaskId])
  return <aside aria-label="事项列表">
    <button onClick={() => workspaceId && commands.createTask(workspaceId)}>+ 新事项</button>
    <div role="list">
      {tasks.map((task) => <div role="listitem" key={task.id} data-testid={`task-${task.id}`}
        draggable onDragStart={(event) => event.dataTransfer.setData(TASK_TRANSFER, JSON.stringify({ workspaceId, taskId: task.id }))}
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => {
          const source = parseTransfer(event.dataTransfer.getData(TASK_TRANSFER))
          if (source && source.workspaceId === workspaceId) {
            void commands.reorderTask(source.taskId, task.id)
          }
        }}>
        <button ref={task.id === activeTaskId ? activeRef : undefined}
          aria-current={task.id === activeTaskId ? 'page' : undefined}
          onClick={() => commands.activateTask(task.id)}>{task.title}</button>
        <button aria-label={`事项菜单：${task.title}`} onClick={() => setMenuTask(task)}>•••</button>
      </div>)}
    </div>
    {menuTask && <div role="menu">
      <button role="menuitem" onClick={() => { setRenameTask(menuTask); setMenuTask(null) }}>重命名</button>
      <button role="menuitem" onClick={() => { setDeleteTask(menuTask); setMenuTask(null) }}>删除事项</button>
    </div>}
    {renameTask && <RenameDialog label="事项名称" initialValue={renameTask.title}
      error={(value) => tasks.some((task) => task.id !== renameTask.id && task.title === value)
        ? `当前工作区下已存在名为“${value}”的事项` : undefined}
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

function NOOP(): void {}

function parseTransfer(value: string): { workspaceId: string; taskId: string } | undefined {
  try {
    const parsed = JSON.parse(value) as Record<string, unknown>
    return typeof parsed.workspaceId === 'string' && typeof parsed.taskId === 'string'
      ? { workspaceId: parsed.workspaceId, taskId: parsed.taskId } : undefined
  } catch { return undefined }
}
