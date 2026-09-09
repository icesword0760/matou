import { useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

import { ConfirmationSequence, ConfirmDialog } from './ConfirmDialog'
import { RenameDialog } from './RenameDialog'
import type { HierarchyProjection } from './hierarchy-types'
import { sceneCloseFlow } from './terminal-close-flow'
import { useMessages } from '../i18n/LocaleProvider'
import { useNotificationSnapshot, useNotificationStore } from '../notifications/NotificationProvider'
import { AppIcon } from '../ui/AppIcon'

export interface SceneCommands {
  activateScene(sceneId: string): unknown
  createScene(taskId: string): unknown
  createCanvas?(taskId: string): unknown
  renameScene(sceneId: string, name: string): unknown
  reorderScene(sceneId: string, beforeSceneId?: string): unknown
  closeScene(sceneId: string, confirmed?: boolean): unknown
  splitSession(sceneId: string, sessionId: string, direction: 'horizontal' | 'vertical'): unknown
  createShellSibling?(sceneId: string, sessionId: string, parentSessionId?: string): unknown
}

export function SceneTabBar({ projection, commands, pathValid = true, readOnly = false, levelParentSessionId, onOpenDag, trailingControl }: {
  projection: HierarchyProjection
  commands: SceneCommands
  pathValid?: boolean
  readOnly?: boolean
  levelParentSessionId?: string
  onOpenDag?(): void
  trailingControl?: ReactNode
}) {
  const shell = useMessages().hierarchyShell
  const m = shell.sceneTabBar
  const workspaceId = projection.navigation.activeWorkspaceId
  const taskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const activeSceneId = taskId ? projection.navigation.sceneByTask[taskId] : undefined
  const scenes = projection.scenes.filter((scene) => scene.taskId === taskId)
    .map((scene) => ({ ...scene, name: sceneDisplayName(scene, projection) }))
  const [closingSceneId, setClosingSceneId] = useState<string | null>(null)
  const [sceneMenu, setSceneMenu] = useState<{ sceneId: string; x: number; y: number } | null>(null)
  const sceneMenuRef = useRef<HTMLDivElement>(null)
  const [renamingSceneId, setRenamingSceneId] = useState<string | null>(null)
  const [isTabOverflowing, setIsTabOverflowing] = useState(false)
  const [tabOverflowVisible, setTabOverflowVisible] = useState(false)
  const [hiddenSceneIds, setHiddenSceneIds] = useState<string[]>([])
  const tabBarLeftRef = useRef<HTMLDivElement>(null)
  const tabItemRefs = useRef(new Map<string, HTMLDivElement>())
  const notificationStore = useNotificationStore()
  useNotificationSnapshot()
  const activeRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    activeRef.current?.scrollIntoView?.({ inline: 'nearest', block: 'nearest' })
  }, [activeSceneId])
  useEffect(() => {
    const container = tabBarLeftRef.current
    if (!container) return
    const check = () => {
      const overflowing = container.scrollWidth - container.clientWidth > 4
      setIsTabOverflowing(overflowing)
      if (!overflowing) {
        setTabOverflowVisible(false)
        setHiddenSceneIds([])
      }
    }
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(check)
    observer?.observe(container)
    window.addEventListener('resize', check)
    check()
    const frame = requestAnimationFrame(check)
    return () => {
      cancelAnimationFrame(frame)
      observer?.disconnect()
      window.removeEventListener('resize', check)
    }
  }, [scenes.length])
  useEffect(() => {
    if (!sceneMenu) return
    const closeOutside = (event: Event) => {
      const target = event.target
      if (target instanceof Node && sceneMenuRef.current?.contains(target)) return
      setSceneMenu(null)
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setSceneMenu(null)
    }
    window.addEventListener('pointerdown', closeOutside, true)
    window.addEventListener('contextmenu', closeOutside, true)
    window.addEventListener('keydown', closeOnEscape)
    return () => {
      window.removeEventListener('pointerdown', closeOutside, true)
      window.removeEventListener('contextmenu', closeOutside, true)
      window.removeEventListener('keydown', closeOnEscape)
    }
  }, [sceneMenu])
  const activeSessionId = activeSceneId ? projection.navigation.sessionByScene[activeSceneId] : undefined
  const select = (sceneId: string, center = false) => {
    void commands.activateScene(sceneId)
    const tab = document.querySelector<HTMLElement>(`[data-scene-id="${sceneId}"]`)
    tab?.scrollIntoView({
      inline: center ? 'center' : 'nearest', block: 'nearest',
      ...(center ? { behavior: 'smooth' as const } : {})
    })
  }
  const addCanvas = () => {
    if (readOnly || !taskId) return
    if (commands.createCanvas) commands.createCanvas(taskId)
    else commands.createScene(taskId)
  }
  const refreshHiddenTabs = () => {
    const container = tabBarLeftRef.current
    if (!container || !isTabOverflowing) {
      setHiddenSceneIds([])
      return
    }
    const containerRect = container.getBoundingClientRect()
    setHiddenSceneIds(scenes.filter(({ id }) => {
      const element = tabItemRefs.current.get(id)
      if (!element) return true
      const rect = element.getBoundingClientRect()
      return rect.right > containerRect.right + 1 || rect.left < containerRect.left - 1
    }).map(({ id }) => id))
  }
  const task = projection.tasks.find(({ id }) => id === taskId)
  const sceneHasUnread = (sceneId: string) => {
    const sessionIds = projection.sceneSnapshots?.find(({ scene }) => scene.id === sceneId)?.mounts
      .map(({ sessionId }) => sessionId) ?? []
    return sessionIds.some((sessionId) => notificationStore.sessionHasUnread(sessionId))
  }
  const workspaceTasks = projection.tasks.filter(({ workspaceId: candidate }) => candidate === workspaceId)
  const closeFlowFor = (sceneId: string) => {
    const scene = scenes.find(({ id }) => id === sceneId)
    const nodes = projection.sessionGraphs?.[sceneId]?.nodes.filter(({ archivedAt }) => archivedAt === undefined) ?? []
    return sceneCloseFlow({
      isLastScene: scenes.length === 1,
      isLastTask: workspaceTasks.length === 1,
      taskName: task?.title ?? m.currentTask,
      sceneName: scene?.name ?? m.currentCanvas,
      sessionCount: nodes.length,
      runningCount: nodes.filter(({ workStatus }) => workStatus === 'running' || workStatus === 'starting').length,
      needsInputCount: nodes.filter(({ workStatus }) => workStatus === 'needs-input').length
    })
  }
  const close = (sceneId: string) => {
    if (readOnly) return
    const closeFlow = closeFlowFor(sceneId)
    if (closeFlow.action === 'hide-window') {
      setClosingSceneId(sceneId)
    } else if (closeFlow.action === 'silent') {
      void Promise.resolve(commands.closeScene(sceneId, false)).catch(NOOP)
    } else {
      setClosingSceneId(sceneId)
    }
  }
  return <div className="scene-bar tab-bar" role="tablist" onKeyDown={(event) => {
    if (readOnly || !(event.metaKey || event.ctrlKey) || !event.shiftKey || !activeSceneId) return
    const index = scenes.findIndex(({ id }) => id === activeSceneId)
    if (event.key === 'PageUp' && index > 0) void commands.reorderScene(activeSceneId, scenes[index - 1]!.id)
    if (event.key === 'PageDown' && index >= 0 && index < scenes.length - 1) {
      void commands.reorderScene(activeSceneId, scenes[index + 2]?.id)
    }
  }}>
    <div ref={tabBarLeftRef} className="scene-tabs tab-bar-left">
      {scenes.map((scene) => <div key={scene.id} data-scene-id={scene.id}
        ref={(element) => {
          if (element) tabItemRefs.current.set(scene.id, element)
          else tabItemRefs.current.delete(scene.id)
          if (scene.id === activeSceneId) activeRef.current = element
        }}
        className={`tab-item${scene.id === activeSceneId ? ' active' : ''}`}
        onContextMenu={(event) => {
          event.preventDefault()
          event.stopPropagation()
          if (!readOnly) setSceneMenu(canvasMenuPosition(scene.id, event.clientX, event.clientY))
        }}>
        <button role="tab" className="tab-title" aria-selected={scene.id === activeSceneId}
          title={readOnly ? scene.name : m.tabHint(scene.name)}
          onDoubleClick={() => { if (!readOnly) setRenamingSceneId(scene.id) }}
          onClick={() => select(scene.id)}>{scene.name}</button>
        {sceneHasUnread(scene.id) && <span className="tab-status-dot" data-testid={`scene-unread-${scene.id}`} />}
        <button className="tab-close" aria-label={m.closeTab(scene.name)}
          disabled={readOnly} title={readOnly ? shell.readOnlyRecoveryReason : undefined}
          onClick={() => close(scene.id)}><AppIcon name="x" /></button>
      </div>)}
      {!isTabOverflowing && <button className="tab-add-btn" aria-label={m.newTab}
        disabled={readOnly || !pathValid}
        title={readOnly ? shell.readOnlyRecoveryReason : !pathValid ? shell.workspacePathUnavailable : undefined}
        onClick={addCanvas}><AppIcon name="plus" /></button>}
    </div>
    {isTabOverflowing && <div className="tab-bar-overflow-actions">
      <button className="tab-overflow-btn" aria-label={m.moreTabs} title={m.showHiddenTabs}
        onClick={(event) => {
          event.stopPropagation()
          refreshHiddenTabs()
          setTabOverflowVisible((visible) => !visible)
        }}><AppIcon name="ellipsis" /></button>
      <button className="tab-add-btn" aria-label={m.newTab}
        disabled={readOnly || !pathValid}
        title={readOnly ? shell.readOnlyRecoveryReason : !pathValid ? shell.workspacePathUnavailable : undefined}
        onClick={addCanvas}><AppIcon name="plus" /></button>
    </div>}
    <div className="tab-bar-right">
    {onOpenDag && <button className="toolbar-btn dag-canvas-icon" aria-label={m.openDag}
      title={m.openDagHint} onClick={onOpenDag}><AppIcon name="graph-ring" /></button>}
    <button className="toolbar-btn split-horizontal-icon" aria-label={m.addShell}
      disabled={readOnly || !pathValid || !activeSceneId || !activeSessionId}
      title={readOnly ? shell.readOnlyRecoveryReason : !pathValid ? shell.workspacePathUnavailable : m.addShell}
      onClick={() => {
        if (!activeSceneId || !activeSessionId) return
        if (commands.createShellSibling) {
          if (levelParentSessionId) commands.createShellSibling(activeSceneId, activeSessionId, levelParentSessionId)
          else commands.createShellSibling(activeSceneId, activeSessionId)
        }
        else commands.splitSession(activeSceneId, activeSessionId, 'horizontal')
      }}>
      <AppIcon name="panel-right-open" />
    </button>
    {trailingControl}
    </div>
    {sceneMenu && createPortal(<div ref={sceneMenuRef} role="menu" className="scene-tab-menu"
      style={{ left: sceneMenu.x, top: sceneMenu.y }}>
      <button role="menuitem" disabled={readOnly} title={readOnly ? shell.readOnlyRecoveryReason : undefined} onClick={() => {
        setRenamingSceneId(sceneMenu.sceneId)
        setSceneMenu(null)
      }}>{m.renameTabAction}</button>
    </div>, document.body)}
    {tabOverflowVisible && createPortal(<>
      <div className="tab-overflow-mask" onMouseDown={() => setTabOverflowVisible(false)} />
      <div role="menu" aria-label={m.hiddenTabs} className="tab-overflow-panel" style={tabOverflowPanelStyle(tabBarLeftRef.current)}>
        {hiddenSceneIds.map((sceneId) => {
          const scene = scenes.find(({ id }) => id === sceneId)
          if (!scene) return null
          return <button key={scene.id} role="menuitem" className={`tab-overflow-item${scene.id === activeSceneId ? ' is-active' : ''}`}
            onClick={() => {
              setTabOverflowVisible(false)
              select(scene.id, true)
            }}>
            {sceneHasUnread(scene.id) && <span className="tab-overflow-dot" />}
            <span className="tab-overflow-title">{scene.name}</span>
            {scene.id === activeSceneId && <span className="tab-overflow-check">✓</span>}
          </button>
        })}
      </div>
    </>, document.body)}
    {renamingSceneId && (() => {
      const scene = scenes.find(({ id }) => id === renamingSceneId)
      if (!scene) return null
      return <RenameDialog title={m.renameTabTitle} label={m.tabName} placeholder={m.tabNamePlaceholder} initialValue={scene.name}
        error={(value) => scenes.some((candidate) =>
          candidate.id !== scene.id && candidate.titlePinned && candidate.name === value
        ) ? m.tabNameTaken(value) : undefined}
        onCancel={() => setRenamingSceneId(null)} onConfirm={(name) => {
          setRenamingSceneId(null)
          void Promise.resolve(commands.renameScene(scene.id, name)).catch(NOOP)
        }} />
    })()}
    {closingSceneId && closeFlowFor(closingSceneId).action === 'hide-window' && <ConfirmDialog title={shell.notice}
      body={m.lastTabBody}
      confirmLabel={m.lastTabConfirm} showCancel={false} onCancel={() => setClosingSceneId(null)}
      onConfirm={() => setClosingSceneId(null)} />}
    {closingSceneId && closeFlowFor(closingSceneId).action !== 'hide-window' && <ConfirmationSequence steps={closeFlowFor(closingSceneId).steps}
      onCancel={() => setClosingSceneId(null)} onComplete={() => {
        const sceneId = closingSceneId
        setClosingSceneId(null)
        void Promise.resolve(commands.closeScene(sceneId, true)).catch(NOOP)
      }} />}
  </div>
}

const CANVAS_MENU_WIDTH = 128
const CANVAS_MENU_HEIGHT = 36
const CANVAS_MENU_MARGIN = 8

function canvasMenuPosition(sceneId: string, clientX: number, clientY: number) {
  return {
    sceneId,
    x: Math.max(CANVAS_MENU_MARGIN, Math.min(clientX, window.innerWidth - CANVAS_MENU_WIDTH - CANVAS_MENU_MARGIN)),
    y: Math.max(CANVAS_MENU_MARGIN, Math.min(clientY, window.innerHeight - CANVAS_MENU_HEIGHT - CANVAS_MENU_MARGIN))
  }
}

function tabOverflowPanelStyle(element: HTMLElement | null) {
  if (!element) return { top: 48, right: 80 }
  const rect = element.getBoundingClientRect()
  return { top: rect.bottom + 4, right: window.innerWidth - rect.right }
}
function NOOP(): void {}

export function sceneDisplayName(
  scene: HierarchyProjection['scenes'][number],
  projection: HierarchyProjection
): string {
  if (scene.titlePinned || !scene.name.startsWith('Shell · ')) return scene.name
  const graph = projection.sessionGraphs?.[scene.id]
  const focusedId = graph?.focusedSessionId ?? projection.navigation.sessionByScene[scene.id]
  const node = graph?.nodes.find(({ sessionId }) => sessionId === focusedId)
  if (!node?.cwd) return scene.name
  const label = node.currentMode === 'claude-code' ? 'Claude' : node.currentMode === 'codex' ? 'Codex' : 'Shell'
  return `${label} · ${node.cwd}`
}
