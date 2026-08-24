import { useEffect, useRef, useState } from 'react'

import { SceneOverflowMenu } from './SceneOverflowMenu'
import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
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

export function SceneTabBar({ projection, commands, visibleLimit = 10, pathValid = true }: {
  projection: HierarchyProjection; commands: SceneCommands; visibleLimit?: number; pathValid?: boolean
}) {
  const workspaceId = projection.navigation.activeWorkspaceId
  const taskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const activeSceneId = taskId ? projection.navigation.sceneByTask[taskId] : undefined
  const scenes = projection.scenes.filter((scene) => scene.taskId === taskId)
  const visible = scenes.slice(0, visibleLimit)
  const overflow = scenes.slice(visibleLimit)
  const [overflowOpen, setOverflowOpen] = useState(false)
  const [closingSceneId, setClosingSceneId] = useState<string | null>(null)
  const [menuSceneId, setMenuSceneId] = useState<string | null>(null)
  const [renamingSceneId, setRenamingSceneId] = useState<string | null>(null)
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
      setClosingSceneId(sceneId)
    } else if (closeFlow.action === 'silent') {
      void Promise.resolve(commands.closeScene(sceneId, false)).catch(NOOP)
    } else {
      setClosingSceneId(sceneId)
    }
  }
  return <div className="scene-bar tab-bar" role="tablist" onKeyDown={(event) => {
    if (!(event.metaKey || event.ctrlKey) || !event.shiftKey || !activeSceneId) return
    const index = scenes.findIndex(({ id }) => id === activeSceneId)
    if (event.key === 'PageUp' && index > 0) void commands.reorderScene(activeSceneId, scenes[index - 1]!.id)
    if (event.key === 'PageDown' && index >= 0 && index < scenes.length - 1) {
      void commands.reorderScene(activeSceneId, scenes[index + 2]?.id)
    }
  }}>
    <div className="scene-tabs tab-bar-left">
      {visible.map((scene) => <div key={scene.id} data-scene-id={scene.id}
        className={`tab-item${scene.id === activeSceneId ? ' active' : ''}`}
        onContextMenu={(event) => { event.preventDefault(); setMenuSceneId(scene.id) }}>
        <button role="tab" ref={scene.id === activeSceneId ? activeRef : undefined}
          className="tab-title" aria-selected={scene.id === activeSceneId} onClick={() => select(scene.id)}>{scene.name}</button>
        <button className="tab-close" aria-label={`关闭页签：${scene.name}`} onClick={() => close(scene.id)}>✕</button>
      </div>)}
    </div>
    <div className="tab-bar-overflow-actions">
    {overflow.length > 0 && <div>
      <button className="tab-overflow-btn" aria-label="更多页签" onClick={() => setOverflowOpen(!overflowOpen)}>···</button>
      {overflowOpen && <SceneOverflowMenu scenes={overflow} onSelect={(scene) => {
        setOverflowOpen(false); select(scene.id, true)
      }} />}
    </div>}
    <button className="tab-add-btn" aria-label="新建页签" disabled={!pathValid} title={!pathValid ? WORKSPACE_PATH_MESSAGE : undefined}
      onClick={() => taskId && commands.createScene(taskId)}>+</button>
    </div>
    <div className="tab-bar-right">
    <button className="toolbar-btn split-horizontal-icon" aria-label="水平分屏" disabled={!pathValid || !activeSceneId || !activeSessionId}
      title={!pathValid ? WORKSPACE_PATH_MESSAGE : '水平分屏（左右）'}
      onClick={() => activeSceneId && activeSessionId && commands.splitSession(activeSceneId, activeSessionId, 'horizontal')}>↔</button>
    <button className="toolbar-btn split-vertical-icon" aria-label="垂直分屏" disabled={!pathValid || !activeSceneId || !activeSessionId}
      title={!pathValid ? WORKSPACE_PATH_MESSAGE : '垂直分屏（上下）'}
      onClick={() => activeSceneId && activeSessionId && commands.splitSession(activeSceneId, activeSessionId, 'vertical')}>↕</button>
    <span className="tab-bar-separator" />
    <button className="toolbar-btn file-panel-icon" aria-label="文件面板" title="文件面板">▱</button>
    </div>
    {menuSceneId && <div role="menu" className="scene-tab-menu">
      <button role="menuitem" onClick={() => { setRenamingSceneId(menuSceneId); setMenuSceneId(null) }}>重命名页签</button>
    </div>}
    {renamingSceneId && (() => {
      const scene = scenes.find(({ id }) => id === renamingSceneId)
      if (!scene) return null
      return <RenameDialog title="重命名标签页" label="页签名称" placeholder="输入标签页名称" initialValue={scene.name}
        error={(value) => scenes.some((candidate) =>
          candidate.id !== scene.id && candidate.titlePinned && candidate.name === value
        ) ? `当前事项下已存在名为"${value}"的标签页` : undefined}
        onCancel={() => setRenamingSceneId(null)} onConfirm={(name) => {
          setRenamingSceneId(null)
          void Promise.resolve(commands.renameScene(scene.id, name)).catch(NOOP)
        }} />
    })()}
    {closingSceneId && closeFlow.action === 'hide-window' && <ConfirmDialog title="提示"
      body={'当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。'}
      confirmLabel="我知道了" showCancel={false} onCancel={() => setClosingSceneId(null)}
      onConfirm={() => setClosingSceneId(null)} />}
    {closingSceneId && closeFlow.action !== 'hide-window' && <ConfirmationSequence steps={closeFlow.steps}
      onCancel={() => setClosingSceneId(null)} onComplete={() => {
        const sceneId = closingSceneId
        setClosingSceneId(null)
        void Promise.resolve(commands.closeScene(sceneId, true)).catch(NOOP)
      }} />}
  </div>
}

const WORKSPACE_PATH_MESSAGE = '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
function NOOP(): void {}
