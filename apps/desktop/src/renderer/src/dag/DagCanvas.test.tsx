// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { DagCanvas, clampDagScale, zoomAt } from './DagCanvas'

beforeEach(() => { Element.prototype.scrollIntoView = vi.fn() })
afterEach(cleanup)

describe('DagCanvas', () => {
  it('clamps zoom to the PRD range and preserves the world point under the pointer', () => {
    expect(clampDagScale(0.1)).toBe(0.4)
    expect(clampDagScale(3)).toBe(2)
    expect(zoomAt({ x: 10, y: 20, scale: 1 }, 2, { x: 110, y: 120 }))
      .toEqual({ x: -90, y: -80, scale: 2 })
  })

  it('pans smoothly, zooms, restores 100 percent and selects a node', async () => {
    const onSelect = vi.fn()
    render(<DagCanvas graph={graph()} focusedSessionId="child" onSelect={onSelect} />)
    const canvas = screen.getByRole('application', { name: '会话 DAG 画布' })
    Object.defineProperty(canvas, 'clientWidth', { configurable: true, value: 1000 })
    Object.defineProperty(canvas, 'clientHeight', { configurable: true, value: 700 })

    fireEvent.wheel(canvas, { deltaX: 40, deltaY: 30 })
    expect(canvas.getAttribute('data-pan')).not.toBe('0,0')
    fireEvent.wheel(canvas, { deltaY: -100, ctrlKey: true, clientX: 500, clientY: 350 })
    expect(Number(canvas.getAttribute('data-scale'))).toBeGreaterThan(1)
    await userEvent.setup().click(screen.getByRole('button', { name: '恢复 100%' }))
    expect(canvas.getAttribute('data-scale')).toBe('1')

    await userEvent.setup().click(screen.getByRole('button', { name: '打开会话：Child' }))
    expect(onSelect).toHaveBeenCalledWith('child')
  })

  it('restores a persisted viewport before rendering and reports later canvas movement', () => {
    const onTransformChange = vi.fn()
    render(<DagCanvas graph={graph()} focusedSessionId="child" onSelect={vi.fn()}
      initialTransform={{ x: 123, y: -45, scale: 1.4 }} onTransformChange={onTransformChange} />)
    const canvas = screen.getByRole('application', { name: '会话 DAG 画布' })

    expect(canvas.getAttribute('data-pan')).toBe('123,-45')
    expect(canvas.getAttribute('data-scale')).toBe('1.4')
    fireEvent.wheel(canvas, { deltaX: 20, deltaY: 10 })
    expect(onTransformChange).toHaveBeenLastCalledWith({ x: 103, y: -55, scale: 1.4 })
  })

  it('renders exactly the parent, focused, and child depths while farther layers stay as ghosts', () => {
    const nodes = [
      node('depth-0', 'Depth 0'),
      { ...node('depth-1', 'Depth 1'), parentSessionId: 'depth-0' },
      { ...node('depth-2', 'Depth 2'), parentSessionId: 'depth-1' },
      { ...node('depth-3', 'Depth 3'), parentSessionId: 'depth-2' },
      { ...node('depth-4', 'Depth 4'), parentSessionId: 'depth-3' }
    ]
    render(<DagCanvas graph={{
      sceneId: 'scene', nodes,
      edges: nodes.slice(1).map((item, index) => ({
        parentSessionId: `depth-${index}`, childSessionId: item.sessionId,
        relationKind: 'derived-from' as const, createdAt: index + 1
      }))
    }} focusedSessionId="depth-2" onSelect={vi.fn()} />)

    expect(screen.queryByRole('button', { name: '打开会话：Depth 0' })).toBeNull()
    expect(screen.getByRole('button', { name: '打开会话：Depth 1' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '打开会话：Depth 2' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '打开会话：Depth 3' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: '打开会话：Depth 4' })).toBeNull()
    expect(screen.getAllByText(/平移到此处查看第/)).toHaveLength(2)
  })
})

function graph(): SessionGraphView {
  return {
    sceneId: 'scene', focusedSessionId: 'child',
    nodes: [node('root', 'Root'), { ...node('child', 'Child'), parentSessionId: 'root', relationKind: 'forked-from' }],
    edges: [{ parentSessionId: 'root', childSessionId: 'child', relationKind: 'forked-from', createdAt: 1 }]
  }
}

function node(sessionId: string, title: string) {
  return {
    sessionId, sceneId: 'scene', currentMode: 'shell' as const, workStatus: 'idle' as const,
    providerRestoreState: 'none' as const, canFork: false, title, cwd: '/tmp', activeChildCount: 0,
    historicalChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [], lastUserInteractionSeq: 0
  }
}
