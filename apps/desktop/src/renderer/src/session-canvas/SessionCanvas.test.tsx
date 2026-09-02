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

    expect(document.querySelector('.session-level-header')).toBeNull()
    expect(screen.getAllByLabelText(/^会话：/).map((node) => node.getAttribute('aria-label')))
      .toEqual(['会话：Shell 子会话', '会话：Claude 子会话'])
    expect(screen.getByLabelText('会话：Claude 子会话').getAttribute('aria-current')).toBe('true')
    expect(screen.queryByLabelText('会话：父会话')).toBeNull()
  })

  it('leaves level-wide creation outside the canvas and keeps Fork on each card header', () => {
    renderCanvas(graph())

    expect(screen.queryByRole('button', { name: '新增同级 Shell' })).toBeNull()
    expect(screen.queryByRole('button', { name: '创建同级 Claude 分支' })).toBeNull()
  })

  it('keeps a mixed Shell and Claude root list without inventing persistent sibling edges', () => {
    const data = graph()
    data.focusedSessionId = 'parent'
    data.nodes.push(node('root-shell', '根 Shell'))
    renderCanvas(data)

    expect(screen.getAllByLabelText(/^会话：/).map((element) => element.getAttribute('aria-label')))
      .toEqual(['会话：父会话', '会话：根 Shell'])
    expect(screen.queryByRole('button', { name: '创建同级 Claude 分支' })).toBeNull()
  })

  it('keeps stopped nodes in the ordinary horizontal list without a history projection', () => {
    const data = graph()
    data.nodes.push({ ...node('stopped', '已停止 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' })
    renderCanvas(data)

    expect(screen.getByText('已停止 Shell')).toBeTruthy()
    expect(screen.queryByRole('button', { name: /历史会话/ })).toBeNull()
    expect(screen.queryByText('历史会话')).toBeNull()
  })

  it('keeps only structural removal on a stopped card without process controls', async () => {
    const data = graph()
    data.nodes.push(
      { ...node('stopped', '已停止 Shell', 'parent'), archivedAt: 20, workStatus: 'exited', hasOwnedWorktree: true },
      { ...node('stopped-child', '已停止子节点', 'stopped'), archivedAt: 21, workStatus: 'exited', hasOwnedWorktree: true }
    )
    const onRemoveBranch = vi.fn()
    renderCanvas(data, { onRemoveBranch })
    const user = userEvent.setup()

    expect(screen.queryByRole('button', { name: '重新启动' })).toBeNull()
    await user.click(screen.getByRole('button', { name: '移除节点…：已停止 Shell' }))

    const dialog = screen.getByRole('alertdialog')
    expect(dialog.textContent).toContain('影响 1 个会话、1 个自有 Worktree')
    expect(dialog.textContent).toContain('影响 2 个会话、2 个自有 Worktree')
    await user.click(screen.getByRole('radio', { name: /移除当前节点及全部后代/ }))
    await user.click(screen.getByRole('button', { name: '移除 2 个会话' }))
    expect(onRemoveBranch).toHaveBeenCalledWith('stopped', 'node-and-descendants')
  })

  it('keeps stopped-session navigation visible but disables removal with the recovery reason', async () => {
    const data = graph()
    data.nodes.push(
      { ...node('stopped', '已停止 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' }
    )
    const onRemoveBranch = vi.fn()
    render(<SessionCanvas graph={data} disabled disabledReason="数据库处于只读恢复模式"
      onActivate={() => undefined} onRemoveBranch={onRemoveBranch}
      renderSession={(item) => <div>{item.title}</div>} />)

    const remove = screen.getByRole('button', { name: '移除节点…：已停止 Shell' })
    expect(remove.hasAttribute('disabled')).toBe(true)
    expect(remove.getAttribute('title')).toBe('数据库处于只读恢复模式')
    await userEvent.setup().click(remove)
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(onRemoveBranch).not.toHaveBeenCalled()
  })

  it('reveals a stopped Session selected from the DAG without changing projections', async () => {
    const data = graph()
    data.nodes.push({ ...node('archived', '已停止 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' })
    render(<SessionCanvas graph={data} levelParentSessionId="parent" onActivate={() => undefined}
      revealRequest={{ sessionId: 'archived', sequence: 1, stopped: true }}
      renderSession={(item) => <div>{item.title}</div>} />)

    expect(await screen.findByText('已停止 Shell')).toBeTruthy()
    expect(screen.queryByRole('button', { name: '重新启动' })).toBeNull()
  })

  it('keeps the current parent projection after all children stopped', () => {
    const data = graph()
    data.nodes = [
      data.nodes[0]!,
      { ...node('archived', '已停止 Shell', 'parent'), archivedAt: 20, workStatus: 'exited' }
    ]
    data.focusedSessionId = 'parent'
    render(<SessionCanvas graph={data} levelParentSessionId="parent" onActivate={() => undefined}
      renderSession={(item) => <div>{item.title}</div>} />)

    expect(screen.getByRole('region', { name: '会话画布' }).getAttribute('data-parent-session-id'))
      .toBe('parent')
    expect(screen.getByText('已停止 Shell')).toBeTruthy()
  })

  it('restores the sibling viewport and persists navigation geometry after a short debounce', () => {
    vi.useFakeTimers()
    const onPutGeometry = vi.fn()
    render(<SessionCanvas graph={graph()} onActivate={() => undefined}
      onPutGeometry={onPutGeometry}
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

  it('keeps canvas browsing local without persisting geometry while recovery is read-only', () => {
    vi.useFakeTimers()
    const onPutGeometry = vi.fn()
    const onActivate = vi.fn()
    render(<SessionCanvas graph={graph()} disabled disabledReason="数据库处于只读恢复模式"
      onActivate={onActivate} onPutGeometry={onPutGeometry}
      renderSession={(item) => <div>{item.title}</div>} />)
    const viewport = screen.getByRole('region', { name: '同级会话列表' })

    viewport.scrollLeft = 128
    fireEvent.focus(screen.getByLabelText('会话：Shell 子会话'))
    vi.advanceTimersByTime(500)

    expect(onPutGeometry).not.toHaveBeenCalled()
    expect(onActivate).toHaveBeenCalledWith('child-shell')
    vi.useRealTimers()
  })

  it('retries a geometry write that races the latest authoritative layout revision', async () => {
    vi.useFakeTimers()
    const onPutGeometry = vi.fn()
      .mockRejectedValueOnce(new Error('layout revision changed'))
      .mockResolvedValueOnce(undefined)
    render(<SessionCanvas graph={graph()} onActivate={() => undefined}
      onPutGeometry={onPutGeometry}
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
      onPutGeometry={onPutGeometry}
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
  onRemoveBranch?: (sessionId: string, scope: 'node-only' | 'node-and-descendants') => void
}) {
  return render(<SessionCanvas graph={data} onActivate={() => undefined}
    {...(handlers?.onRemoveBranch ? { onRemoveBranch: handlers.onRemoveBranch } : {})}
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
    title, cwd: '/tmp', hasOwnedWorktree: false, activeChildCount: 0, stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}
