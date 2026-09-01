// @vitest-environment jsdom

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagWindowApp } from './DagWindowApp'

const runtime = vi.hoisted(() => ({ current: null as null | { request: ReturnType<typeof vi.fn> } }))
vi.mock('../runtime/RuntimeProvider', () => ({ useRuntimeClient: () => runtime.current }))
let runtimeConnectionListener: ((state: 'reconnecting' | 'ready') => void) | undefined
let dagNotificationListener: ((sessionIds: string[]) => void) | undefined

beforeEach(() => {
  runtime.current = null
  runtimeConnectionListener = undefined
  dagNotificationListener = undefined
  window.history.replaceState({}, '', '/?kind=dag&mainWindowId=main-1&sceneId=scene-1&sessionId=child&theme=light')
  Object.defineProperty(window, 'matouDesktop', { configurable: true, value: {
    selectDagNode: vi.fn(), closeDagWindow: vi.fn(), onDagContext: vi.fn(() => () => {}),
    onDagNotifications: vi.fn((listener) => {
      dagNotificationListener = listener
      return () => { dagNotificationListener = undefined }
    }),
    onRuntimeConnectionState: vi.fn((listener) => {
      runtimeConnectionListener = listener
      return () => { runtimeConnectionListener = undefined }
    })
  } })
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callback(0); return 1 })
  vi.stubGlobal('cancelAnimationFrame', vi.fn())
})
afterEach(() => { cleanup(); Reflect.deleteProperty(window, 'matouDesktop'); vi.unstubAllGlobals() })

describe('DagWindowApp', () => {
  it('renders the native-window DAG, selects detached nodes and closes on Escape', async () => {
    const data = graph()
    render(<DagWindowApp fixtureGraph={data} />)

    expect(screen.getByRole('application', { name: '会话 DAG 画布' })).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '打开会话：Child' }))
    expect(window.matouDesktop.selectDagNode).toHaveBeenCalledWith(expect.objectContaining({
      mainWindowId: 'main-1', sceneId: 'scene-1', sessionId: 'child', targetWindowId: 'detached-1'
    }))

    fireEvent.keyDown(window, { key: 'Escape' })
    expect(window.matouDesktop.closeDagWindow).toHaveBeenCalledWith('main-1')
  })

  it('shows Git branch, dirty state and shared-worktree impact on a node card', () => {
    const data = graph()
    data.nodes[1] = {
      ...data.nodes[1]!, git: { state: 'ready', branch: 'feature/dag', dirty: true },
      sharedWorkingDirectory: true
    }
    render(<DagWindowApp fixtureGraph={data} />)

    expect(screen.getByText('feature/dag*')).toBeTruthy()
    expect(screen.getByText('共享工作树')).toBeTruthy()
  })

  it('updates node notification breathing borders while the DAG window stays open', () => {
    render(<DagWindowApp fixtureGraph={graph()} />)

    act(() => dagNotificationListener?.(['child']))
    expect(screen.getByRole('button', { name: '打开会话：Child' })
      .classList.contains('has-notification')).toBe(true)

    act(() => dagNotificationListener?.([]))
    expect(screen.getByRole('button', { name: '打开会话：Child' })
      .classList.contains('has-notification')).toBe(false)
  })

  it('keeps the last DAG visible while clearly saying that Runtime information is temporarily stale', () => {
    render(<DagWindowApp fixtureGraph={graph()} />)

    act(() => runtimeConnectionListener?.('reconnecting'))

    expect(screen.getByRole('application', { name: '会话 DAG 画布' })).toBeTruthy()
    expect(screen.getByRole('status').textContent).toContain('会话信息暂时未更新')
    expect(screen.getByRole('status').textContent).toContain('正在重新连接')
  })

  it('persists every changed viewport before the short-lived native DAG can close', async () => {
    const data = graph()
    const request = vi.fn(async (method: string) => {
      if (method === 'geometry.list') return []
      if (method === 'hierarchy.get-scene-session-graph') return data
      return undefined
    })
    runtime.current = { request }

    render(<DagWindowApp />)
    await screen.findByRole('application', { name: '会话 DAG 画布' })
    request.mockClear()

    await userEvent.setup().click(screen.getByRole('button', { name: '放大' }))

    expect(request).toHaveBeenCalledWith('geometry.put', expect.objectContaining({
      sceneId: 'scene-1', ownerKey: 'dag-viewport:scene-1',
      geometry: expect.objectContaining({ zoom: 1.1 })
    }))
  })

  it('keeps DAG browsing active but never persists viewport geometry in read-only recovery', async () => {
    const data = graph()
    const request = vi.fn(async (method: string) => {
      if (method === 'geometry.list') return []
      if (method === 'hierarchy.get-scene-session-graph') return data
      return undefined
    })
    runtime.current = { request }

    render(<DagWindowApp runtimeMode="read-only" />)
    await screen.findByRole('application', { name: '会话 DAG 画布' })
    request.mockClear()
    await userEvent.setup().click(screen.getByRole('button', { name: '放大' }))
    await userEvent.setup().click(screen.getByRole('button', { name: '打开会话：Child' }))

    expect(screen.getByRole('status').textContent).toContain('数据库处于只读恢复模式')
    expect(request).not.toHaveBeenCalledWith('geometry.put', expect.anything())
    expect(window.matouDesktop.selectDagNode).toHaveBeenCalledWith(expect.objectContaining({
      sessionId: 'child'
    }))
  })

  it('does not flush a pending viewport change while entering read-only recovery', async () => {
    const data = graph()
    const request = vi.fn(async (method: string) => {
      if (method === 'geometry.list') return []
      if (method === 'hierarchy.get-scene-session-graph') return data
      return undefined
    })
    runtime.current = { request }

    const view = render(<DagWindowApp />)
    await screen.findByRole('application', { name: '会话 DAG 画布' })
    await userEvent.setup().click(screen.getByRole('button', { name: '放大' }))
    request.mockClear()

    view.rerender(<DagWindowApp runtimeMode="read-only" />)
    await screen.findByText('数据库处于只读恢复模式')

    expect(request).not.toHaveBeenCalledWith('geometry.put', expect.anything())
  })
})

function graph(): SessionGraphView {
  return {
    sceneId: 'scene-1', focusedSessionId: 'child',
    nodes: [node('root', 'Root'), { ...node('child', 'Child'), parentSessionId: 'root', detachedWindowId: 'detached-1' }],
    edges: [{ parentSessionId: 'root', childSessionId: 'child', relationKind: 'derived-from', createdAt: 1 }]
  }
}

function node(sessionId: string, title: string) {
  return {
    sessionId, sceneId: 'scene-1', currentMode: 'shell' as const, workStatus: 'idle' as const,
    providerRestoreState: 'none' as const, canFork: false, title, cwd: '/tmp', activeChildCount: 0,
    stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [],
    lastUserInteractionSeq: 0
  }
}
