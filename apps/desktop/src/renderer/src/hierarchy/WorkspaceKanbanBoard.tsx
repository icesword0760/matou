import { useEffect, useMemo, useRef, useState, type DragEvent } from 'react'

import type { TaskView, WorkspaceView } from './hierarchy-types'
import { useMessages } from '../i18n/LocaleProvider'

type BoardStatus = Exclude<NonNullable<TaskView['status']>, 'archived'>

const COLUMN_ORDER = ['planned', 'active', 'blocked', 'completed'] as const satisfies readonly BoardStatus[]

export function WorkspaceKanbanBoard({ workspace, tasks, activeTaskId, sessionCountByTask, onMoveTask }: {
  workspace: WorkspaceView
  tasks: TaskView[]
  activeTaskId?: string
  sessionCountByTask: Record<string, number>
  onMoveTask(taskId: string, status: BoardStatus, beforeTaskId?: string): unknown
}) {
  const m = useMessages().hierarchyShell.kanban
  const columns = COLUMN_ORDER.map((status) => ({ status, label: m.columns[status] }))
  const [visibleTasks, setVisibleTasks] = useState(() => ordered(tasks))
  const [draggedTaskId, setDraggedTaskId] = useState<string>()
  const [dropTarget, setDropTarget] = useState<{ status: BoardStatus; beforeTaskId?: string }>()
  const [feedback, setFeedback] = useState('')
  const taskSignature = useMemo(() => tasks.map(({ id, status, sortKey, updatedAt }) =>
    `${id}:${status ?? 'planned'}:${sortKey ?? ''}:${updatedAt ?? 0}`).join('|'), [tasks])
  const authoritativeRef = useRef(tasks)

  useEffect(() => {
    authoritativeRef.current = tasks
    setVisibleTasks(ordered(tasks))
  }, [taskSignature])
  useEffect(() => {
    if (!feedback) return
    const timer = window.setTimeout(() => setFeedback(''), 1_600)
    return () => window.clearTimeout(timer)
  }, [feedback])

  const move = (status: BoardStatus, beforeTaskId?: string) => {
    if (!draggedTaskId || draggedTaskId === beforeTaskId) return
    const previous = visibleTasks
    setVisibleTasks(moveOptimistically(previous, draggedTaskId, status, beforeTaskId))
    setDraggedTaskId(undefined)
    setDropTarget(undefined)
    setFeedback(m.moved(m.columns[status]))
    void Promise.resolve(onMoveTask(draggedTaskId, status, beforeTaskId)).catch(() => {
      setVisibleTasks(ordered(authoritativeRef.current))
      setFeedback(m.moveFailed)
    })
  }

  return <section className="workspace-board" aria-label={m.board(workspace.name)}>
    <header className="workspace-board__header">
      <div className="workspace-board__title">
        <h1>{m.board(workspace.name)}</h1>
        <span>{m.taskCount(visibleTasks.length)}</span>
      </div>
      <div className="workspace-board__guide"><i /><span>{m.dragHint}</span></div>
    </header>
    <div className="workspace-board__scroll custom-scrollbar">
      <div className="workspace-board__columns">
        {columns.map((column) => {
          const columnTasks = visibleTasks.filter((task) => taskStatus(task) === column.status)
          const columnDrop = dropTarget?.status === column.status && dropTarget.beforeTaskId === undefined
          return <section key={column.status} role="group" aria-label={m.column(column.label)}
            className={`board-column board-column--${column.status}${columnDrop ? ' is-drop-target' : ''}`}
            onDragOver={(event) => {
              event.preventDefault()
              event.dataTransfer.dropEffect = 'move'
              setDropTarget({ status: column.status })
            }}
            onDrop={(event) => { event.preventDefault(); move(column.status) }}>
            <header className="board-column__header">
              <i /><strong>{column.label}</strong><span>{m.columnCount(columnTasks.length)}</span>
            </header>
            <div className="board-column__cards">
              {columnTasks.map((task) => {
                const before = dropTarget?.status === column.status && dropTarget.beforeTaskId === task.id
                return <article key={task.id} aria-label={task.title} draggable
                  className={`board-task-card${task.id === activeTaskId ? ' is-active' : ''}${draggedTaskId === task.id ? ' is-dragging' : ''}${before ? ' is-drop-before' : ''}`}
                  onDragStart={(event) => {
                    setDraggedTaskId(task.id)
                    event.dataTransfer.effectAllowed = 'move'
                    event.dataTransfer.setData('text/plain', task.id)
                  }}
                  onDragEnd={() => { setDraggedTaskId(undefined); setDropTarget(undefined) }}
                  onDragOver={(event) => {
                    event.preventDefault(); event.stopPropagation()
                    event.dataTransfer.dropEffect = 'move'
                    setDropTarget({ status: column.status, beforeTaskId: task.id })
                  }}
                  onDrop={(event) => { event.preventDefault(); event.stopPropagation(); move(column.status, task.id) }}>
                  <h2>{task.title}</h2>
                  <footer>
                    <span><i />{m.sessionCount(sessionCountByTask[task.id] ?? 0)}</span>
                    {task.id === activeTaskId && <b>{m.current}</b>}
                  </footer>
                </article>
              })}
              {columnTasks.length === 0 && <div className="board-column__empty">{m.empty}</div>}
            </div>
          </section>
        })}
      </div>
    </div>
    {feedback && <div className="board-feedback" role="status">{feedback}</div>}
  </section>
}

function taskStatus(task: TaskView): BoardStatus {
  return !task.status || task.status === 'archived' ? 'planned' : task.status
}

function ordered(tasks: TaskView[]): TaskView[] {
  return [...tasks].sort((left, right) =>
    taskStatus(left).localeCompare(taskStatus(right)) ||
    (left.sortKey ?? '').localeCompare(right.sortKey ?? '') ||
    (left.createdAt ?? 0) - (right.createdAt ?? 0) || left.id.localeCompare(right.id))
}

function moveOptimistically(
  tasks: TaskView[], taskId: string, status: BoardStatus, beforeTaskId?: string
): TaskView[] {
  const source = tasks.find(({ id }) => id === taskId)
  if (!source) return tasks
  const remaining = tasks.filter(({ id }) => id !== taskId)
  const target = remaining.filter((task) => taskStatus(task) === status)
  const targetIndex = beforeTaskId ? target.findIndex(({ id }) => id === beforeTaskId) : target.length
  target.splice(targetIndex < 0 ? target.length : targetIndex, 0, { ...source, status })
  let targetCursor = 0
  const result = remaining.map((task) => taskStatus(task) === status ? target[targetCursor++]! : task)
  if (targetCursor < target.length) result.push(...target.slice(targetCursor))
  return ordered(result.map((task) => taskStatus(task) === status
    ? { ...task, sortKey: `a${target.indexOf(task).toString().padStart(8, '0')}` }
    : task))
}
