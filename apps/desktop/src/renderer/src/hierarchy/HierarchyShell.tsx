import {
  useCallback, useEffect, useMemo, useRef, useState,
  type Dispatch, type SetStateAction
} from 'react'

import type { LayoutNode } from '@matou/domain'
import type { RuntimeMessage } from '@matou/contracts'

import { RuntimeProjectionStore, type RuntimeProjectionSnapshot } from '../projection/RuntimeProjectionStore'
import { useRuntimeClient } from '../runtime/RuntimeProvider'
import { createHierarchyCommands } from './hierarchy-commands'
import { DetachedPlaceholder } from './DetachedPlaceholder'
import type {
  HierarchyCommands, HierarchyProjection, SceneNodeView, SceneSnapshotView
} from './hierarchy-types'
import { SceneTabBar } from './SceneTabBar'
import { SplitTree } from './SplitTree'
import { TaskSidebar } from './TaskSidebar'
import { TerminalPane } from './TerminalPane'
import { WorkspaceSwitcher } from './WorkspaceSwitcher'

export function HierarchyShell({ fixture }: { fixture?: HierarchyProjection }) {
  const client = useRuntimeClient()
  const windowId = fixture?.windowId ?? queryValue('windowId') ?? 'window-1'
  const [projection, setProjection] = useState<HierarchyProjection | null>(
    () => fixture ? structuredClone(fixture) : null
  )
  const [loadError, setLoadError] = useState('')
  const storeRef = useRef(new RuntimeProjectionStore())

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
      if (message.type !== 'events.batch' || !alive) return
      try {
        storeRef.current.applyBatch(message.runtimeGeneration, message.events)
        setProjection(toHierarchyProjection(storeRef.current.view().hierarchy))
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
        defaultRootDirectory: queryValue('defaultRootDirectory') ?? '/tmp/matou_workspace',
        defaultName: 'matou_workspace', now
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

  if (!projection || !commands) {
    return <main className="hierarchy-loading" aria-busy="true">
      <strong>正在恢复工作现场…</strong>
      {loadError && <p role="alert">{loadError}</p>}
    </main>
  }
  return <HierarchyProduct projection={projection} commands={commands} />
}

function HierarchyProduct({ projection, commands }: {
  projection: HierarchyProjection
  commands: HierarchyCommands
}) {
  useEffect(() => window.matouDesktop?.onDetachedWindowClosed((event) => {
    if (event.mainWindowId === projection.windowId) {
      void Promise.resolve(commands.returnSession(event.windowId)).catch(() => {})
    }
  }), [commands, projection.windowId])
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

  return <main className="hierarchy-shell">
    <header className="hierarchy-topbar">
      <div className="brand-mark" aria-label="Matou">M</div>
      <div data-testid="workspace-name"><WorkspaceSwitcher projection={projection} commands={commands} /></div>
      <div className="hierarchy-window-title">{task?.title ?? '终端工作区'}</div>
    </header>
    <div className="hierarchy-body">
      <TaskSidebar projection={projection} commands={commands} />
      <section className="workspace-stage" aria-label={workspace ? `${workspace.name} 工作现场` : '工作现场'}>
        {task && <>
          <div className="task-stage-heading">
            <strong data-testid="active-task">{task.title}</strong>
            <span>{scenes.length} 个页签 · {projection.sessions.filter(({ taskId: owner }) => owner === task.id).length} 个终端</span>
          </div>
          <SceneTabBar projection={projection} commands={commands} />
          <div className="scene-stack">
            {scenes.map((scene) => {
              const snapshot = projection.sceneSnapshots?.find(({ scene: owner }) => owner.id === scene.id)
              const layout = snapshot ? layoutFromSnapshot(snapshot) : undefined
              return <section className="scene-stage" key={scene.id} hidden={scene.id !== activeSceneId}
                aria-label={`${scene.name} 终端布局`}>
                {layout && snapshot
                  ? <SplitTree root={layout} renderMount={(mountId) => {
                      const mount = snapshot.mounts.find(({ id }) => id === mountId)
                      const session = projection.sessions.find(({ id }) => id === mount?.sessionId)
                      if (!session) return <div role="status">终端记录正在恢复</div>
                      const detachedWindow = snapshot.windows.find(({ id, state }) =>
                        id === mount?.sceneWindowId && state === 'detached'
                      )
                      if (detachedWindow) {
                        return <DetachedPlaceholder title={session.title} windowId={detachedWindow.id} />
                      }
                      return <TerminalPane session={session}
                        active={projection.navigation.sessionByScene[scene.id] === session.id}
                        visible={scene.id === activeSceneId}
                        workspaceSessionCount={workspaceSessionCount}
                        taskName={task.title} pathValid={pathValid}
                        onActivate={commands.activateSession} onDelete={commands.deleteSession}
                        {...(window.matouDesktop?.createDetachedTerminalWindow
                          ? { onDetach: async () => {
                              const sceneWindowId = crypto.randomUUID()
                              await commands.detachSession(scene.id, mountId, session.id, sceneWindowId)
                              try {
                                await window.matouDesktop.createDetachedTerminalWindow({
                                  windowId: sceneWindowId, mainWindowId: projection.windowId,
                                  sceneId: scene.id, mountId, sessionId: session.id,
                                  executionContextId: session.executionContextId ?? 'local-default',
                                  profile: session.kind === 'claude-code' || session.kind === 'codex'
                                    ? session.kind : 'shell',
                                  title: session.title
                                })
                              } catch (error) {
                                await commands.returnSession(sceneWindowId)
                                throw error
                              }
                            } }
                          : {})} />
                    }} />
                  : <div className="scene-recovery" role="status">正在恢复页签布局…</div>}
              </section>
            })}
          </div>
        </>}
        {!task && <div className="scene-recovery" role="status">选择或新建一个事项开始工作</div>}
      </section>
    </div>
  </main>
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
    createWorkspace: NOOP, renameWorkspace: NOOP, removeWorkspace: NOOP,
    createTask: NOOP, renameTask: NOOP, reorderTask: NOOP, deleteTask: NOOP,
    createScene: NOOP, renameScene: NOOP, reorderScene: NOOP, closeScene: NOOP,
    splitSession: NOOP, deleteSession: NOOP, detachSession: NOOP, returnSession: NOOP
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
