// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
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
    expect(onCreateShellSibling).toHaveBeenCalledWith('child-claude')
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
})

function renderCanvas(data: SessionGraphView, handlers?: {
  onCreateShellSibling?: (sourceSessionId: string) => void
  onCreateForkSibling?: (source: SessionGraphNodeView, parent: SessionGraphNodeView) => void
}) {
  return render(<SessionCanvas graph={data} onActivate={() => undefined}
    onCreateShellSibling={handlers?.onCreateShellSibling ?? vi.fn()}
    onCreateForkSibling={handlers?.onCreateForkSibling ?? vi.fn()}
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
