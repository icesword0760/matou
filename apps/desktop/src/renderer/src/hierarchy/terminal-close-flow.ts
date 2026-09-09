import { messages } from '../i18n/current'

export interface ConfirmStep {
  title: string
  body: string
  confirmLabel: string
  cancelLabel: string
  confirmTone?: 'default' | 'danger'
}

export interface CloseFlow {
  action: 'confirm' | 'silent' | 'hide-window'
  steps: ConfirmStep[]
}

export function taskDeleteFlow(input: {
  sessionCount: number
  taskName: string
}): CloseFlow {
  const all = messages()
  const m = all.hierarchyTerminal.closeFlow
  const regular: ConfirmStep = {
    title: all.hierarchyShell.notice,
    body: m.taskDeleteBody(input.taskName),
    confirmLabel: all.hierarchyShell.confirmOk,
    cancelLabel: all.common.cancel
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
  sceneName?: string
  sessionCount?: number
  runningCount?: number
  needsInputCount?: number
}): CloseFlow {
  if (input.isLastScene && input.isLastTask) return { action: 'hide-window', steps: [] }
  const all = messages()
  const m = all.hierarchyTerminal.closeFlow
  const runningCount = input.runningCount ?? 0
  const needsInputCount = input.needsInputCount ?? 0
  const affected = [
    runningCount > 0 ? m.runningSessions(runningCount) : '',
    needsInputCount > 0 ? m.needsInputSessions(needsInputCount) : ''
  ].filter(Boolean).join(m.affectedJoin)
  const activity = affected ? m.activity(affected) : ''
  return {
    action: 'confirm',
    steps: [{
      title: m.closeCanvas,
      body: m.sceneCloseBody(
        input.sceneName ?? all.hierarchyShell.sceneTabBar.currentCanvas,
        input.sessionCount ?? 0,
        activity
      ),
      confirmLabel: m.closeCanvas,
      confirmTone: 'danger',
      cancelLabel: all.common.cancel
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
  const active = input.workStatus === 'running' || input.workStatus === 'needs-input'
  if (!active && childCount === 0) return { action: 'silent', steps: [] }
  const all = messages()
  const m = all.hierarchyTerminal.closeFlow
  const activity = active ? m.running : m.idle
  const descendants = childCount > 0 ? m.childSessions(childCount) : ''
  return {
    action: 'confirm',
    steps: [{
      title: m.stopSession,
      body: m.sessionDeleteBody(input.sessionTitle ?? m.currentSession, activity, descendants),
      confirmLabel: m.stopSession, cancelLabel: all.common.cancel
    }]
  }
}
