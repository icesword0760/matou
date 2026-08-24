import { useEffect, useRef, useState } from 'react'

import { SceneOverflowMenu } from './SceneOverflowMenu'
import { ConfirmationSequence } from './ConfirmDialog'
import type { HierarchyProjection } from './hierarchy-types'
import { sceneCloseFlow } from './terminal-close-flow'

export interface SceneCommands {
  activateScene(sceneId: string): unknown
  createScene(taskId: string): unknown
  renameScene(sceneId: string, name: string): unknown
  reorderScene(sceneId: string, beforeSceneId?: string): unknown
  closeScene(sceneId: string, confirmed?: boolean): unknown
  splitSession(sceneId: string, sessionId: string, direction: 'horizontal' | 'vertical'): unknown
}

export function SceneTabBar({ projection, commands, visibleLimit = 10 }: {
  projection: HierarchyProjection; commands: SceneCommands; visibleLimit?: number
}) {
  const workspaceId = projection.navigation.activeWorkspaceId
  const taskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const activeSceneId = taskId ? projection.navigation.sceneByTask[taskId] : undefined
  const scenes = projection.scenes.filter((scene) => scene.taskId === taskId)
  const visible = scenes.slice(0, visibleLimit)
  const overflow = scenes.slice(visibleLimit)
  const [overflowOpen, setOverflowOpen] = useState(false)
  const [closingSceneId, setClosingSceneId] = useState<string | null>(null)
  const activeRef = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    activeRef.current?.scrollIntoView?.({ inline: 'nearest', block: 'nearest' })
  }, [activeSceneId])
  const activeSessionId = activeSceneId ? projection.navigation.sessionByScene[activeSceneId] : undefined
  const select = (sceneId: string, center = false) => {
    void commands.activateScene(sceneId)
    const tab = document.querySelector<HTMLElement>(`[data-scene-id="${sceneId}"]`)
    tab?.scrollIntoView({ inline: center ? 'center' : 'nearest', block: 'nearest' })
  }
  const task = projection.tasks.find(({ id }) => id === taskId)
  const workspaceTasks = projection.tasks.filter(({ workspaceId: candidate }) => candidate === workspaceId)
  const closeFlow = sceneCloseFlow({
    isLastScene: scenes.length === 1,
    isLastTask: workspaceTasks.length === 1,
    taskName: task?.title ?? '当前事项'
  })
  const close = (sceneId: string) => {
    if (closeFlow.action === 'hide-window') {
      void window.matouDesktop?.hideWindow(projection.windowId)
    } else if (closeFlow.action === 'silent') {
      void Promise.resolve(commands.closeScene(sceneId, false)).catch(NOOP)
    } else {
      setClosingSceneId(sceneId)
    }
  }
  return <div className="scene-bar" role="tablist" onKeyDown={(event) => {
    if (!(event.metaKey || event.ctrlKey) || !event.shiftKey || !activeSceneId) return
    const index = scenes.findIndex(({ id }) => id === activeSceneId)
    if (event.key === 'PageUp' && index > 0) void commands.reorderScene(activeSceneId, scenes[index - 1]!.id)
    if (event.key === 'PageDown' && index >= 0 && index < scenes.length - 1) {
      void commands.reorderScene(activeSceneId, scenes[index + 2]?.id)
    }
  }}>
    <div className="scene-tabs">
      {visible.map((scene) => <div key={scene.id} data-scene-id={scene.id}>
        <button role="tab" ref={scene.id === activeSceneId ? activeRef : undefined}
          aria-selected={scene.id === activeSceneId} onClick={() => select(scene.id)}>{scene.name}</button>
        <button aria-label={`关闭页签：${scene.name}`} onClick={() => close(scene.id)}>×</button>
      </div>)}
    </div>
    {overflow.length > 0 && <div>
      <button aria-label="更多页签" onClick={() => setOverflowOpen(!overflowOpen)}>…</button>
      {overflowOpen && <SceneOverflowMenu scenes={overflow} onSelect={(scene) => {
        setOverflowOpen(false); select(scene.id, true)
      }} />}
    </div>}
    <button aria-label="新建页签" onClick={() => taskId && commands.createScene(taskId)}>+</button>
    <button aria-label="水平分屏" disabled={!activeSceneId || !activeSessionId}
      onClick={() => activeSceneId && activeSessionId && commands.splitSession(activeSceneId, activeSessionId, 'horizontal')}>↔</button>
    <button aria-label="垂直分屏" disabled={!activeSceneId || !activeSessionId}
      onClick={() => activeSceneId && activeSessionId && commands.splitSession(activeSceneId, activeSessionId, 'vertical')}>↕</button>
    {closingSceneId && <ConfirmationSequence steps={closeFlow.steps}
      onCancel={() => setClosingSceneId(null)} onComplete={() => {
        const sceneId = closingSceneId
        setClosingSceneId(null)
        void Promise.resolve(commands.closeScene(sceneId, true)).catch(NOOP)
      }} />}
  </div>
}

function NOOP(): void {}
