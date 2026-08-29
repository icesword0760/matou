// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ChildSessionBadge } from './ChildSessionBadge'

afterEach(cleanup)

describe('ChildSessionBadge', () => {
  it('shows active and historical totals with the full mixed-mode status breakdown', () => {
    render(<ChildSessionBadge children={[
      node('claude', 'claude-code', 'running'),
      node('shell', 'shell', 'needs-input')
    ]} historicalCount={2} onOpen={() => undefined} />)

    const badge = screen.getByRole('button', { name: '查看 4 个子会话' })
    expect(badge.className).toContain('status-needs-input')
    expect(badge.getAttribute('title')).toContain('Claude 1 · Shell 1')
    expect(badge.getAttribute('title')).toContain('运行中 1 · 待输入 1')
    expect(badge.getAttribute('title')).toContain('+2 历史')
  })

  it('uses error before needs-input and running as the aggregate state', () => {
    render(<ChildSessionBadge children={[
      node('running', 'shell', 'running'), node('waiting', 'shell', 'needs-input'),
      node('error', 'claude-code', 'error')
    ]} historicalCount={0} onOpen={() => undefined} />)

    expect(screen.getByRole('button').className).toContain('status-error')
  })

  it('opens the direct child level even when the parent currently displays as Shell', async () => {
    const onOpen = vi.fn()
    render(<ChildSessionBadge children={[node('child', 'shell', 'idle')]}
      historicalCount={0} onOpen={onOpen} />)

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
    activeChildCount: 0, historicalChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}
