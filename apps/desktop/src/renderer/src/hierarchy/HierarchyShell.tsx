import {
  useCallback, useEffect, useMemo, useRef, useState,
  type Dispatch, type SetStateAction
} from 'react'

import type { ForkStage, LayoutNode, SessionEnvironment } from '@matou/domain'
import type { RuntimeMessage, RuntimeMode } from '@matou/contracts'

import {
  RuntimeProjectionStore, type RuntimeProjectionSnapshot, type SceneSnapshotProjection
} from '../projection/RuntimeProjectionStore'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { createBrowserNotificationStore } from '../notifications/browser-notification-store'
import type { AgentNotificationStore } from '../notifications/AgentNotificationStore'
import {
  NotificationProvider, useNotificationSnapshot, useNotificationStore
} from '../notifications/NotificationProvider'
import { ingestAgentNotification } from '../notifications/agent-event-ingestion'
import { TerminalHud } from '../hud/TerminalHud'
import {
  createHierarchyCommands, createReadOnlyHierarchyCommands
} from './hierarchy-commands'
import { DetachedPlaceholder } from './DetachedPlaceholder'
import type {
  HierarchyCommands, HierarchyProjection, SceneSnapshotView,
  SessionGraphNodeView, SessionGraphView
} from './hierarchy-types'
import { SceneTabBar } from './SceneTabBar'
import { SplitTree } from './SplitTree'
import { TaskSidebar } from './TaskSidebar'
import { TerminalPane } from './TerminalPane'
import { ShortcutPanel } from './ShortcutPanel'
import { TerminalSearchBar, type TerminalSearchOptions } from './TerminalSearchBar'
import { BranchDialog, type BranchDialogSubmit } from '../session-canvas/BranchDialog'
import { SessionBreadcrumb } from '../session-canvas/SessionBreadcrumb'
import { SessionCanvas } from '../session-canvas/SessionCanvas'
import { indexSessionGraph } from '../session-canvas/session-graph-index'
import { SessionLoaderDialog } from '../session-canvas/SessionLoaderDialog'
import { useDagShortcut } from '../dag/useDagShortcut'
import '../session-canvas/session-canvas.css'
import { useTerminalShortcuts } from './useTerminalShortcuts'
import {
  DEFAULT_TERMINAL_THEME, type TerminalThemeKey
} from '../terminal/terminal-themes'
import { foregroundTerminalModels } from '../terminal/terminal-model-cache'
import { ReadOnlyRecoveryBanner, READ_ONLY_REASON } from '../recovery/ReadOnlyRecoveryBanner'
import { AppFocusRestorer } from './focus-restoration'
import { useSessionRecovery } from '../runtime/useSessionRecovery'
import { indexSceneLayout, layoutFromSnapshot } from './scene-layout-index'

export function HierarchyShell({ fixture, runtimeMode = 'normal' }: {
  fixture?: HierarchyProjection
  runtimeMode?: RuntimeMode
}) {
  const client = useRuntimeClient()
  const windowId = fixture?.windowId ?? queryValue('windowId') ?? 'window-1'
  const [projection, setProjection] = useState<HierarchyProjection | null>(
    () => fixture ? structuredClone(fixture) : null
  )
  const [loadError, setLoadError] = useState('')
  const [effectiveMode, setEffectiveMode] = useState<RuntimeMode>(runtimeMode)
  const storeRef = useRef(new RuntimeProjectionStore())
  const notificationStoreRef = useRef(createBrowserNotificationStore())
  const readOnly = effectiveMode === 'read-only'

  useEffect(() => setEffectiveMode(runtimeMode), [runtimeMode])

  const refresh = useCallback(async () => {
    if (!client) return
    const snapshot = await client.request<RuntimeProjectionSnapshot>('projection.snapshot', { windowId })
    storeRef.current.replace(snapshot)
    client.startProjection(snapshot.eventSequence)
    setProjection(toHierarchyProjection(storeRef.current.view().hierarchy))
  }, [client, windowId])

  const applyCommandResult = useCallback(async (
    result: unknown,
    context: { type: string; input: Record<string, unknown> }
  ) => {
    storeRef.current.applyCommandResult(result, context)
    const sceneId = mutationSceneId(result, context)
    if (client && sceneId && requiresFreshSceneSnapshot(context.type)) {
      const sceneSnapshot = await client.request<SceneSnapshotProjection>(
        'hierarchy.get-scene-snapshot', { sceneId }
      )
      storeRef.current.applySceneSnapshot(sceneSnapshot)
    }
    setProjection(toHierarchyProjection(storeRef.current.view().hierarchy))
  }, [client])

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
      } catch (error) {
        // Runtime generation changes and event gaps are the only normal reason
        // to rebuild the projection. Ordered events stay on the incremental path.
        void refresh().catch((refreshError: unknown) => {
          if (alive) setLoadError(errorMessage(refreshError ?? error))
        })
      }
    }
    const unsubscribe = client.subscribeProjection(onProjection)
    const now = Date.now()
    if (readOnly) {
      let retryTimer: number | undefined
      const loadReadOnlyProjection = () => {
        void refresh().then(() => {
          if (alive) setLoadError('')
        }).catch((error: unknown) => {
          if (!alive) return
          const message = errorMessage(error)
          setLoadError(message)
          if (message === 'Runtime channel replaced before the request completed') {
            retryTimer = window.setTimeout(loadReadOnlyProjection, 50)
          }
        })
      }
      loadReadOnlyProjection()
      return () => {
        alive = false
        if (retryTimer !== undefined) window.clearTimeout(retryTimer)
        unsubscribe()
      }
    }
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
    }).then(refresh).catch((error: unknown) => {
      if (!alive) return
      if (!isStorageReadOnlyError(error)) {
        setLoadError(errorMessage(error))
        return
      }
      setEffectiveMode('read-only')
      void refresh().catch((refreshError: unknown) => {
        if (alive) setLoadError(errorMessage(refreshError))
      })
    })
    return () => { alive = false; unsubscribe() }
  }, [client, fixture, readOnly, refresh, windowId])

  const fixtureCommands = useMemo(
    () => fixture ? createFixtureCommands(setProjection) : null,
    [fixture]
  )
  const commands = useMemo(() => {
    const base = fixtureCommands ?? (client ? createHierarchyCommands(client, windowId, applyCommandResult) : null)
    if (!base || !readOnly) return base
    return createReadOnlyHierarchyCommands(base, (update) => setProjection((current) => {
      if (!current) return current
      const next = structuredClone(current)
      update(next)
      return next
    }))
  }, [applyCommandResult, client, fixtureCommands, readOnly, windowId])

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
    if (!client || fixture || readOnly || !projection?.navigation.activeWorkspaceId) return
    const workspaceId = projection.navigation.activeWorkspaceId
    let checking = false
    const checkPath = async () => {
      if (checking) return
      checking = true
      const now = Date.now()
      try {
        const result = await client.request('hierarchy.validate-workspace-path', {
          command: {
            commandId: `hierarchy.validate-workspace-path-${workspaceId}-${now}`,
            commandType: 'hierarchy.validate-workspace-path', requestHash: `${workspaceId}:${now}`
          },
          input: { workspaceId, windowId, now }
        })
        applyCommandResult(result, {
          type: 'hierarchy.validate-workspace-path', input: { workspaceId, windowId, now }
        })
      } finally {
        checking = false
      }
    }
    void checkPath().catch(() => {})
    const timer = window.setInterval(() => { void checkPath().catch(() => {}) }, 400)
    return () => window.clearInterval(timer)
  }, [applyCommandResult, client, fixture, projection?.navigation.activeWorkspaceId, readOnly, windowId])

  if (!projection || !commands) {
    return <main className="hierarchy-loading" aria-busy="true" data-load-error={loadError || undefined} />
  }
  return <NotificationProvider store={notificationStoreRef.current}>
    <HierarchyProduct projection={projection} commands={commands} readOnly={readOnly} />
  </NotificationProvider>
}

function HierarchyProduct({ projection, commands, readOnly }: {
  projection: HierarchyProjection
  commands: HierarchyCommands
  readOnly: boolean
}) {
  const client = useRuntimeClient()
  const notificationStore = useNotificationStore()
  useNotificationSnapshot()
  const readOnlyRef = useRef(readOnly)
  readOnlyRef.current = readOnly
  const projectionRef = useRef(projection)
  useEffect(() => { projectionRef.current = projection }, [projection])
  const detachedWindowIds = useMemo(() => Array.from(new Set(
    (projection.sceneSnapshots ?? []).flatMap(({ mounts, windows }) => {
      const detached = new Set(windows.filter(({ state }) => state === 'detached').map(({ id }) => id))
      return mounts.flatMap(({ sceneWindowId }) =>
        sceneWindowId && detached.has(sceneWindowId) ? [sceneWindowId] : [])
    })
  )).sort(), [projection.sceneSnapshots])
  const detachedWindowSignature = detachedWindowIds.join(':')
  const [liveDetachedWindowIds, setLiveDetachedWindowIds] = useState<Set<string> | null>(null)
  const closedDetachedWindowIds = useRef(new Set<string>())
  useEffect(() => {
    if (!readOnly) {
      closedDetachedWindowIds.current.clear()
      setLiveDetachedWindowIds(null)
      return
    }
    let alive = true
    setLiveDetachedWindowIds(null)
    const exists = window.matouDesktop?.detachedTerminalWindowExists
    void Promise.all(detachedWindowIds.map(async (windowId) => {
      if (!exists) return [windowId, false] as const
      try {
        return [windowId, await exists(windowId)] as const
      } catch {
        return [windowId, false] as const
      }
    })).then((results) => {
      if (alive) setLiveDetachedWindowIds(new Set(
        results.filter(([windowId, present]) =>
          present && !closedDetachedWindowIds.current.has(windowId)
        ).map(([windowId]) => windowId)
      ))
    })
    return () => { alive = false }
  // The stable signature avoids rechecking BrowserWindows for unrelated projection refreshes.
  }, [detachedWindowSignature, readOnly])
  const [liveRatios, setLiveRatios] = useState<Record<string, number>>({})
  const [themeKey, setThemeKey] = useState<TerminalThemeKey>(DEFAULT_TERMINAL_THEME)
  const [fontSize, setFontSize] = useState(11)
  const [shortcutPanelOpen, setShortcutPanelOpen] = useState(false)
  const [searchOpen, setSearchOpen] = useState(false)
  const [sessionLoader, setSessionLoader] = useState<{
    sessionId: string
    sceneId: string
    title: string
    running: boolean
  } | null>(null)
  const [searchRequest, setSearchRequest] = useState({
    query: '', options: { caseSensitive: false, regex: false, wholeWord: false } as TerminalSearchOptions,
    direction: 'next' as 'next' | 'previous', sequence: 0
  })
  const [searchResults, setSearchResults] = useState({ resultIndex: 0, resultCount: 0 })
  const [closeRequest, setCloseRequest] = useState({ sessionId: '', sequence: 0 })
  const [dagOpenError, setDagOpenError] = useState(false)
  const [terminalFocusRequest, setTerminalFocusRequest] = useState(0)
  const [environmentRestartBySession, setEnvironmentRestartBySession] = useState<Record<string, number>>({})
  const [environmentTransitionBySession, setEnvironmentTransitionBySession] = useState<
    Record<string, 'recovering' | 'handoff' | undefined>
  >({})
  const workspaceStageRef = useRef<HTMLElement>(null)
  const loaderSessionId = sessionLoader?.sessionId ?? ''
  const loaderSceneId = sessionLoader?.sceneId ?? ''
  const listLoaderSessions = useCallback((query: string, providerSessionId?: string) => {
    if (!loaderSessionId) return Promise.resolve({ sessions: [], total: 0 })
    return commands.listClaudeSessions(loaderSessionId, query, providerSessionId)
  }, [commands, loaderSessionId])
  const loadLoaderDetail = useCallback((providerSessionId: string, query: string) => {
    if (!loaderSessionId) return Promise.reject(new Error('会话管理器已关闭'))
    return commands.getClaudeSessionDetail(loaderSessionId, providerSessionId, query)
  }, [commands, loaderSessionId])
  const cancelSessionLoader = useCallback(() => {
    setSessionLoader(null)
    setTerminalFocusRequest((value) => value + 1)
  }, [])
  const loadIntoCurrentCard = useCallback(async (providerSessionId: string) => {
    if (!loaderSessionId || !loaderSceneId) return
    await commands.loadClaudeSession(loaderSessionId, providerSessionId)
    await Promise.resolve(commands.setFocusedSession(loaderSceneId, loaderSessionId))
    setSessionLoader(null)
    setTerminalFocusRequest((value) => value + 1)
  }, [commands, loaderSceneId, loaderSessionId])
  useEffect(() => {
    const restorer = new AppFocusRestorer()
    const rememberFocus = (event: FocusEvent) => restorer.remember(event.target)
    const restoreFocus = () => restorer.scheduleRestore(() => {
      setTerminalFocusRequest((value) => value + 1)
    })
    const restoreVisibleFocus = () => {
      if (document.visibilityState === 'visible') restoreFocus()
    }
    restorer.remember(document.activeElement)
    document.addEventListener('focusin', rememberFocus, true)
    window.addEventListener('focus', restoreFocus)
    document.addEventListener('visibilitychange', restoreVisibleFocus)
    return () => {
      document.removeEventListener('focusin', rememberFocus, true)
      window.removeEventListener('focus', restoreFocus)
      document.removeEventListener('visibilitychange', restoreVisibleFocus)
      restorer.dispose()
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
    stopped?: boolean
  }>>({})
  const ratioTimers = useRef(new Map<string, number>())
  useEffect(() => () => {
    for (const timer of ratioTimers.current.values()) window.clearTimeout(timer)
    ratioTimers.current.clear()
  }, [])
  useEffect(() => {
    if (!readOnly) return
    for (const timer of ratioTimers.current.values()) window.clearTimeout(timer)
    ratioTimers.current.clear()
    setSessionLoader(null)
    setBranchDialog(null)
  }, [readOnly])
  useEffect(() => {
    const unsubscribe = window.matouDesktop?.onDetachedWindowClosed((event) => {
      if (event.mainWindowId === projection.windowId) {
        if (readOnlyRef.current) {
          closedDetachedWindowIds.current.add(event.windowId)
          setLiveDetachedWindowIds((current) => {
            const next = new Set(current ?? [])
            next.delete(event.windowId)
            return next
          })
          return
        }
        // Closing the independent window only closes that presentation. The
        // same Session returns to its canvas instead of becoming a dead card.
        void Promise.resolve(commands.returnSession(event.windowId)).catch(() => {})
      }
    })
    return () => { if (typeof unsubscribe === 'function') unsubscribe() }
  }, [commands, projection.windowId])
  const sessionById = useMemo(
    () => new Map(projection.sessions.map((session) => [session.id, session])),
    [projection.sessions]
  )
  const sessionHudById = useMemo(
    () => new Map((projection.sessionHuds ?? []).map((hud) => [hud.sessionId, hud])),
    [projection.sessionHuds]
  )
  const snapshotByScene = useMemo(
    () => new Map((projection.sceneSnapshots ?? []).map((snapshot) => [snapshot.scene.id, snapshot])),
    [projection.sceneSnapshots]
  )
  const layoutIndexByScene = useMemo(() => new Map(
    (projection.sceneSnapshots ?? []).map((snapshot) => [snapshot.scene.id, indexSceneLayout(snapshot)])
  ), [projection.sceneSnapshots])
  const graphIndexByScene = useMemo(() => new Map(
    Object.entries(projection.sessionGraphs ?? {}).map(([sceneId, graph]) =>
      [sceneId, indexSessionGraph(graph.nodes)] as const)
  ), [projection.sessionGraphs])
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
  const activeHud = focusedSessionId ? sessionHudById.get(focusedSessionId) : undefined
  const activeSnapshot = activeSceneId ? snapshotByScene.get(activeSceneId) : undefined
  const activeGraph = activeSceneId ? projection.sessionGraphs?.[activeSceneId] : undefined
  const activeGraphIndex = activeSceneId ? graphIndexByScene.get(activeSceneId) : undefined
  const activeLayoutIndex = activeSceneId ? layoutIndexByScene.get(activeSceneId) : undefined
  const dagFocusSessionId = dagFocusTarget(activeGraph, focusedSessionId)
  const dagNotificationSessionIds = notifiedSessionIds(projection, notificationStore)
  const dagNotificationSignature = dagNotificationSessionIds.join(':')
  const activeGraphFocused = (focusedSessionId ? activeGraphIndex?.byId.get(focusedSessionId) : undefined) ??
    (activeGraph?.focusedSessionId ? activeGraphIndex?.byId.get(activeGraph.focusedSessionId) : undefined)
  const selectedLevelParent = activeSceneId ? levelParentByScene[activeSceneId] : undefined
  const activeLevelParentId = selectedLevelParent !== undefined
    ? selectedLevelParent ?? undefined
    : activeGraphFocused?.parentSessionId
  const activeLevelParent = activeLevelParentId
    ? activeGraphIndex?.byId.get(activeLevelParentId)
    : undefined
  const activeLevelNodes = activeGraphIndex?.childrenOf(activeLevelParentId) ?? []
  const activeLevelSessionCount = activeLevelNodes.length
  const foregroundTerminalSessionIds = activeLevelNodes.flatMap((node) =>
    node.archivedAt === undefined && node.currentMode !== 'agent-team-member' ? [node.sessionId] : [])
  const foregroundTerminalSignature = foregroundTerminalSessionIds.join(':')
  const paneSessionIds = activeGraph && activeGraphFocused
    ? (activeGraphIndex?.childrenOf(activeGraphFocused.parentSessionId) ?? []).flatMap(({ archivedAt, sessionId }) =>
        archivedAt === undefined ? [sessionId] : [])
    : activeLayoutIndex?.orderedSessionIds ?? []
  const sessionRecovery = useSessionRecovery(
    client, activeSceneId, focusedSessionId, paneSessionIds
  )
  const activeRatios = activeSnapshot ? layoutRatios(activeSnapshot, liveRatios) : {}
  useEffect(() => {
    client?.setForegroundTerminalSessions?.(foregroundTerminalSessionIds)
    foregroundTerminalModels.setForegroundSessions(foregroundTerminalSessionIds)
    // Session IDs are stable and globally unique. The signature prevents a
    // projection refresh from churning Runtime bindings when the level is
    // unchanged while still reacting to add/remove and navigation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, foregroundTerminalSignature])
  useEffect(() => () => {
    client?.setForegroundTerminalSessions?.([])
    foregroundTerminalModels.setForegroundSessions([])
  }, [client])
  const run = (action: unknown) => { void Promise.resolve(action).catch(() => {}) }
  const applyEnvironmentResult = (result: Awaited<ReturnType<HierarchyCommands['restoreSessionEnvironment']>>) => {
    if (result.kind === 'environment' && result.restartRequired) {
      setEnvironmentRestartBySession((current) => ({
        ...current,
        [result.sessionId]: (current[result.sessionId] ?? 0) + 1
      }))
    }
    return result
  }
  const withEnvironmentTransition = async <T,>(
    sessionId: string,
    state: 'recovering' | 'handoff',
    action: () => Promise<T>
  ): Promise<T> => {
    setEnvironmentTransitionBySession((current) => ({ ...current, [sessionId]: state }))
    try {
      return await action()
    } finally {
      setEnvironmentTransitionBySession((current) => {
        const next = { ...current }
        delete next[sessionId]
        return next
      })
    }
  }
  const environmentActions = {
    open: (sessionId: string) => commands.openSessionEnvironment(sessionId),
    restore: async (sessionId: string) => withEnvironmentTransition(
      sessionId, 'recovering', async () => applyEnvironmentResult(
        await commands.restoreSessionEnvironment(sessionId)
      )
    ),
    locate: async (sessionId: string, path: string) => withEnvironmentTransition(
      sessionId, 'recovering', async () => applyEnvironmentResult(
        await commands.locateSessionEnvironment(sessionId, path)
      )
    ),
    handoff: async (sessionId: string, target: 'local' | 'worktree') => withEnvironmentTransition(
      sessionId, 'handoff', async () => applyEnvironmentResult(
        await commands.handoffSessionEnvironment(sessionId, target)
      )
    )
  }
  const activeEnvironment = optimisticEnvironment(
    activeGraphFocused?.environment,
    focusedSessionId ? environmentTransitionBySession[focusedSessionId] : undefined
  )
  const activeSessionMutationBlocked = readOnly || (
    activeEnvironment !== undefined && activeEnvironment.state !== 'ready'
  )
  const locateEnvironment = async (sessionId: string) => {
    const path = await window.matouDesktop?.selectSessionEnvironmentDirectory()
    if (!path) return
    return environmentActions.locate(sessionId, path)
  }
  const returnToLevelParent = (
    sceneId: string,
    parentSessionId: string,
    graph: SessionGraphView
  ) => {
    const parentNode = indexSessionGraph(graph.nodes).byId.get(parentSessionId)
    setLevelParentByScene((current) => ({
      ...current,
      [sceneId]: parentNode?.parentSessionId ?? null
    }))
    run(Promise.resolve(commands.setFocusedSession(sceneId, parentSessionId)).then(() => {
      setTerminalFocusRequest((value) => value + 1)
    }))
  }
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
      if (!activeSessionMutationBlocked && pathValid && activeSceneId && focusedSessionId) run(commands.createShellSibling(activeSceneId, focusedSessionId))
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
      if (!activeSessionMutationBlocked && focusedSessionId) {
        setCloseRequest((value) => ({ sessionId: focusedSessionId, sequence: value.sequence + 1 }))
      }
    },
    newTab: () => { if (!readOnly && pathValid && task) run(commands.createCanvas(task.id)) },
    nextTab: () => focusScene((scenes.findIndex(({ id }) => id === activeSceneId) + 1) % Math.max(1, scenes.length)),
    prevTab: () => {
      const index = scenes.findIndex(({ id }) => id === activeSceneId)
      focusScene((index - 1 + scenes.length) % Math.max(1, scenes.length))
    },
    jumpToTab: focusScene,
    moveTabPosition: (direction: 'left' | 'right') => {
      if (readOnly) return
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
  }), [activeRatios, activeSceneId, activeSessionMutationBlocked, activeSnapshot, commands, focusedSessionId, paneSessionIds.join(':'), pathValid, readOnly, scenes, shortcutPanelOpen, task])
  const isMac = useTerminalShortcuts(shortcutHandlers)
  const openDag = () => {
    if (!activeSceneId || !dagFocusSessionId) return
    const request = window.matouDesktop?.openDagWindow?.({
      mainWindowId: projection.windowId,
      sceneId: activeSceneId,
      sessionId: dagFocusSessionId,
      theme: themeKey,
      notificationSessionIds: dagNotificationSessionIds
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
    const target = graph ? indexSessionGraph(graph.nodes).byId.get(selection.sessionId) : undefined
    if (!target) return
    notificationStore.dismissSessionIndicator(selection.sessionId)
    setLevelParentByScene((current) => ({
      ...current,
      // An undefined value means "infer the level from the focused running node".
      // A stopped root selection instead needs an explicit root projection.
      [selection.sceneId]: target.parentSessionId ?? null
    }))
    setRevealSessionByScene((current) => ({
      ...current,
      [selection.sceneId]: {
        sessionId: selection.sessionId,
        sequence: (current[selection.sceneId]?.sequence ?? 0) + 1,
        ...(target.archivedAt === undefined ? {} : { stopped: true })
      }
    }))
    const activate = Promise.resolve(commands.activateScene(selection.sceneId))
    run(target.archivedAt === undefined
      ? activate.then(() => commands.setFocusedSession(selection.sceneId, selection.sessionId))
        .then(() => setTerminalFocusRequest((value) => value + 1))
      : activate)
  }), [commands, notificationStore])
  useEffect(() => {
    void window.matouDesktop?.updateDagNotifications?.(
      projection.windowId,
      dagNotificationSessionIds
    )
  }, [dagNotificationSignature, projection.windowId])
  useEffect(() => {
    document.body.classList.toggle('light-theme', themeKey === 'light')
    document.documentElement.dataset.theme = themeKey
    return () => {
      document.body.classList.remove('light-theme')
      delete document.documentElement.dataset.theme
    }
  }, [themeKey])

  return <main className="hierarchy-shell cli-module" data-theme={themeKey}
    data-runtime-mode={readOnly ? 'read-only' : 'normal'}>
              {readOnly && <ReadOnlyRecoveryBanner onSearch={() => setSearchOpen(true)} exportBundle={() =>
                window.matouDesktop.exportDatabaseRecoveryBundle()} />}
              <div className="claude-code-view hierarchy-body">
                <TaskSidebar projection={projection} commands={commands} pathValid={pathValid}
                  readOnly={readOnly}
                  onRevealSession={(sceneId, sessionId) => {
                    const node = graphIndexByScene.get(sceneId)?.byId.get(sessionId)
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
                        ...(node?.archivedAt === undefined ? {} : { stopped: true })
                      }
                    }))
                  }} />
                <section ref={workspaceStageRef} className="workspace-stage claude-code-main" aria-label={workspace ? `${workspace.name} 工作现场` : '工作现场'}>
        {dagOpenError && <div className="dag-open-error" role="alert">
          <span>会话关系视图打开失败，当前会话列表和返回入口仍可继续使用。</span>
          <button type="button" onClick={openDag}>重试打开 DAG</button>
          <button type="button" aria-label="关闭 DAG 异常提示" onClick={() => setDagOpenError(false)}>×</button>
        </div>}
        {task && <>
          <SceneTabBar projection={projection} commands={commands} pathValid={pathValid}
            readOnly={readOnly}
            onOpenDag={openDag} />
          <div className="scene-stack terminals-area">
            {scenes.map((scene) => {
              const snapshot = snapshotByScene.get(scene.id)
              const layoutIndex = layoutIndexByScene.get(scene.id)
              const layout = layoutIndex?.layout
              const ratios = snapshot ? layoutRatios(snapshot, liveRatios) : {}
              const graph = projection.sessionGraphs?.[scene.id]
              const graphIndex = graphIndexByScene.get(scene.id)
              const activeSessionId = graph?.focusedSessionId ?? projection.navigation.sessionByScene[scene.id]
              const renderSession = (sessionId: string, cardVisible: boolean) => {
                const mount = layoutIndex?.mountBySession.get(sessionId)
                const session = sessionById.get(sessionId)
                if (!session || !mount) return <div className="scene-recovery" aria-hidden="true" />
                const mountedWindow = mount.sceneWindowId
                  ? layoutIndex?.windowById.get(mount.sceneWindowId)
                  : undefined
                const detachedWindow = mountedWindow?.state === 'detached' ? mountedWindow : undefined
                if (detachedWindow) {
                  if (!readOnly || liveDetachedWindowIds?.has(detachedWindow.id)) {
                    return <DetachedPlaceholder title={session.title} windowId={detachedWindow.id} />
                  }
                  if (liveDetachedWindowIds === null) {
                    return <div className="scene-recovery" role="status">正在确认历史窗口…</div>
                  }
                }
                const graphNode = graphIndex?.byId.get(session.id)
                const sessionEnvironment = optimisticEnvironment(
                  graphNode?.environment,
                  environmentTransitionBySession[session.id]
                )
                const parentGraphNode = graphNode?.parentSessionId
                  ? graphIndex?.byId.get(graphNode.parentSessionId)
                  : undefined
                const childNodes = graphIndex?.childrenOf(session.id) ?? []
                const descendantSummary = graphIndex?.descendantSummaryOf(session.id)
                const sessionHud = sessionHudById.get(session.id)
                const recoveryStatus = sessionRecovery.statusBySession.get(session.id)
                const isFocused = activeSessionId === session.id
                return <TerminalPane session={session}
                  active={isFocused} visible={scene.id === activeSceneId && cardVisible}
                  foreground={scene.id === activeSceneId}
                  workspaceSessionCount={workspaceSessionCount}
                  taskName={task.title} sceneId={scene.id} pathValid={pathValid} readOnly={readOnly}
                  themeKey={themeKey} fontSize={fontSize} onFontSizeChange={setFontSize}
                  closeRequest={session.id === closeRequest.sessionId ? closeRequest.sequence : 0}
                  {...(searchOpen && scene.id === activeSceneId && isFocused ? { searchRequest } : {})}
                  {...(scene.id === activeSceneId && isFocused ? { onSearchResults: setSearchResults } : {})}
                  focusRequest={scene.id === activeSceneId && isFocused ? terminalFocusRequest : 0}
                  resumable={sessionHud?.resumable === true}
                  {...(recoveryStatus ? {
                    recoveryState: recoveryStatus.state,
                    ...(recoveryStatus.error ? { recoveryError: recoveryStatus.error } : {}),
                    onRetryRecovery: sessionRecovery.retry
                  } : {})}
                  {...(graphNode ? {
                    forkReady: graphNode.canFork,
                    workStatus: graphNode.workStatus,
                    latestLines: graphNode.latestLines,
                    providerRestoreState: graphNode.providerRestoreState,
                    forkState: graphNode.forkProgress
                      ? forkStateFromStage(graphNode.forkProgress.stage)
                      : graphNode.forkState,
                    ...(graphNode.forkProgress ? { forkProgress: graphNode.forkProgress } : {}),
                    spawnRevision: (graphNode.forkProgress?.attempt ?? graphNode.forkAttempt ?? 0) +
                      (graphNode.providerSpawnRevision ?? 0) +
                      (environmentRestartBySession[session.id] ?? 0),
                    ...(graphNode.forkProgress?.error ?? graphNode.forkError
                      ? { forkError: (graphNode.forkProgress?.error ?? graphNode.forkError)! }
                      : {}),
                    ...(graphNode.providerRestoreError ? { restoreError: graphNode.providerRestoreError } : {})
                  } : {})}
                  {...(sessionHud?.cwd ?? graphNode?.cwd
                    ? { cwd: (sessionHud?.cwd ?? graphNode?.cwd)! }
                    : {})}
                  {...(sessionHud?.gitBranch || graphNode?.git ? {
                    git: graphNode?.git ?? {
                      state: 'ready' as const,
                      branch: sessionHud!.gitBranch!, dirty: sessionHud?.gitDirty === true
                    }
                  } : {})}
                  sharedWorkingDirectory={graphNode?.sharedWorkingDirectory === true || graphNode?.worktree?.shared === true}
                  {...(sessionEnvironment ? { environment: sessionEnvironment } : {})}
                  hasOwnedWorktree={graphNode?.hasOwnedWorktree === true}
                  {...(workspace ? { workspaceId: workspace.id } : {})}
                  onActivate={(id) => commands.setFocusedSession(scene.id, id)}
                  onLoadSession={() => setSessionLoader({
                    sessionId: session.id,
                    sceneId: scene.id,
                    title: session.title,
                    running: graphNode?.workStatus === 'running' ||
                      graphNode?.workStatus === 'starting'
                  })}
                  onDelete={commands.deleteSession}
                  descendantCount={descendantSummary?.count ?? 0}
                  descendantImpact={{
                    running: descendantSummary?.running ?? 0,
                    needsInput: descendantSummary?.needsInput ?? 0
                  }}
                  {...(commands.removeSessionBranch ? {
                    onRemoveBranch: (sessionId: string, includeDescendants: boolean) =>
                      commands.removeSessionBranch?.(scene.id, sessionId, includeDescendants)
                  } : {})}
                  onRetryRestore={commands.retryProviderRestore}
                  {...(client ? { onRetryWork: (sessionId: string) => {
                    client.retryLastTerminalInput(sessionId)
                  } } : {})}
                  onRetryFork={() => commands.retryFork(scene.id, session.id)}
                  onRemoveFailedFork={() => commands.removeFailedFork(scene.id, session.id)}
                  onRestoreEnvironment={(sessionId) => environmentActions.restore(sessionId)}
                  onLocateEnvironment={(sessionId) => locateEnvironment(sessionId)}
                  onHandoffEnvironment={(sessionId, target) => environmentActions.handoff(sessionId, target)}
                  childNodes={childNodes}
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
                    const parentHud = sessionHudById.get(parentGraphNode.sessionId)
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
                  ? <SessionCanvas graph={graph} disabled={!pathValid || readOnly}
                      {...(readOnly ? { disabledReason: READ_ONLY_REASON } : {})}
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
                      {...(commands.removeSessionBranch ? {
                        onRemoveBranch: (sessionId: string, includeDescendants: boolean) =>
                          commands.removeSessionBranch?.(scene.id, sessionId, includeDescendants)
                      } : {})}
                      onNavigateToChildren={(sessionId) => {
                        setLevelParentByScene((current) => ({ ...current, [scene.id]: sessionId }))
                        let firstChild: SessionGraphNodeView | undefined
                        for (const child of graphIndex?.childrenOf(sessionId) ?? []) {
                          if (child.archivedAt === undefined) {
                            firstChild = child
                            break
                          }
                        }
                        if (firstChild) run(commands.setFocusedSession(scene.id, firstChild.sessionId))
                      }}
                      onReturnParent={(parentSessionId) => {
                        returnToLevelParent(scene.id, parentSessionId, graph)
                      }}
                      onEnsureSessionVisible={(sessionId) => {
                        if (sessionId === activeSessionId) setTerminalFocusRequest((value) => value + 1)
                      }} />
                  : layout && snapshot
                  ? <SplitTree root={layout} ratios={ratios} onRatio={(nodeId, ratio) => {
                      const key = `${scene.id}:${nodeId}`
                      setLiveRatios((current) => ({ ...current, [key]: ratio }))
                      if (readOnlyRef.current) return
                      const pending = ratioTimers.current.get(key)
                      if (pending !== undefined) window.clearTimeout(pending)
                      ratioTimers.current.set(key, window.setTimeout(() => {
                        ratioTimers.current.delete(key)
                        if (readOnlyRef.current) return
                        void Promise.resolve(commands.putGeometry(
                          scene.id, `node:${nodeId}`, scene.layoutRevision ?? 0, { ratio }
                        )).catch(() => {})
                      }, 100))
                    }} renderMount={(mountId) => {
                      const mount = layoutIndex?.mountById.get(mountId)
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
                    {activeGraph && <SessionBreadcrumb
                      {...(activeLevelParent ? { parentTitle: activeLevelParent.title } : {})}
                      sessionCount={activeLevelSessionCount}
                      {...(activeSceneId && activeLevelParentId ? {
                        onReturnParent: () => returnToLevelParent(
                          activeSceneId, activeLevelParentId, activeGraph
                        )
                      } : {})} />}
                    <TerminalHud hud={activeHud}
                      {...(focusedSessionId ? { sessionId: focusedSessionId } : {})}
                      {...(activeEnvironment
                        ? { environment: activeEnvironment }
                        : {})}
                      hasOwnedWorktree={activeGraphFocused?.hasOwnedWorktree === true}
                      {...(activeGraphFocused?.git ? { git: activeGraphFocused.git } : {})}
                      environmentActions={environmentActions}
                      onPermissionMode={commands.setPermissionMode}
                      onModel={commands.setModel}
                      {...(activeSessionMutationBlocked ? {
                        disabledReason: readOnly
                          ? READ_ONLY_REASON
                          : '当前运行环境需要先恢复或交接'
                      } : {})}
                      {...(activeSceneId ? {
                        gitContext: { windowId: projection.windowId, sceneId: activeSceneId }
                      } : {})} />
                  </div>
                </section>
              </div>
              <ShortcutPanel open={shortcutPanelOpen} isMac={isMac} themeKey={themeKey}
                onClose={() => {
                  setShortcutPanelOpen(false)
                  setTerminalFocusRequest((value) => value + 1)
                }} />
              {sessionLoader && !readOnly && <SessionLoaderDialog
                targetTitle={sessionLoader.title}
                targetRunning={sessionLoader.running}
                {...(workspaceStageRef.current ? { portalTarget: workspaceStageRef.current } : {})}
                listSessions={listLoaderSessions}
                loadDetail={loadLoaderDetail}
                onCancel={cancelSessionLoader}
                onLoad={loadIntoCurrentCard} />}
              {branchDialog && !readOnly && <BranchDialog relationMode={branchDialog.relationMode}
                sourceTitle={branchDialog.sourceTitle} gitAvailable={branchDialog.gitAvailable}
                onCancel={() => {
                  setBranchDialog(null)
                  setTerminalFocusRequest((value) => value + 1)
                }}
                onConfirm={async (input: BranchDialogSubmit) => {
                  if (readOnlyRef.current) return
                  const { sceneId, sourceSessionId, relationMode } = branchDialog
                  if (relationMode === 'child') {
                    const result = await Promise.resolve(commands.createForkChild(
                      sceneId, sourceSessionId, input.name, input.worktreeMode,
                      input.submissionKey
                    ))
                    const createdSessionId = mutationSessionId(result)
                    if (createdSessionId) {
                      await Promise.resolve(commands.setFocusedSession(sceneId, createdSessionId))
                    }
                    // A child Fork creates the next level in the same canvas. Keep
                    // the newly focused child visible instead of leaving the user
                    // on the source session's sibling level.
                    setLevelParentByScene((current) => ({
                      ...current,
                      [sceneId]: sourceSessionId
                    }))
                  } else {
                    await Promise.resolve(commands.createForkSibling(
                      sceneId, sourceSessionId, input.name, input.worktreeMode,
                      input.submissionKey
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
  const pending: Array<{ node: LayoutNode; rect: Rect }> = [
    { node: layout, rect: { x: 0, y: 0, width: 1, height: 1 } }
  ]
  while (pending.length > 0) {
    const { node, rect } = pending.pop()!
    if (node.kind === 'mount') {
      byMount.set(node.mountId, rect)
      continue
    }
    const childRects: Array<{ node: LayoutNode; rect: Rect }> = []
    const count = node.children.length
    let cursor = node.direction === 'horizontal' ? rect.x : rect.y
    for (let index = 0; index < node.children.length; index += 1) {
      const child = node.children[index]!
      const share = count === 2 && ratios[node.id] !== undefined
        ? (index === 0 ? ratios[node.id]! : 1 - ratios[node.id]!)
        : 1 / node.children.length
      const childRect = node.direction === 'horizontal'
        ? { x: cursor, y: rect.y, width: rect.width * share, height: rect.height }
        : { x: rect.x, y: cursor, width: rect.width, height: rect.height * share }
      childRects.push({ node: child, rect: childRect })
      cursor += node.direction === 'horizontal' ? childRect.width : childRect.height
    }
    for (let index = childRects.length - 1; index >= 0; index -= 1) {
      pending.push(childRects[index]!)
    }
  }
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
      activeChildCount: 0, stoppedChildCount: 0,
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
    openSessionEnvironment: async (sessionId) => ({
      sessionId, kind: 'local' as const, path: '/tmp'
    }),
    restoreSessionEnvironment: async (sessionId) => ({
      kind: 'environment' as const, sessionId, activeTarget: 'local' as const,
      state: 'ready' as const, path: '/tmp', restartRequired: false
    }),
    locateSessionEnvironment: async (sessionId) => ({
      kind: 'environment' as const, sessionId, activeTarget: 'local' as const,
      state: 'ready' as const, path: '/tmp', restartRequired: false
    }),
    handoffSessionEnvironment: async (sessionId, target) => ({
      kind: 'environment' as const, sessionId, activeTarget: target,
      state: 'ready' as const, path: '/tmp', restartRequired: true
    }),
    createWorkspace: NOOP, renameWorkspace: NOOP, relinkWorkspace: NOOP, removeWorkspace: NOOP,
    setWorkspacePinned: NOOP, reorderPinnedWorkspace: NOOP,
    createTask: NOOP, renameTask: NOOP, reorderTask: NOOP, deleteTask: NOOP,
    setTaskPinned: NOOP, reorderPinnedTask: NOOP,
    createCanvas: createFixtureCanvas,
    createShellSibling: (sceneId, sourceSessionId) => createFixtureSibling(sceneId, sourceSessionId),
    createForkChild: createFixtureForkChild, createForkSibling: NOOP,
    retryFork: NOOP, removeFailedFork: NOOP,
    retryProviderRestore: NOOP, restartStoppedSession: NOOP, removeSessionBranch: NOOP,
    listClaudeSessions: async () => ({ sessions: [], total: 0 }),
    getClaudeSessionDetail: async () => { throw new Error('fixture session is unavailable') },
    loadClaudeSession: async (sessionId, providerSessionId) => ({
      sessionId, providerSessionId, permissionMode: 'default' as const
    }),
    getSceneSessionGraph: NOOP,
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
    }), detachSession: NOOP, returnSession: (sceneWindowId) => updateNavigation((value) => {
      const snapshot = value.sceneSnapshots?.find(({ windows }) =>
        windows.some(({ id }) => id === sceneWindowId)
      )
      if (!snapshot) return
      const mount = snapshot.mounts.find(({ sceneWindowId: owner }) => owner === sceneWindowId)
      if (mount) delete mount.sceneWindowId
      snapshot.windows = snapshot.windows.map((candidate) => candidate.id === sceneWindowId
        ? { ...candidate, state: 'closed' as const }
        : candidate)
    }),
    setPermissionMode: NOOP, setModel: NOOP
  }
}

function toHierarchyProjection(value: unknown): HierarchyProjection {
  return value as HierarchyProjection
}

function forkStateFromStage(stage: ForkStage): 'pending' | 'starting' | 'succeeded' | 'failed' {
  if (stage === 'queued') return 'pending'
  if (stage === 'succeeded' || stage === 'failed') return stage
  return 'starting'
}

function queryValue(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name)
}

function optimisticEnvironment(
  environment: SessionEnvironment | undefined,
  state: 'recovering' | 'handoff' | undefined
): SessionEnvironment | undefined {
  if (!environment || !state) return environment
  return { ...environment, state }
}
function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

function isStorageReadOnlyError(error: unknown): boolean {
  return typeof error === 'object' && error !== null && 'code' in error &&
    error.code === 'STORAGE_READ_ONLY'
}

function mutationSessionId(value: unknown): string | undefined {
  if (!value || typeof value !== 'object' || !('session' in value)) return undefined
  const session = value.session
  if (!session || typeof session !== 'object' || !('id' in session)) return undefined
  return typeof session.id === 'string' ? session.id : undefined
}

function mutationSceneId(
  value: unknown,
  context: { type: string; input: Record<string, unknown> }
): string | undefined {
  if (typeof context.input.sceneId === 'string') return context.input.sceneId
  if (!value || typeof value !== 'object' || !('scene' in value)) return undefined
  const scene = value.scene
  return scene && typeof scene === 'object' && 'id' in scene && typeof scene.id === 'string'
    ? scene.id
    : undefined
}

function requiresFreshSceneSnapshot(type: string): boolean {
  return [
    'hierarchy.create-scene', 'hierarchy.create-canvas', 'hierarchy.reopen-scene',
    'hierarchy.split-session', 'hierarchy.fork-session', 'hierarchy.create-shell-sibling',
    'hierarchy.create-fork-child', 'hierarchy.create-fork-sibling',
    'hierarchy.retry-fork', 'hierarchy.remove-failed-fork',
    'hierarchy.remove-session-branch', 'hierarchy.delete-session',
    'hierarchy.detach-session', 'hierarchy.return-session', 'hierarchy.replace-layout'
  ].includes(type)
}

function focusedSession(projection: HierarchyProjection): string | undefined {
  const workspaceId = projection.navigation.activeWorkspaceId
  const taskId = workspaceId ? projection.navigation.taskByWorkspace[workspaceId] : undefined
  const sceneId = taskId ? projection.navigation.sceneByTask[taskId] : undefined
  return sceneId ? projection.navigation.sessionByScene[sceneId] : undefined
}

function notifiedSessionIds(
  projection: HierarchyProjection,
  store: AgentNotificationStore
): string[] {
  const ids = new Set(Object.values(projection.sessionGraphs ?? {})
    .flatMap(({ nodes }) => nodes.map(({ sessionId }) => sessionId)))
  return [...ids].filter((sessionId) => store.sessionHasVisibleIndicator(sessionId))
}

function dagFocusTarget(
  graph: SessionGraphView | undefined,
  liveFocusedSessionId: string | undefined
): string | undefined {
  if (!graph || graph.nodes.length === 0) return liveFocusedSessionId
  const index = indexSessionGraph(graph.nodes)
  if (liveFocusedSessionId && index.byId.has(liveFocusedSessionId)) return liveFocusedSessionId
  if (graph.focusedSessionId && index.byId.has(graph.focusedSessionId)) return graph.focusedSessionId
  return index.roots[0]?.sessionId ?? graph.nodes[0]?.sessionId
}
