export interface ConfirmStep {
  title: string
  body: string
  confirmLabel: string
  cancelLabel: '取消'
}

export interface CloseFlow {
  action: 'confirm' | 'silent' | 'hide-window'
  steps: ConfirmStep[]
}

export function taskDeleteFlow(input: {
  sessionCount: number
  taskName: string
}): CloseFlow {
  const regular: ConfirmStep = {
    title: '删除事项',
    body: `删除“${input.taskName}”会丢失该事项下所有终端会话，但不会删除本地目录。是否继续？`,
    confirmLabel: '确认删除',
    cancelLabel: '取消'
  }
  return {
    action: 'confirm',
    steps: input.sessionCount <= 1
      ? [{
          title: '删除事项',
          body: '删除最后一个终端将连带删除对应事项，是否继续？',
          confirmLabel: '继续',
          cancelLabel: '取消'
        }, regular]
      : [regular]
  }
}

export function sceneCloseFlow(input: {
  isLastScene: boolean
  isLastTask: boolean
  taskName: string
}): CloseFlow {
  if (!input.isLastScene) return { action: 'silent', steps: [] }
  if (input.isLastTask) return { action: 'hide-window', steps: [] }
  return {
    action: 'confirm',
    steps: [{
      title: '关闭标签',
      body: `关闭此标签会丢失“${input.taskName}”中的所有终端会话，但不会删除本地目录。是否继续？`,
      confirmLabel: '确认关闭',
      cancelLabel: '取消'
    }]
  }
}

export function sessionDeleteFlow(input: {
  isWorkspaceFinal: boolean
  taskName: string
}): CloseFlow {
  if (!input.isWorkspaceFinal) return { action: 'silent', steps: [] }
  return {
    action: 'confirm',
    steps: [{
      title: '删除终端',
      body: `删除最后一个终端将连带销毁事项“${input.taskName}”，所有终端会话会丢失（本地目录不受影响）。是否继续？`,
      confirmLabel: '确认删除',
      cancelLabel: '取消'
    }]
  }
}
