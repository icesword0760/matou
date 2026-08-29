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
    title: '提示',
    body: `删除 "${input.taskName}" 会丢失该事项下所有终端会话，但不会删除本地目录。 是否继续？`,
    confirmLabel: '确定',
    cancelLabel: '取消'
  }
  return {
    action: 'confirm',
    steps: [regular]
  }
}

export function sceneCloseFlow(input: {
  isLastScene: boolean
  isLastTask: boolean
  taskName: string
  runningCount?: number
  needsInputCount?: number
}): CloseFlow {
  if (input.isLastScene && input.isLastTask) return { action: 'hide-window', steps: [] }
  const runningCount = input.runningCount ?? 0
  const needsInputCount = input.needsInputCount ?? 0
  if (runningCount === 0 && needsInputCount === 0) return { action: 'silent', steps: [] }
  const affected = [
    runningCount > 0 ? `${runningCount} 个运行中会话` : '',
    needsInputCount > 0 ? `${needsInputCount} 个待输入会话` : ''
  ].filter(Boolean).join('和 ')
  return {
    action: 'confirm',
    steps: [{
      title: '关闭画布',
      body: `这张画布中有 ${affected}。关闭后这些会话将结束，画布、关系和历史摘要会进入“已关闭画布”，本地目录和工作树保持原样。`,
      confirmLabel: '确认关闭',
      cancelLabel: '取消'
    }]
  }
}

export function sessionDeleteFlow(input: {
  isWorkspaceFinal: boolean
  taskName: string
  sessionTitle?: string
  workStatus?: 'starting' | 'idle' | 'running' | 'needs-input' | 'error' | 'interrupted' | 'exited'
  childCount?: number
}): CloseFlow {
  if (input.isWorkspaceFinal) return { action: 'hide-window', steps: [] }
  const childCount = input.childCount ?? 0
  const active = input.workStatus === 'running' || input.workStatus === 'starting' || input.workStatus === 'needs-input'
  if (!active && childCount === 0) return { action: 'silent', steps: [] }
  const activity = active ? '正在运行' : '当前空闲'
  const descendants = childCount > 0 ? `，并有 ${childCount} 个子会话` : ''
  return {
    action: 'confirm',
    steps: [{
      title: '结束会话',
      body: `“${input.sessionTitle ?? '当前会话'}”${activity}${descendants}。结束后节点将在 DAG 中保留为历史，子会话继续工作。`,
      confirmLabel: '结束会话', cancelLabel: '取消'
    }]
  }
}
