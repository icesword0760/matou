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

  it('unfolds and reveals a historical Session selected from the DAG', async () => {
    const data = graph()
    data.nodes.push({ ...node('archived', '历史 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' })
    render(<SessionCanvas graph={data} levelParentSessionId="parent" onActivate={() => undefined}
      revealRequest={{ sessionId: 'archived', sequence: 1, historical: true }}
      onCreateShellSibling={vi.fn()} onCreateForkSibling={vi.fn()}
      onReopenHistorical={vi.fn()} renderSession={(item) => <div>{item.title}</div>} />)

    expect(await screen.findByText('历史 Shell')).toBeTruthy()
    const reopen = screen.getByRole('button', { name: '重新打开 Shell' })
    expect(reopen).toBeTruthy()
    await vi.waitFor(() => expect(document.activeElement).toBe(reopen))
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

  it('removes an exited leaf only after a clear confirmation', async () => {
    const data = graph()
    data.nodes.push({ ...node('archived', '历史 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' })
    const onRemoveHistorical = vi.fn()
    renderCanvas(data, { onRemoveHistorical })
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '显示历史会话 (1)' }))
    await user.click(screen.getByRole('button', { name: '移除历史会话：历史 Shell' }))
    expect(screen.getByRole('alertdialog', { name: '移除历史会话' }).textContent)
      .toContain('只会从当前画布移除这个历史节点')
    await user.click(screen.getByRole('button', { name: '确认移除' }))
    expect(onRemoveHistorical).toHaveBeenCalledWith('archived', false)
  })

  it('keeps a historical parent navigable and double-confirms whole-branch removal', async () => {
    const data = graph()
    data.nodes = [
      { ...node('archived-parent', '历史父会话'), archivedAt: 20, workStatus: 'exited' },
      { ...node('descendant', '仍在工作的子会话', 'archived-parent'), workStatus: 'running' },
      { ...node('grandchild', '孙会话', 'descendant'), workStatus: 'needs-input' }
    ]
    data.edges = [
      { parentSessionId: 'archived-parent', childSessionId: 'descendant', relationKind: 'derived-from', createdAt: 1 },
      { parentSessionId: 'descendant', childSessionId: 'grandchild', relationKind: 'derived-from', createdAt: 2 }
    ]
    data.focusedSessionId = 'descendant'
    const onNavigateToChildren = vi.fn()
    const onRemoveHistorical = vi.fn()
    render(<SessionCanvas graph={data} levelParentSessionId={null} onActivate={() => undefined}
      onCreateShellSibling={vi.fn()} onCreateForkSibling={vi.fn()} onReopenHistorical={vi.fn()}
      onNavigateToChildren={onNavigateToChildren} onRemoveHistorical={onRemoveHistorical}
      renderSession={(item) => <div>{item.title}</div>} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '查看 1 个子会话' }))
    expect(onNavigateToChildren).toHaveBeenCalledWith('archived-parent')
    await user.click(screen.getByRole('button', { name: '移除整条分支：历史父会话' }))
    expect(screen.getByRole('alertdialog', { name: '移除整条分支' }).textContent)
      .toContain('2 个后代节点')
    expect(screen.getByRole('alertdialog', { name: '移除整条分支' }).textContent)
      .toContain('1 个运行中、1 个待输入')
    expect(screen.getByRole('alertdialog', { name: '移除整条分支' }).textContent)
      .toContain('本地工作树和未提交修改会继续保留')
    await user.click(screen.getByRole('button', { name: '继续' }))
    expect(screen.getByRole('alertdialog', { name: '再次确认' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '移除整条分支' }))
    expect(onRemoveHistorical).toHaveBeenCalledWith('archived-parent', true)
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
    expect(Element.prototype.scrollIntoView).not.toHaveBeenCalled()
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

  it('retries a geometry write that races the latest authoritative layout revision', async () => {
    vi.useFakeTimers()
    const onPutGeometry = vi.fn()
      .mockRejectedValueOnce(new Error('layout revision changed'))
      .mockResolvedValueOnce(undefined)
    render(<SessionCanvas graph={graph()} onActivate={() => undefined}
      onCreateShellSibling={vi.fn()} onCreateForkSibling={vi.fn()}
      onReopenHistorical={vi.fn()} onPutGeometry={onPutGeometry}
      renderSession={(item) => <div>{item.title}</div>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' })
    viewport.scrollLeft = 96
    fireEvent.focus(screen.getByLabelText('会话：Shell 子会话'))
    expect(screen.getByRole('region', { name: '会话画布' }).getAttribute('aria-busy')).toBe('true')

    await vi.advanceTimersByTimeAsync(180)
    expect(onPutGeometry).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(120)
    expect(onPutGeometry).toHaveBeenCalledTimes(2)
    vi.useRealTimers()
  })

  it('keeps newer geometry when an older in-flight write fails', async () => {
    vi.useFakeTimers()
    let rejectFirst: ((reason: Error) => void) | undefined
    const firstWrite = new Promise<void>((_resolve, reject) => { rejectFirst = reject })
    const onPutGeometry = vi.fn()
      .mockReturnValueOnce(firstWrite)
      .mockResolvedValueOnce(undefined)
    render(<SessionCanvas graph={graph()} onActivate={() => undefined}
      onCreateShellSibling={vi.fn()} onCreateForkSibling={vi.fn()}
      onReopenHistorical={vi.fn()} onPutGeometry={onPutGeometry}
      renderSession={(item) => <div>{item.title}</div>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' })

    viewport.scrollLeft = 96
    fireEvent.focus(screen.getByLabelText('会话：Shell 子会话'))
    await vi.advanceTimersByTimeAsync(180)
    expect(onPutGeometry).toHaveBeenCalledTimes(1)

    viewport.scrollLeft = 144
    fireEvent.focus(screen.getByLabelText('会话：Shell 子会话'))
    rejectFirst?.(new Error('layout revision changed'))
    await vi.advanceTimersByTimeAsync(180)

    expect(onPutGeometry).toHaveBeenCalledTimes(2)
    expect(onPutGeometry).toHaveBeenLastCalledWith(
      'session-group:scene-1:parent',
      expect.objectContaining({ scrollLeft: 144, focusedSessionId: 'child-shell' })
    )
    vi.useRealTimers()
  })
})

function renderCanvas(data: SessionGraphView, handlers?: {
  onCreateShellSibling?: (sourceSessionId: string, parentSessionId?: string) => void
  onCreateForkSibling?: (source: SessionGraphNodeView, parent: SessionGraphNodeView) => void
  onReopenHistorical?: (sessionId: string) => void
  onRemoveHistorical?: (sessionId: string, includeDescendants: boolean) => void
}) {
  return render(<SessionCanvas graph={data} onActivate={() => undefined}
    onCreateShellSibling={handlers?.onCreateShellSibling ?? vi.fn()}
    onCreateForkSibling={handlers?.onCreateForkSibling ?? vi.fn()}
    onReopenHistorical={handlers?.onReopenHistorical ?? vi.fn()}
    onRemoveHistorical={handlers?.onRemoveHistorical ?? vi.fn()}
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
