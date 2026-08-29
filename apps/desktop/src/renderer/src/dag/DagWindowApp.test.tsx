// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagWindowApp } from './DagWindowApp'

beforeEach(() => {
  window.history.replaceState({}, '', '/?kind=dag&mainWindowId=main-1&sceneId=scene-1&sessionId=child&theme=light')
  Object.defineProperty(window, 'matouDesktop', { configurable: true, value: {
    selectDagNode: vi.fn(), closeDagWindow: vi.fn(), onDagContext: vi.fn(() => () => {})
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
    historicalChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [],
    lastUserInteractionSeq: 0
  }
}
