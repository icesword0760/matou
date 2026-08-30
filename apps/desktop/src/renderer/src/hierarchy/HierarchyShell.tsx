import {
  useCallback, useEffect, useMemo, useRef, useState,
  type Dispatch, type SetStateAction
} from 'react'

import type { LayoutNode } from '@matou/domain'
import type { RuntimeMessage } from '@matou/contracts'

import { RuntimeProjectionStore, type RuntimeProjectionSnapshot } from '../projection/RuntimeProjectionStore'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { createBrowserNotificationStore } from '../notifications/browser-notification-store'
import { NotificationProvider } from '../notifications/NotificationProvider'
import { ingestAgentNotification } from '../notifications/agent-event-ingestion'
import { TerminalHud } from '../hud/TerminalHud'
import { createHierarchyCommands } from './hierarchy-commands'
import { DetachedPlaceholder } from './DetachedPlaceholder'
import type {
  HierarchyCommands, HierarchyProjection, SceneNodeView, SceneSnapshotView,
  SessionGraphNodeView, SessionGraphView
} from './hierarchy-types'
import { SceneTabBar } from './SceneTabBar'
import { SplitTree } from './SplitTree'
import { TaskSidebar } from './TaskSidebar'
import { TerminalPane } from './TerminalPane'
import { ShortcutPanel } from './ShortcutPanel'
import { TerminalSearchBar, type TerminalSearchOptions } from './TerminalSearchBar'
import { BranchDialog, type BranchDialogSubmit } from '../session-canvas/BranchDialog'
import { SessionCanvas } from '../session-canvas/SessionCanvas'
import { useDagShortcut } from '../dag/useDagShortcut'
import '../session-canvas/session-canvas.css'
import { useTerminalShortcuts } from './useTerminalShortcuts'
import {
  DEFAULT_TERMINAL_THEME, type TerminalThemeKey
} from '../terminal/terminal-themes'
export function HierarchyShell({ fixture }: { fixture?: HierarchyProjection }) {
  const client = useRuntimeClient()
  const windowId = fixture?.windowId ?? queryValue('windowId') ?? 'window-1'
  const [projection, setProjection] = useState<HierarchyProjection | null>(
    () => fixture ? structuredClone(fixture) : null
  )
  const [loadError, setLoadError] = useState('')
  const storeRef = useRef(new RuntimeProjectionStore())
  const notificationStoreRef = useRef(createBrowserNotificationStore())

  const refresh = useCallback(async () => {
    if (!client) return
    const snapshot = await client.request<RuntimeProjectionSnapshot>('projection.snapshot', { windowId })
    storeRef.current.replace(snapshot)
    client.startProjection(snapshot.eventSequence)
    setProjection(toHierarchyProjection(storeRef.current.view().hierarchy))
  }, [client, windowId])

  useEffect(() => {
    if (fixture || !client) return
    let alive = true
    const onProjection = (message: RuntimeMessage) => {
      if (!alive) return
      if (message.type === 'terminal.hud') {
        setProjection((current) => {
          if (!current) return current
          const next = structuredClone(current)
          next.sessionHuds = (next.sessionHuds ?? []).filter(({ sessionId }) => sessionId !== message.sessionId)
          if (message.hud) next.sessionHuds.push(message.hud)
          return next
        })
        return
      }
      if (message.type !== 'events.batch') return
      try {
        storeRef.current.applyBatch(message.runtimeGeneration, message.events)
        const after = toHierarchyProjection(storeRef.current.view().hierarchy)
        const focusedSessionId = focusedSession(after)
        for (const event of message.events) {
          ingestAgentNotification(event, after, focusedSessionId, notificationStoreRef.current)
        }
        setProjection(after)
      } catch {
        // A fresh snapshot below also repairs Runtime reconnects and event gaps.
      }
      void refresh().catch((error: unknown) => alive && setLoadError(errorMessage(error)))
    }
    const unsubscribe = client.subscribeProjection(onProjection)
    const now = Date.now()
    void client.request('hierarchy.bootstrap-window', {
      command: {
        commandId: `hierarchy.bootstrap-window-${windowId}-${now}`,
        commandType: 'hierarchy.bootstrap-window', requestHash: `${windowId}:${now}`
      },
      input: {
        windowId,
        defaultRootDirectory: queryValue('defaultRootDirectory') ?? '/tmp',
        defaultName: queryValue('defaultName') ?? 'home', now
      }
    }).then(refresh).catch((error: unknown) => alive && setLoadError(errorMessage(error)))
    return () => { alive = false; unsubscribe() }
  }, [client, fixture, refresh, windowId])

  const fixtureCommands = useMemo(
    () => fixture ? createFixtureCommands(setProjection) : null,
    [fixture]
  )
  const commands = useMemo(
    () => fixtureCommands ?? (client ? createHierarchyCommands(client, windowId, refresh) : null),
    [client, fixtureCommands, refresh, windowId]
  )

  useEffect(() => {
    if (queryValue('e2e') !== '1') return
    window.matouE2e = {
      pushNotification: (input) => { notificationStoreRef.current.push(input) },
      moveTaskToWindow: async (input) => {
        if (!client) throw new Error('Runtime client is unavailable')
        const now = Date.now()
        const envelope = (phase: 'prepare' | 'acknowledge', commandId: string) => ({
          command: { commandId, commandType: 'hierarchy.move-task-to-window', requestHash: commandId },
          input: { ...input, phase, windowId, now: Date.now() }
        })
        await client.request('hierarchy.move-task-to-window', envelope('prepare', `${input.migrationId}-prepare-${now}`))
        await client.request('hierarchy.move-task-to-window', envelope('acknowledge', `${input.migrationId}-ack-${now}`))
        await refresh()
      }
    }
    return () => { delete window.matouE2e }
  }, [client, refresh, windowId])

  useEffect(() => {
    if (!client || fixture || !projection?.navigation.activeWorkspaceId) return
    const workspaceId = projection.navigation.activeWorkspaceId
    let checking = false
    const checkPath = async () => {
      if (checking) return
      checking = true
      const now = Date.now()
      try {
        await client.request('hierarchy.validate-workspace-path', {
          command: {
            commandId: `hierarchy.validate-workspace-path-${workspaceId}-${now}`,
            commandType: 'hierarchy.validate-workspace-path', requestHash: `${workspaceId}:${now}`
          },
          input: { workspaceId, windowId, now }
        })
        await refresh()
      } finally {
        checking = false
      }
    }
    void checkPath().catch(() => {})
    const timer = window.setInterval(() => { void checkPath().catch(() => {}) }, 400)
    return () => window.clearInterval(timer)
  }, [client, fixture, projection?.navigation.activeWorkspaceId, refresh, windowId])

  if (!projection || !commands) {
    return <main className="hierarchy-loading" aria-busy="true" data-load-error={loadError || undefined} />
  }
  return <NotificationProvider store={notificationStoreRef.current}>
    <HierarchyProduct projection={projection} commands={commands} />
  </NotificationProvider>
}

function HierarchyProduct({ projection, commands }: {
  projection: HierarchyProjection
  commands: HierarchyCommands
}) {
  const client = useRuntimeClient()
  const projectionRef = useRef(projection)
  useEffect(() => { projectionRef.current = projection }, [projection])
  const [liveRatios, setLiveRatios] = useState<Record<string, number>>({})
  const [themeKey, setThemeKey] = useState<TerminalThemeKey>(DEFAULT_TERMINAL_THEME)
  const [fontSize, setFontSize] = useState(11)
  const [shortcutPanelOpen, setShortcutPanelOpen] = useState(false)
  const [searchOpen, setSearchOpen] = useState(false)
  const [searchRequest, setSearchRequest] = useState({
    query: '', options: { caseSensitive: false, regex: false, wholeWord: false } as TerminalSearchOptions,
    direction: 'next' as 'next' | 'previous', sequence: 0
  })
  const [searchResults, setSearchResults] = useState({ resultIndex: 0, resultCount: 0 })
  const [closeRequest, setCloseRequest] = useState({ sessionId: '', sequence: 0 })
  const [dagOpenError, setDagOpenError] = useState(false)
  const [terminalFocusRequest, setTerminalFocusRequest] = useState(0)
  useEffect(() => {
    const restoreTerminalFocus = () => setTerminalFocusRequest((value) => value + 1)
    const restoreVisibleTerminalFocus = () => {
      if (document.visibilityState === 'visible') restoreTerminalFocus()
    }
    window.addEventListener('focus', restoreTerminalFocus)
    document.addEventListener('visibilitychange', restoreVisibleTerminalFocus)
    return () => {
      window.removeEventListener('focus', restoreTerminalFocus)
      document.removeEventListener('visibilitychange', restoreVisibleTerminalFocus)
    }
  }, [])
  const [branchDialog, setBranchDialog] = useState<{
    sceneId: string
    sourceSessionId: string
    sourceTitle: string
    relationMode: 'child' | 'sibling'
    gitAvailable: boolean
  } | null>(null)
  const [levelParentByScene, setLevelParentByScene] = useState<Record<string, string | null | undefined>>({})
  const [revealSessionByScene, setRevealSessionByScene] = useState<Record<string, {
    sessionId: string
    sequence: number
    historical?: boolean
  }>>({})
  const ratioTimers = useRef(new Map<string, number>())
  useEffect(() => () => {
    for (const timer of ratioTimers.current.values()) window.clearTimeout(timer)
    ratioTimers.current.clear()
  }, [])
  useEffect(() => {
    const unsubscribe = window.matouDesktop?.onDetachedWindowClosed((event) => {
      if (event.mainWindowId === projection.windowId) {
        // Closing the independent window is an explicit end of that terminal.
        // Keep the stable graph node as history so DAG selection exposes the
        // continue/reopen path instead of silently remounting a live process.
        void Promise.resolve(commands.deleteSession(event.sessionId, true, true)).catch(() => {})
      }
    })
    return () => { if (typeof unsubscribe === 'function') unsubscribe() }
  }, [commands, projection.windowId])
  const workspaceId = projection.navigation.activeWorkspaceId
  const workspace = projection.workspaces.find(({ id }) => id === workspaceId)
  const placedTaskIds = new Set(projection.taskPlacements
    .filter(({ windowId }) => windowId === projection.windowId)
    .map(({ taskId }) => taskId))
  const focusedTaskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const taskId = focusedTaskId && (projection.taskPlacements.length === 0 || placedTaskIds.has(focusedTaskId))
    ? focusedTaskId
    : projection.tasks.find(({ id, workspaceId: owner }) =>
        owner === workspaceId && (projection.taskPlacements.length === 0 || placedTaskIds.has(id))
      )?.id
  const task = projection.tasks.find(({ id }) => id === taskId)
  const activeSceneId = taskId ? projection.navigation.sceneByTask[taskId] : undefined
  const scenes = projection.scenes.filter(({ taskId: owner }) => owner === taskId)
  const workspaceTaskIds = new Set(
    projection.tasks.filter(({ id, workspaceId: owner }) =>
      owner === workspaceId && (projection.taskPlacements.length === 0 || placedTaskIds.has(id))
    ).map(({ id }) => id)
  )
  const workspaceSessionCount = projection.sessions.filter(({ taskId: owner }) => workspaceTaskIds.has(owner)).length
  const pathValid = projection.pathStates.find(({ workspaceId: owner }) => owner === workspaceId)?.status !== 'invalid'
  const focusedSessionId = focusedSession(projection)
  const activeHud = projection.sessionHuds?.find(({ sessionId }) => sessionId === focusedSessionId)
  const activeSnapshot = projection.sceneSnapshots?.find(({ scene }) => scene.id === activeSceneId)
  const activeGraph = activeSceneId ? projection.sessionGraphs?.[activeSceneId] : undefined
  const dagFocusSessionId = dagFocusTarget(activeGraph, focusedSessionId)
  const activeGraphFocused = activeGraph?.nodes.find(({ sessionId }) => sessionId === focusedSessionId) ??
    activeGraph?.nodes.find(({ sessionId }) => sessionId === activeGraph.focusedSessionId)
  const paneSessionIds = activeGraph && activeGraphFocused
    ? activeGraph.nodes.filter(({ archivedAt, parentSessionId }) =>
        archivedAt === undefined && parentSessionId === activeGraphFocused.parentSessionId
      ).map(({ sessionId }) => sessionId)
    : activeSnapshot ? orderedSessionIds(activeSnapshot) : []
  const activeRatios = activeSnapshot ? layoutRatios(activeSnapshot, liveRatios) : {}
  const run = (action: unknown) => { void Promise.resolve(action).catch(() => {}) }
  const focusPane = (offset: number) => {
    if (paneSessionIds.length <= 1) return
    const current = Math.max(0, paneSessionIds.indexOf(focusedSessionId ?? ''))
    const next = (current + offset + paneSessionIds.length) % paneSessionIds.length
    run(commands.activateSession(paneSessionIds[next]!))
  }
  const focusScene = (index: number) => {
    if (index >= 0 && index < scenes.length) run(commands.activateScene(scenes[index]!.id))
  }
  const updateSearch = (query: string, options: TerminalSearchOptions, direction: 'next' | 'previous' = 'next') => {
    setSearchRequest((current) => ({ query, options, direction, sequence: current.sequence + 1 }))
  }
  const shortcutHandlers = useMemo(() => ({
    splitHorizontal: () => {
      if (pathValid && activeSceneId && focusedSessionId) run(commands.createShellSibling(activeSceneId, focusedSessionId))
    },
    splitVertical: () => {},
    nextPane: () => focusPane(1),
    prevPane: () => focusPane(-1),
    switchPaneByDirection: (direction: 'up' | 'down' | 'left' | 'right') => {
      const target = activeSnapshot && focusedSessionId
        ? directionalSessionId(activeSnapshot, focusedSessionId, direction, activeRatios)
        : undefined
      if (target) run(commands.activateSession(target))
    },
    closePane: () => {
      if (focusedSessionId) {
        setCloseRequest((value) => ({ sessionId: focusedSessionId, sequence: value.sequence + 1 }))
      }
    },
    newTab: () => { if (pathValid && task) run(commands.createCanvas(task.id)) },
    nextTab: () => focusScene((scenes.findIndex(({ id }) => id === activeSceneId) + 1) % Math.max(1, scenes.length)),
    prevTab: () => {
      const index = scenes.findIndex(({ id }) => id === activeSceneId)
      focusScene((index - 1 + scenes.length) % Math.max(1, scenes.length))
    },
    jumpToTab: focusScene,
    moveTabPosition: (direction: 'left' | 'right') => {
      const index = scenes.findIndex(({ id }) => id === activeSceneId)
      if (!activeSceneId || index < 0) return
      if (direction === 'left' && index > 0) run(commands.reorderScene(activeSceneId, scenes[index - 1]!.id))
      if (direction === 'right' && index < scenes.length - 1) run(commands.reorderScene(activeSceneId, scenes[index + 2]?.id))
    },
    openSearch: () => setSearchOpen(true),
    increaseFontSize: () => setFontSize((value) => Math.min(24, value + 1)),
    decreaseFontSize: () => setFontSize((value) => Math.max(10, value - 1)),
    resetFontSize: () => setFontSize(11),
    cycleTheme: () => {
      setThemeKey((value) => value === 'light' ? 'dark' : 'light')
      setTerminalFocusRequest((value) => value + 1)
    },
    toggleShortcutPanel: () => {
      if (shortcutPanelOpen) setTerminalFocusRequest((value) => value + 1)
      setShortcutPanelOpen(!shortcutPanelOpen)
    }
  }), [activeRatios, activeSceneId, activeSnapshot, commands, focusedSessionId, paneSessionIds.join(':'), pathValid, scenes, shortcutPanelOpen, task])
  const isMac = useTerminalShortcuts(shortcutHandlers)
  const openDag = () => {
    if (!activeSceneId || !dagFocusSessionId) return
    const request = window.matouDesktop?.openDagWindow?.({
      mainWindowId: projection.windowId,
      sceneId: activeSceneId,
      sessionId: dagFocusSessionId,
      theme: themeKey
    })
    if (request === undefined) {
      setDagOpenError(true)
      return
    }
    setDagOpenError(false)
    void Promise.resolve(request).catch(() => setDagOpenError(true))
  }
  useDagShortcut({
    enabled: Boolean(activeSceneId && dagFocusSessionId),
    onShortPress: () => window.dispatchEvent(new Event('matou:forward-terminal-tab')),
    onLongPress: openDag
  })
  useEffect(() => window.matouDesktop?.onDagShortcut?.((kind) => {
    if (kind === 'long') openDag()
    else window.dispatchEvent(new Event('matou:forward-terminal-tab'))
  }), [activeSceneId, dagFocusSessionId, projection.windowId, themeKey])
  useEffect(() => window.matouDesktop?.onDagNodeSelected?.((selection) => {
    const currentProjection = projectionRef.current
    if (selection.mainWindowId !== currentProjection.windowId) return
    const graph = currentProjection.sessionGraphs?.[selection.sceneId]
    const target = graph?.nodes.find(({ sessionId }) => sessionId === selection.sessionId)
    if (!target) return
    setLevelParentByScene((current) => ({
      ...current,
      // An undefined value means "infer the level from the focused live node".
      // A root historical selection instead needs an explicit root projection.
      [selection.sceneId]: target.parentSessionId ?? null
    }))
    setRevealSessionByScene((current) => ({
      ...current,
      [selection.sceneId]: {
        sessionId: selection.sessionId,
        sequence: (current[selection.sceneId]?.sequence ?? 0) + 1,
        ...(target.archivedAt === undefined ? {} : { historical: true })
      }
    }))
    const activate = Promise.resolve(commands.activateScene(selection.sceneId))
    run(target.archivedAt === undefined
      ? activate.then(() => commands.setFocusedSession(selection.sceneId, selection.sessionId))
        .then(() => setTerminalFocusRequest((value) => value + 1))
      : activate)
  }), [commands])
  useEffect(() => {
    document.body.classList.toggle('light-theme', themeKey === 'light')
    document.documentElement.dataset.theme = themeKey
    return () => {
      document.body.classList.remove('light-theme')
      delete document.documentElement.dataset.theme
    }
  }, [themeKey])

  return <main className="hierarchy-shell cli-module" data-theme={themeKey}>
              <div className="claude-code-view hierarchy-body">
                <TaskSidebar projection={projection} commands={commands} pathValid={pathValid}
                  onRevealSession={(sceneId, sessionId) => {
                    const node = projection.sessionGraphs?.[sceneId]?.nodes.find((candidate) =>
                      candidate.sessionId === sessionId)
                    setLevelParentByScene((current) => ({
                      ...current,
                      [sceneId]: node ? node.parentSessionId ?? null : undefined
                    }))
                    setTerminalFocusRequest((value) => value + 1)
                    setRevealSessionByScene((current) => ({
                      ...current,
                      [sceneId]: {
                        sessionId,
                        sequence: (current[sceneId]?.sequence ?? 0) + 1,
                        ...(node?.archivedAt === undefined ? {} : { historical: true })
                      }
                    }))
                  }} />
                <section className="workspace-stage claude-code-main" aria-label={workspace ? `${workspace.name} 工作现场` : '工作现场'}>
        {dagOpenError && <div className="dag-open-error" role="alert">
          <span>会话关系视图打开失败，当前会话列表和返回入口仍可继续使用。</span>
          <button type="button" onClick={openDag}>重试打开 DAG</button>
          <button type="button" aria-label="关闭 DAG 异常提示" onClick={() => setDagOpenError(false)}>×</button>
        </div>}
        {task && <>
          <SceneTabBar projection={projection} commands={commands} pathValid={pathValid}
            onOpenDag={openDag} />
          <div className="scene-stack terminals-area">
            {scenes.map((scene) => {
              const snapshot = projection.sceneSnapshots?.find(({ scene: owner }) => owner.id === scene.id)
              const layout = snapshot ? layoutFromSnapshot(snapshot) : undefined
              const ratios = snapshot ? layoutRatios(snapshot, liveRatios) : {}
              const graph = projection.sessionGraphs?.[scene.id]
              const activeSessionId = graph?.focusedSessionId ?? projection.navigation.sessionByScene[scene.id]
              const renderSession = (sessionId: string, cardVisible: boolean) => {
                const mount = snapshot?.mounts.find((candidate) => candidate.sessionId === sessionId)
                const session = projection.sessions.find(({ id }) => id === sessionId)
                if (!session || !mount) return <div className="scene-recovery" aria-hidden="true" />
                const detachedWindow = snapshot?.windows.find(({ id, state }) =>
                  id === mount.sceneWindowId && state === 'detached'
                )
                if (detachedWindow) {
                  return <DetachedPlaceholder title={session.title} windowId={detachedWindow.id} />
                }
                const graphNode = graph?.nodes.find(({ sessionId: candidate }) => candidate === session.id)
                const parentGraphNode = graphNode?.parentSessionId
                  ? graph?.nodes.find(({ sessionId: candidate }) => candidate === graphNode.parentSessionId)
                  : undefined
                const childNodes = graph?.nodes.filter(({ parentSessionId }) => parentSessionId === session.id) ?? []
                const sessionHud = projection.sessionHuds?.find(({ sessionId: candidate }) => candidate === session.id)
                const isFocused = activeSessionId === session.id
                return <TerminalPane session={session}
                  active={isFocused} visible={scene.id === activeSceneId && cardVisible}
                  workspaceSessionCount={workspaceSessionCount}
                  taskName={task.title} sceneId={scene.id} pathValid={pathValid}
                  themeKey={themeKey} fontSize={fontSize} onFontSizeChange={setFontSize}
                  closeRequest={session.id === closeRequest.sessionId ? closeRequest.sequence : 0}
                  {...(searchOpen && scene.id === activeSceneId && isFocused ? { searchRequest } : {})}
                  {...(scene.id === activeSceneId && isFocused ? { onSearchResults: setSearchResults } : {})}
                  focusRequest={scene.id === activeSceneId && isFocused ? terminalFocusRequest : 0}
                  resumable={sessionHud?.resumable === true}
                  {...(graphNode ? {
                    forkReady: graphNode.canFork,
                    workStatus: graphNode.workStatus,
                    latestLines: graphNode.latestLines,
                    providerRestoreState: graphNode.providerRestoreState,
                    forkState: graphNode.forkState,
                    spawnRevision: graphNode.forkAttempt ?? 0,
                    ...(graphNode.forkError ? { forkError: graphNode.forkError } : {}),
                    ...(graphNode.providerRestoreError ? { restoreError: graphNode.providerRestoreError } : {})
                  } : {})}
                  {...(sessionHud?.cwd ?? graphNode?.cwd
                    ? { cwd: (sessionHud?.cwd ?? graphNode?.cwd)! }
                    : {})}
                  {...(sessionHud?.gitBranch || graphNode?.git ? {
                    git: graphNode?.git ?? {
                      branch: sessionHud!.gitBranch!, dirty: sessionHud?.gitDirty === true
                    }
                  } : {})}
                  sharedWorkingDirectory={graphNode?.sharedWorkingDirectory === true || graphNode?.worktree?.shared === true}
                  {...(workspace ? { workspaceId: workspace.id } : {})}
                  onActivate={(id) => commands.setFocusedSession(scene.id, id)}
                  onDelete={commands.deleteSession}
                  onRetryRestore={commands.retryProviderRestore}
                  {...(client ? { onRetryWork: (sessionId: string) => {
                    client.retryLastTerminalInput(sessionId)
                  } } : {})}
                  onRetryFork={() => commands.retryFork(scene.id, session.id)}
                  onRemoveFailedFork={() => commands.removeFailedFork(scene.id, session.id)}
                  childNodes={childNodes}
                  historicalChildCount={graphNode?.historicalChildCount ?? 0}
                  onOpenChildren={() => {
                    setLevelParentByScene((current) => ({ ...current, [scene.id]: session.id }))
                    const preferredChild = preferredActiveChild(childNodes)
                    if (preferredChild) run(commands.setFocusedSession(scene.id, preferredChild.sessionId))
                  }}
                  onFork={() => setBranchDialog({
                    sceneId: scene.id, sourceSessionId: session.id, sourceTitle: session.title,
                    relationMode: 'child', gitAvailable: Boolean(sessionHud?.gitBranch)
                  })}
                  {...(parentGraphNode?.canFork === true ? { onForkSibling: () => {
                    const parentHud = projection.sessionHuds?.find(
                      ({ sessionId: candidate }) => candidate === parentGraphNode.sessionId
                    )
                    setBranchDialog({
                      sceneId: scene.id, sourceSessionId: session.id, sourceTitle: parentGraphNode.title,
                      relationMode: 'sibling', gitAvailable: Boolean(parentHud?.gitBranch)
                    })
                  } } : {})}
                  {...(window.matouDesktop?.createDetachedTerminalWindow
                    ? { onDetach: async () => {
                        const sceneWindowId = crypto.randomUUID()
                        await commands.detachSession(scene.id, mount.id, session.id, sceneWindowId)
                        try {
                          await window.matouDesktop.createDetachedTerminalWindow({
                            windowId: sceneWindowId, mainWindowId: projection.windowId,
                            sceneId: scene.id, mountId: mount.id, sessionId: session.id,
                            executionContextId: session.executionContextId ?? 'local-default',
                            profile: session.kind === 'claude-code' || session.kind === 'codex' || session.kind === 'agent-team-member'
                              ? session.kind : 'shell', title: session.title
                          })
                        } catch (error) {
                          await commands.returnSession(sceneWindowId)
                          throw error
                        }
                      } }
                    : {})} />
              }
              return <section className="scene-stage" key={scene.id} hidden={scene.id !== activeSceneId}
                aria-label={`${scene.name} 终端布局`}>
                {graph && snapshot
                  ? <SessionCanvas graph={graph} disabled={!pathValid}
                      {...(levelParentByScene[scene.id] !== undefined
                        ? { levelParentSessionId: levelParentByScene[scene.id]! }
                        : {})}
                      {...(revealSessionByScene[scene.id]
                        ? { revealRequest: revealSessionByScene[scene.id] }
                        : {})}
                      renderSession={(node, cardVisible) => renderSession(node.sessionId, cardVisible)}
                      {...(snapshot.geometry
                        ? { geometry: snapshot.geometry as Array<{ ownerKey: string; geometry: Record<string, unknown> }> }
                        : {})}
                      onPutGeometry={(ownerKey, geometry) => commands.putGeometry(
                        scene.id, ownerKey, graph.layoutRevision ?? scene.layoutRevision ?? 0, geometry
                      )}
                      onActivate={(sessionId) => run(commands.setFocusedSession(scene.id, sessionId))}
                      onCreateShellSibling={(sessionId, parentSessionId) =>
                        run(commands.createShellSibling(scene.id, sessionId, parentSessionId))}
                      onCreateForkSibling={(source, parent) => {
                        const parentHud = projection.sessionHuds?.find(({ sessionId }) => sessionId === parent.sessionId)
                        setBranchDialog({
                          sceneId: scene.id, sourceSessionId: source.sessionId,
                          sourceTitle: parent.title, relationMode: 'sibling',
                          gitAvailable: Boolean(parentHud?.gitBranch)
                        })
                      }}
                      onReopenHistorical={(sessionId) => run(commands.reopenHistoricalSession(sessionId))}
                      onNavigateToChildren={(sessionId) => {
                        setLevelParentByScene((current) => ({ ...current, [scene.id]: sessionId }))
                        const firstChild = graph.nodes.find((node) =>
                          node.parentSessionId === sessionId && node.archivedAt === undefined)
                        if (firstChild) run(commands.setFocusedSession(scene.id, firstChild.sessionId))
                      }}
                      onRemoveHistorical={(sessionId, includeDescendants) => run(
                        commands.removeHistoricalSession?.(scene.id, sessionId, includeDescendants)
                      )}
                      onReturnParent={(parentSessionId) => {
                        const parentNode = graph.nodes.find(({ sessionId }) => sessionId === parentSessionId)
                        setLevelParentByScene((current) => ({
                          ...current,
                          [scene.id]: parentNode?.parentSessionId
                        }))
                        run(Promise.resolve(commands.setFocusedSession(scene.id, parentSessionId)).then(() => {
                          setTerminalFocusRequest((value) => value + 1)
                        }))
                      }}
                      onEnsureSessionVisible={(sessionId) => {
                        if (sessionId === activeSessionId) setTerminalFocusRequest((value) => value + 1)
                      }} />
                  : layout && snapshot
                  ? <SplitTree root={layout} ratios={ratios} onRatio={(nodeId, ratio) => {
                      const key = `${scene.id}:${nodeId}`
                      setLiveRatios((current) => ({ ...current, [key]: ratio }))
                      const pending = ratioTimers.current.get(key)
                      if (pending !== undefined) window.clearTimeout(pending)
                      ratioTimers.current.set(key, window.setTimeout(() => {
                        ratioTimers.current.delete(key)
                        void Promise.resolve(commands.putGeometry(
                          scene.id, `node:${nodeId}`, scene.layoutRevision ?? 0, { ratio }
                        )).catch(() => {})
                      }, 100))
                    }} renderMount={(mountId) => {
                      const mount = snapshot.mounts.find(({ id }) => id === mountId)
                      return mount ? renderSession(mount.sessionId, true) : <div className="scene-recovery" aria-hidden="true" />
                    }} />
                  : <div className="scene-recovery" aria-hidden="true" />}
              </section>
            })}
          </div>
        </>}
        {!task && <div className="scene-recovery" role="status">选择或新建一个事项开始工作</div>}
                  <TerminalSearchBar open={searchOpen} themeKey={themeKey}
                    resultIndex={searchResults.resultIndex} resultCount={searchResults.resultCount}
                    onSearch={(query, options) => updateSearch(query, options)}
                    onNext={() => updateSearch(searchRequest.query, searchRequest.options, 'next')}
                    onPrevious={() => updateSearch(searchRequest.query, searchRequest.options, 'previous')}
                    onClose={() => {
                      updateSearch('', searchRequest.options)
                      setSearchOpen(false)
                      setTerminalFocusRequest((value) => value + 1)
                    }} />
                  <div className="shortcut-bar" aria-label="快捷指令栏">
                    <button className="add-btn" aria-label="添加快捷指令">+</button>
                    <div className="btn-list" />
                    <TerminalHud hud={activeHud} onPermissionMode={commands.setPermissionMode}
                      onModel={commands.setModel} />
                  </div>
                </section>
              </div>
              <ShortcutPanel open={shortcutPanelOpen} isMac={isMac} themeKey={themeKey}
                onClose={() => {
                  setShortcutPanelOpen(false)
                  setTerminalFocusRequest((value) => value + 1)
                }} />
              {branchDialog && <BranchDialog relationMode={branchDialog.relationMode}
                sourceTitle={branchDialog.sourceTitle} gitAvailable={branchDialog.gitAvailable}
                onCancel={() => {
                  setBranchDialog(null)
                  setTerminalFocusRequest((value) => value + 1)
                }}
                onConfirm={async (input: BranchDialogSubmit) => {
                  const { sceneId, sourceSessionId, relationMode } = branchDialog
                  if (relationMode === 'child') {
                    await Promise.resolve(commands.createForkChild(
                      sceneId, sourceSessionId, input.name, input.worktreeMode
                    ))
                    // A child Fork creates the next level in the same canvas. Keep
                    // the newly focused child visible instead of leaving the user
                    // on the source session's sibling level.
                    setLevelParentByScene((current) => ({
                      ...current,
                      [sceneId]: sourceSessionId
                    }))
                  } else {
                    await Promise.resolve(commands.createForkSibling(
                      sceneId, sourceSessionId, input.name, input.worktreeMode
                    ))
                  }
                  setBranchDialog(null)
                  setTerminalFocusRequest((value) => value + 1)
                }} />}
  </main>
}

const CHILD_STATUS_PRIORITY: Record<SessionGraphNodeView['workStatus'], number> = {
  error: 0,
  'needs-input': 1,
  running: 2,
  starting: 2,
  interrupted: 3,
  idle: 4,
  exited: 5
}

/**
 * The aggregate badge represents the most urgent child state. Opening it must
 * land on the child responsible for that state instead of an unrelated first
 * child. Array order remains the tie-breaker so sibling ordering stays stable.
 */
export function preferredActiveChild(
  children: SessionGraphNodeView[]
): SessionGraphNodeView | undefined {
  return children.reduce<SessionGraphNodeView | undefined>((preferred, child) => {
    if (child.archivedAt !== undefined) return preferred
    if (!preferred) return child
    return CHILD_STATUS_PRIORITY[child.workStatus] < CHILD_STATUS_PRIORITY[preferred.workStatus]
      ? child
      : preferred
  }, undefined)
}

function layoutRatios(snapshot: SceneSnapshotView, live: Record<string, number>): Record<string, number> {
  const ratios: Record<string, number> = {}
  for (const item of snapshot.geometry ?? []) {
    if (!item.ownerKey.startsWith('node:')) continue
    const ratio = typeof item.geometry.ratio === 'number' ? item.geometry.ratio : undefined
    if (ratio !== undefined) ratios[item.ownerKey.slice('node:'.length)] = ratio
  }
  for (const node of snapshot.nodes) {
    const ratio = live[`${snapshot.scene.id}:${node.id}`]
    if (ratio !== undefined) ratios[node.id] = ratio
  }
  return ratios
}

function layoutFromSnapshot(snapshot: SceneSnapshotView): LayoutNode | undefined {
  const rootId = snapshot.scene.rootNodeId ?? snapshot.nodes.find(({ parentNodeId }) => !parentNodeId)?.id
  if (!rootId) return undefined
  const mountByNode = new Map(snapshot.mounts.flatMap((mount) =>
    mount.sceneNodeId ? [[mount.sceneNodeId, mount.id] as const] : []
  ))
  const byParent = new Map<string, SceneNodeView[]>()
  for (const node of snapshot.nodes) {
    if (!node.parentNodeId) continue
    const siblings = byParent.get(node.parentNodeId) ?? []
    siblings.push(node)
    byParent.set(node.parentNodeId, siblings)
  }
  const read = (nodeId: string): LayoutNode | undefined => {
    const mountId = mountByNode.get(nodeId)
    if (mountId) return { id: nodeId, kind: 'mount', mountId }
    const node = snapshot.nodes.find(({ id }) => id === nodeId)
    const children = (byParent.get(nodeId) ?? [])
      .sort((left, right) => left.ordinal - right.ordinal)
      .flatMap((child) => {
        const value = read(child.id)
        return value ? [value] : []
      })
    if (children.length === 1) return children[0]
    if (children.length > 1) {
      return {
        id: nodeId, kind: 'split', direction: node?.direction ?? 'horizontal', children
      }
    }
    return undefined
  }
  return read(rootId)
}

function orderedSessionIds(snapshot: SceneSnapshotView): string[] {
  const layout = layoutFromSnapshot(snapshot)
  if (!layout) return []
  const sessionByMount = new Map(snapshot.mounts.map((mount) => [mount.id, mount.sessionId]))
  const read = (node: LayoutNode): string[] => node.kind === 'mount'
    ? [sessionByMount.get(node.mountId)].filter((value): value is string => Boolean(value))
    : node.children.flatMap(read)
  return read(layout)
}

function directionalSessionId(
  snapshot: SceneSnapshotView,
  currentSessionId: string,
  direction: 'up' | 'down' | 'left' | 'right',
  ratios: Record<string, number>
): string | undefined {
  const layout = layoutFromSnapshot(snapshot)
  if (!layout) return undefined
  type Rect = { x: number; y: number; width: number; height: number }
  const byMount = new Map<string, Rect>()
  const visit = (node: LayoutNode, rect: Rect) => {
    if (node.kind === 'mount') {
      byMount.set(node.mountId, rect)
      return
    }
    const count = node.children.length
    let cursor = node.direction === 'horizontal' ? rect.x : rect.y
    node.children.forEach((child, index) => {
      const share = count === 2 && ratios[node.id] !== undefined
        ? (index === 0 ? ratios[node.id]! : 1 - ratios[node.id]!)
        : 1 / count
      const childRect = node.direction === 'horizontal'
        ? { x: cursor, y: rect.y, width: rect.width * share, height: rect.height }
        : { x: rect.x, y: cursor, width: rect.width, height: rect.height * share }
      visit(child, childRect)
      cursor += node.direction === 'horizontal' ? childRect.width : childRect.height
    })
  }
  visit(layout, { x: 0, y: 0, width: 1, height: 1 })
  const sessionByMount = new Map(snapshot.mounts.map((mount) => [mount.id, mount.sessionId]))
  const currentEntry = [...byMount.entries()].find(([mountId]) => sessionByMount.get(mountId) === currentSessionId)
  if (!currentEntry) return undefined
  const current = currentEntry[1]
  const center = (rect: Rect) => ({ x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 })
  const origin = center(current)
  const candidates = [...byMount.entries()].flatMap(([mountId, rect]) => {
    const sessionId = sessionByMount.get(mountId)
    if (!sessionId || sessionId === currentSessionId) return []
    const point = center(rect)
    const primary = direction === 'left' ? origin.x - point.x
      : direction === 'right' ? point.x - origin.x
        : direction === 'up' ? origin.y - point.y
          : point.y - origin.y
    if (primary <= 0) return []
    const perpendicular = direction === 'left' || direction === 'right'
      ? Math.abs(point.y - origin.y)
      : Math.abs(point.x - origin.x)
    return [{ sessionId, score: primary * 4 + perpendicular }]
  })
  candidates.sort((left, right) => left.score - right.score)
  return candidates[0]?.sessionId
}

function createFixtureCommands(
  setProjection: Dispatch<SetStateAction<HierarchyProjection | null>>
): HierarchyCommands {
  const updateNavigation = (mutate: (projection: HierarchyProjection) => void) => {
    setProjection((current) => {
      if (!current) return current
      const next = structuredClone(current)
      mutate(next)
      return next
    })
  }
  const NOOP = () => {}
  const createFixtureCanvas = (taskId: string) => updateNavigation((value) => {
    const ordinal = value.scenes.filter((scene) => scene.taskId === taskId).length + 1
    const sceneId = `fixture-scene-${ordinal}`
    const sessionId = `fixture-session-${ordinal}`
    const nodeId = `fixture-node-${ordinal}`
    const mountId = `fixture-mount-${ordinal}`
    value.scenes.push({ id: sceneId, taskId, name: `Shell ${ordinal}`, rootNodeId: nodeId })
    value.sessions.push({ id: sessionId, taskId, title: 'Shell', executionContextId: 'local-default' })
    value.sceneSnapshots ??= []
    value.sceneSnapshots.push({
      scene: value.scenes.at(-1)!, nodes: [{ id: nodeId, sceneId, kind: 'mount', ordinal: 0 }],
      mounts: [{ id: mountId, sceneId, sceneNodeId: nodeId, sessionId }], windows: []
    })
    value.navigation.sceneByTask[taskId] = sceneId
    value.navigation.sessionByScene[sceneId] = sessionId
  })
  const createFixtureSibling = (
    sceneId: string,
    sourceSessionId: string,
    direction: 'horizontal' | 'vertical' = 'horizontal'
  ) => updateNavigation((value) => {
    const snapshot = value.sceneSnapshots?.find(({ scene }) => scene.id === sceneId)
    const source = value.sessions.find(({ id }) => id === sourceSessionId)
    if (!snapshot || !source || !snapshot.scene.rootNodeId) return
    const suffix = snapshot.mounts.length + 1
    const sessionId = `fixture-split-session-${sceneId}-${suffix}`
    const rootId = `fixture-split-root-${sceneId}-${suffix}`
    const nodeId = `fixture-split-node-${sceneId}-${suffix}`
    const mountId = `fixture-split-mount-${sceneId}-${suffix}`
    const previousRootId = snapshot.scene.rootNodeId
    const previousRoot = snapshot.nodes.find(({ id }) => id === previousRootId)
    if (previousRoot) previousRoot.parentNodeId = rootId
    snapshot.nodes.push(
      { id: rootId, sceneId, kind: 'split', direction, ordinal: 0 },
      { id: nodeId, sceneId, parentNodeId: rootId, kind: 'mount', ordinal: 1 }
    )
    snapshot.scene.rootNodeId = rootId
    snapshot.mounts.push({ id: mountId, sceneId, sceneNodeId: nodeId, sessionId })
    value.sessions.push({ ...source, id: sessionId, title: 'Shell' })
    value.navigation.sessionByScene[sceneId] = sessionId
  })
  const createFixtureForkChild = (
    sceneId: string,
    sourceSessionId: string,
    name: string
  ) => updateNavigation((value) => {
    const snapshot = value.sceneSnapshots?.find(({ scene }) => scene.id === sceneId)
    const graph = value.sessionGraphs?.[sceneId]
    const sourceSession = value.sessions.find(({ id }) => id === sourceSessionId)
    const source = graph?.nodes.find(({ sessionId }) => sessionId === sourceSessionId)
    if (!snapshot || !graph || !sourceSession || !source) return
    const suffix = snapshot.mounts.length + 1
    const sessionId = `fixture-fork-session-${sceneId}-${suffix}`
    const nodeId = `fixture-fork-node-${sceneId}-${suffix}`
    const mountId = `fixture-fork-mount-${sceneId}-${suffix}`
    value.sessions.push({
      ...sourceSession, id: sessionId, kind: 'claude-code', title: name
    })
    snapshot.nodes.push({ id: nodeId, sceneId, kind: 'mount', ordinal: suffix })
    snapshot.mounts.push({ id: mountId, sceneId, sceneNodeId: nodeId, sessionId })
    source.activeChildCount += 1
    source.childModeCounts = {
      ...source.childModeCounts,
      claudeCode: source.childModeCounts.claudeCode + 1
    }
    graph.nodes.push({
      sessionId, sceneId, currentMode: 'claude-code', workStatus: 'starting',
      providerRestoreState: 'none', canFork: false, title: name, cwd: source.cwd,
      parentSessionId: sourceSessionId, relationKind: 'forked-from',
      activeChildCount: 0, historicalChildCount: 0,
      childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [],
      lastUserInteractionSeq: 0
    })
    graph.edges.push({
      parentSessionId: sourceSessionId, childSessionId: sessionId,
      relationKind: 'forked-from', createdAt: Date.now()
    })
    graph.focusedSessionId = sessionId
    value.navigation.sessionByScene[sceneId] = sessionId
  })
  return {
    activateWorkspace: (workspaceId) => updateNavigation((value) => { value.navigation.activeWorkspaceId = workspaceId }),
    activateTask: (taskId) => updateNavigation((value) => {
      const task = value.tasks.find(({ id }) => id === taskId)
      if (task) value.navigation.taskByWorkspace[task.workspaceId] = taskId
    }),
    activateScene: (sceneId) => updateNavigation((value) => {
      const scene = value.scenes.find(({ id }) => id === sceneId)
      if (scene) value.navigation.sceneByTask[scene.taskId] = sceneId
    }),
    activateSession: (sessionId) => updateNavigation((value) => {
      const snapshot = value.sceneSnapshots?.find(({ mounts }) => mounts.some((mount) => mount.sessionId === sessionId))
      if (snapshot) value.navigation.sessionByScene[snapshot.scene.id] = sessionId
    }),
    createWorkspace: NOOP, renameWorkspace: NOOP, relinkWorkspace: NOOP, removeWorkspace: NOOP,
    setWorkspacePinned: NOOP, reorderPinnedWorkspace: NOOP,
    createTask: NOOP, renameTask: NOOP, reorderTask: NOOP, deleteTask: NOOP,
    setTaskPinned: NOOP, reorderPinnedTask: NOOP,
    createCanvas: createFixtureCanvas,
    createShellSibling: (sceneId, sourceSessionId) => createFixtureSibling(sceneId, sourceSessionId),
    createForkChild: createFixtureForkChild, createForkSibling: NOOP,
    retryFork: NOOP, removeFailedFork: NOOP,
    retryProviderRestore: NOOP, reopenHistoricalSession: NOOP, getSceneSessionGraph: NOOP,
    recordSessionInteraction: NOOP,
    setFocusedSession: (sceneId, sessionId) => updateNavigation((value) => {
      value.navigation.sessionByScene[sceneId] = sessionId
      if (value.sessionGraphs?.[sceneId]) value.sessionGraphs[sceneId]!.focusedSessionId = sessionId
    }),
    createScene: createFixtureCanvas, renameScene: NOOP, reorderScene: (sceneId, beforeSceneId) => updateNavigation((value) => {
      const scene = value.scenes.find(({ id }) => id === sceneId)
      if (!scene) return
      const peers = value.scenes.filter(({ taskId }) => taskId === scene.taskId)
      const reordered = peers.filter(({ id }) => id !== sceneId)
      const targetIndex = beforeSceneId
        ? reordered.findIndex(({ id }) => id === beforeSceneId)
        : reordered.length
      reordered.splice(targetIndex < 0 ? reordered.length : targetIndex, 0, scene)
      let peerIndex = 0
      value.scenes = value.scenes.map((candidate) =>
        candidate.taskId === scene.taskId ? reordered[peerIndex++]! : candidate
      )
    }), closeScene: NOOP,
    splitSession: createFixtureSibling, forkSession: NOOP, putGeometry: NOOP, deleteSession: (sessionId) => updateNavigation((value) => {
      const snapshot = value.sceneSnapshots?.find(({ mounts }) => mounts.some((mount) => mount.sessionId === sessionId))
      const mount = snapshot?.mounts.find((candidate) => candidate.sessionId === sessionId)
      if (!snapshot || !mount || snapshot.mounts.length <= 1) return
      snapshot.mounts = snapshot.mounts.filter(({ sessionId: candidate }) => candidate !== sessionId)
      if (mount.sceneNodeId) snapshot.nodes = snapshot.nodes.filter(({ id }) => id !== mount.sceneNodeId)
      value.sessions = value.sessions.filter(({ id }) => id !== sessionId)
      const fallback = snapshot.mounts[0]?.sessionId
      if (fallback) value.navigation.sessionByScene[snapshot.scene.id] = fallback
    }), detachSession: NOOP, returnSession: NOOP,
    setPermissionMode: NOOP, setModel: NOOP
  }
}

function toHierarchyProjection(value: unknown): HierarchyProjection {
  return value as HierarchyProjection
}
function queryValue(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name)
}
function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function focusedSession(projection: HierarchyProjection): string | undefined {
  const workspaceId = projection.navigation.activeWorkspaceId
  const taskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const sceneId = taskId ? projection.navigation.sceneByTask[taskId] : undefined
  return sceneId ? projection.navigation.sessionByScene[sceneId] : undefined
}

function dagFocusTarget(
  graph: SessionGraphView | undefined,
  liveFocusedSessionId: string | undefined
): string | undefined {
  if (!graph || graph.nodes.length === 0) return liveFocusedSessionId
  const nodeIds = new Set(graph.nodes.map(({ sessionId }) => sessionId))
  if (liveFocusedSessionId && nodeIds.has(liveFocusedSessionId)) return liveFocusedSessionId
  if (graph.focusedSessionId && nodeIds.has(graph.focusedSessionId)) return graph.focusedSessionId
  return graph.nodes.find(({ parentSessionId }) => parentSessionId === undefined)?.sessionId ??
    graph.nodes[0]?.sessionId
}
