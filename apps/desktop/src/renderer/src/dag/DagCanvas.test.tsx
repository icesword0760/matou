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

  it('renders parent, focused, and child depths fully while farther nodes stay as directional ghosts', () => {
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

    expect(screen.getByRole('button', { name: '远层会话：Depth 0' }).getAttribute('data-ghost')).toBe('true')
    expect(screen.getByRole('button', { name: '打开会话：Depth 1' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '打开会话：Depth 2' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '打开会话：Depth 3' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '远层会话：Depth 4' }).getAttribute('data-ghost')).toBe('true')
    expect(document.querySelectorAll('.dag-node-card')).toHaveLength(5)
    expect(document.querySelectorAll('.dag-node-card.is-ghost')).toHaveLength(2)
  })

  it('keeps a deep DAG DOM bounded to viewport nodes and two directional ghosts', () => {
    const nodes = Array.from({ length: 5000 }, (_, index) => ({
      ...node(`depth-${index}`, `Depth ${index}`),
      ...(index === 0 ? {} : { parentSessionId: `depth-${index - 1}` })
    }))
    render(<DagCanvas graph={{
      sceneId: 'scene', nodes,
      edges: nodes.slice(1).map((item, index) => ({
        parentSessionId: `depth-${index}`, childSessionId: item.sessionId,
        relationKind: 'derived-from' as const, createdAt: index + 1
      }))
    }} focusedSessionId="depth-2500" onSelect={vi.fn()} />)

    expect(document.querySelectorAll('.dag-node-card').length).toBeLessThanOrEqual(5)
    expect(document.querySelectorAll('.dag-node-card.is-ghost').length).toBeLessThanOrEqual(2)
  })

  it('shows full path context and labels Fork versus ordinary relation semantics', () => {
    render(<DagCanvas graph={graph()} focusedSessionId="child" onSelect={vi.fn()} />)

    expect(screen.getByRole('button', { name: '打开会话：Child' }).textContent).toContain('/tmp')
    expect(document.querySelector('.dag-edge.relation-forked-from')?.getAttribute('data-relation-label'))
      .toContain('继承父会话对话上下文')
    expect(screen.getByText('Fork：继承对话')).toBeTruthy()
    expect(screen.getByText('普通关联：不继承对话')).toBeTruthy()
  })

  it('shows the same blue breathing border for a node with a pending notification', () => {
    render(<DagCanvas graph={graph()} focusedSessionId="root" onSelect={vi.fn()}
      notifiedSessionIds={['child']} />)

    const root = screen.getByRole('button', { name: '打开会话：Root' })
    const child = screen.getByRole('button', { name: '打开会话：Child' })
    expect(root.classList.contains('has-notification')).toBe(false)
    expect(child.classList.contains('has-notification')).toBe(true)
  })

  it('renders a legacy archived node as stopped without exposing a history concept', () => {
    const archived = {
      ...node('archived', 'Archived child'),
      workStatus: 'starting' as const,
      archivedAt: 1_730_000_000_000
    }
    render(<DagCanvas graph={{ sceneId: 'scene', nodes: [archived], edges: [] }}
      focusedSessionId="archived" onSelect={vi.fn()} />)

    const card = screen.getByRole('button', { name: '打开会话：Archived child' })
    expect(card.classList.contains('is-stopped')).toBe(true)
    expect(card.classList.contains('is-historical')).toBe(false)
    expect(card.classList.contains('status-exited')).toBe(true)
    expect(card.classList.contains('status-starting')).toBe(false)
    expect(card.querySelector('.dag-node-card__top')?.textContent).toContain('已停止')
    expect(card.textContent).not.toContain('历史')
    expect(card.querySelector('.dag-node-card__top')?.textContent).not.toContain('运行中')
  })

  it('identifies Agent Teams nodes as teammates and shows their latest real output', () => {
    const teammate = {
      ...node('teammate', 'MATOU_QA_TEAMMATE'),
      currentMode: 'agent-team-member' as const,
      workStatus: 'idle' as const,
      latestLines: ['TEAMMATE_REAL_READY']
    }
    render(<DagCanvas graph={{ sceneId: 'scene', nodes: [teammate], edges: [] }}
      focusedSessionId="teammate" onSelect={vi.fn()} />)

    const card = screen.getByRole('button', { name: '打开会话：MATOU_QA_TEAMMATE' })
    expect(card.querySelector('.dag-node-card__top')?.textContent).toContain('队友')
    expect(card.textContent).toContain('TEAMMATE_REAL_READY')
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
    stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [], lastUserInteractionSeq: 0
  }
}
