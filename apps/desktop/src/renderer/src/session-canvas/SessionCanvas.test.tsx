// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView, SessionGraphView } from '../hierarchy/hierarchy-types'
import { SessionCanvas } from './SessionCanvas'

beforeEach(() => { Element.prototype.scrollIntoView = vi.fn() })
afterEach(cleanup)

describe('SessionCanvas', () => {
  it('projects the focused node sibling level with a distinct active card', () => {
    renderCanvas(graph())

    expect(screen.getByText('父会话 的子会话')).toBeTruthy()
    expect(screen.getAllByLabelText(/^会话：/).map((node) => node.getAttribute('aria-label')))
      .toEqual(['会话：Shell 子会话', '会话：Claude 子会话'])
    expect(screen.getByLabelText('会话：Claude 子会话').getAttribute('aria-current')).toBe('true')
    expect(screen.queryByLabelText('会话：父会话')).toBeNull()
  })

  it('creates an ordinary Shell sibling directly and offers a Fork sibling only for a valid common Claude parent', async () => {
    const onCreateShellSibling = vi.fn()
    const onCreateForkSibling = vi.fn()
    const data = graph()
    renderCanvas(data, { onCreateShellSibling, onCreateForkSibling })
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '新增同级 Shell' }))
    expect(onCreateShellSibling).toHaveBeenCalledWith('child-claude', 'parent')
    await user.click(screen.getByRole('button', { name: '创建同级 Claude 分支' }))
    expect(onCreateForkSibling).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: 'child-claude' }),
      expect.objectContaining({ sessionId: 'parent', canFork: true })
    )
  })

  it('keeps a mixed Shell and Claude root list without inventing persistent sibling edges', () => {
    const data = graph()
    data.focusedSessionId = 'parent'
    data.nodes.push(node('root-shell', '根 Shell'))
    renderCanvas(data)

    expect(screen.getByText('根会话')).toBeTruthy()
    expect(screen.getAllByLabelText(/^会话：/).map((element) => element.getAttribute('aria-label')))
      .toEqual(['会话：父会话', '会话：根 Shell'])
    expect(screen.queryByRole('button', { name: '创建同级 Claude 分支' })).toBeNull()
  })

  it('keeps history folded by default and reopens it as a new live continuation', async () => {
    const data = graph()
    data.nodes.push({ ...node('archived', '历史 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' })
    const onReopenHistorical = vi.fn()
    renderCanvas(data, { onReopenHistorical })
    const user = userEvent.setup()

    expect(screen.queryByText('历史 Shell')).toBeNull()
    await user.click(screen.getByRole('button', { name: '显示历史会话 (1)' }))
    expect(screen.getByText('历史 Shell')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '重新打开 Shell' }))
    expect(onReopenHistorical).toHaveBeenCalledWith('archived')
  })

  it('keeps the current parent when adding a Shell after all children became history', async () => {
    const data = graph()
    data.nodes = [
      data.nodes[0]!,
      { ...node('archived', '历史 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' }
    ]
    data.focusedSessionId = 'parent'
    const onCreateShellSibling = vi.fn()
    render(<SessionCanvas graph={data} levelParentSessionId="parent" onActivate={() => undefined}
      onCreateShellSibling={onCreateShellSibling} onCreateForkSibling={vi.fn()}
      onReopenHistorical={vi.fn()} renderSession={(item) => <div>{item.title}</div>} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '新增同级 Shell' }))

    expect(onCreateShellSibling).toHaveBeenCalledWith('archived', 'parent')
  })

  it('provides an explicit keyboard-accessible return to the parent', async () => {
    const onReturnParent = vi.fn()
    render(<SessionCanvas graph={graph()} onActivate={() => undefined}
      onCreateShellSibling={vi.fn()} onCreateForkSibling={vi.fn()}
      onReopenHistorical={vi.fn()} onReturnParent={onReturnParent}
      renderSession={(item) => <div>{item.title}</div>} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '返回父会话' }))

    expect(onReturnParent).toHaveBeenCalledWith('parent')
  })

  it('restores the sibling viewport and persists navigation geometry after a short debounce', () => {
    vi.useFakeTimers()
    const onPutGeometry = vi.fn()
    render(<SessionCanvas graph={graph()} onActivate={() => undefined}
      onCreateShellSibling={vi.fn()} onCreateForkSibling={vi.fn()}
      onReopenHistorical={vi.fn()} onPutGeometry={onPutGeometry}
      geometry={[{ ownerKey: 'session-group:scene-1:parent', geometry: { scrollLeft: 77 } }]}
      renderSession={(item) => <div>{item.title}</div>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' })

    expect(viewport.scrollLeft).toBe(77)
    viewport.scrollLeft = 128
    fireEvent.focus(screen.getByLabelText('会话：Shell 子会话'))
    vi.advanceTimersByTime(179)
    expect(onPutGeometry).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(onPutGeometry).toHaveBeenCalledWith(
      'session-group:scene-1:parent',
      expect.objectContaining({ scrollLeft: 128, focusedSessionId: 'child-shell' })
    )
    vi.useRealTimers()
  })
})

function renderCanvas(data: SessionGraphView, handlers?: {
  onCreateShellSibling?: (sourceSessionId: string, parentSessionId?: string) => void
  onCreateForkSibling?: (source: SessionGraphNodeView, parent: SessionGraphNodeView) => void
  onReopenHistorical?: (sessionId: string) => void
}) {
  return render(<SessionCanvas graph={data} onActivate={() => undefined}
    onCreateShellSibling={handlers?.onCreateShellSibling ?? vi.fn()}
    onCreateForkSibling={handlers?.onCreateForkSibling ?? vi.fn()}
    onReopenHistorical={handlers?.onReopenHistorical ?? vi.fn()}
    renderSession={(item) => <div>{item.title}</div>} />)
}

function graph(): SessionGraphView {
  return {
    sceneId: 'scene-1', focusedSessionId: 'child-claude',
    nodes: [
      node('parent', '父会话', undefined, true, 'claude-code'),
      node('child-shell', 'Shell 子会话', 'parent'),
      node('child-claude', 'Claude 子会话', 'parent', false, 'claude-code')
    ],
    edges: [
      { parentSessionId: 'parent', childSessionId: 'child-shell', relationKind: 'derived-from', createdAt: 1 },
      { parentSessionId: 'parent', childSessionId: 'child-claude', relationKind: 'forked-from', createdAt: 2 }
    ]
  }
}

function node(
  sessionId: string,
  title: string,
  parentSessionId?: string,
  canFork = false,
  currentMode: SessionGraphNodeView['currentMode'] = 'shell'
): SessionGraphNodeView {
  return {
    sessionId, sceneId: 'scene-1', ...(parentSessionId ? { parentSessionId } : {}),
    currentMode, workStatus: 'idle', providerRestoreState: 'none', canFork,
    title, cwd: '/tmp', activeChildCount: 0, historicalChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}
