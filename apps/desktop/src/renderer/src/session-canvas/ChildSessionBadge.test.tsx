// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ChildSessionBadge } from './ChildSessionBadge'

afterEach(cleanup)

describe('ChildSessionBadge', () => {
  it('uses one direct-child count for the badge and the list without a history category', () => {
    render(<ChildSessionBadge children={[
      node('claude', 'claude-code', 'running'),
      node('shell', 'shell', 'needs-input'),
      { ...node('stopped', 'shell', 'exited'), archivedAt: 20 }
    ]} onOpen={() => undefined} />)

    const badge = screen.getByRole('button', { name: '查看 3 个子会话' })
    expect(badge.textContent).toContain('3 分支 · 1 待输入')
    expect(badge.className).toContain('status-needs-input')
    expect(badge.getAttribute('title')).toBeNull()
    const tooltip = screen.getByRole('tooltip')
    expect(screen.getAllByRole('tooltip')).toHaveLength(1)
    expect(tooltip.textContent).toContain('Claude 1 · Shell 2')
    expect(tooltip.textContent).toContain('运行中 1 · 待输入 1')
    expect(tooltip.textContent).not.toContain('已停止')
    expect(tooltip.textContent).not.toContain('历史')
  })

  it('does not render a badge when the node has no direct children', () => {
    render(<ChildSessionBadge children={[]} onOpen={() => undefined} />)
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('uses error before needs-input and running as the aggregate state', () => {
    render(<ChildSessionBadge children={[
      node('running', 'shell', 'running'), node('waiting', 'shell', 'needs-input'),
      node('error', 'claude-code', 'error')
    ]} onOpen={() => undefined} />)

    const badge = screen.getByRole('button')
    expect(badge.className).toContain('status-error')
    expect(badge.textContent).toContain('3 分支 · 1 异常')
    expect(badge.textContent).not.toContain('空闲')
  })

  it('keeps quiet branch groups count-only instead of surfacing internal lifecycle states', () => {
    render(<ChildSessionBadge children={[
      { ...node('stopped-1', 'shell', 'exited'), archivedAt: 20 },
      { ...node('stopped-2', 'claude-code', 'exited'), archivedAt: 21 }
    ]} onOpen={() => undefined} />)

    expect(screen.getByRole('button').textContent).toBe('2 分支›')
    expect(screen.getByRole('tooltip').textContent).toBe('Claude 1 · Shell 1')
  })

  it('distinguishes preparing from running', () => {
    render(<ChildSessionBadge children={[
      node('starting', 'claude-code', 'starting'),
      node('idle', 'shell', 'idle')
    ]} onOpen={() => undefined} />)

    expect(screen.getByRole('button').textContent).toContain('2 分支 · 1 准备中')
    expect(screen.getByRole('button').textContent).not.toContain('运行中')
    expect(screen.getByRole('tooltip').textContent).toContain('准备中 1')
  })

  it('opens the direct child level even when the parent currently displays as Shell', async () => {
    const onOpen = vi.fn()
    render(<ChildSessionBadge children={[node('child', 'shell', 'idle')]} onOpen={onOpen} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '查看 1 个子会话' }))
    expect(onOpen).toHaveBeenCalledOnce()
  })
})

function node(
  sessionId: string,
  currentMode: SessionGraphNodeView['currentMode'],
  workStatus: SessionGraphNodeView['workStatus']
): SessionGraphNodeView {
  return {
    sessionId, sceneId: 'scene-1', parentSessionId: 'parent', currentMode, workStatus,
    providerRestoreState: 'none', canFork: false, title: sessionId, cwd: '/tmp',
    activeChildCount: 0, stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}
