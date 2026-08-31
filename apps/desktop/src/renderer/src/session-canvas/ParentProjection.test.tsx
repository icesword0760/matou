// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { ParentProjection } from './ParentProjection'

afterEach(cleanup)

describe('ParentProjection', () => {
  it('reveals the parent summary gradually and confirms only at the threshold', () => {
    const { rerender } = render(<ParentProjection parent={parent()} pullDistance={54} progress={0.4} />)

    expect(screen.getByText('父会话')).toBeTruthy()
    expect(screen.getByText('等待输入')).toBeTruthy()
    expect(screen.getByText(/最近一行/)).toBeTruthy()
    expect(screen.getByText('继续右拉返回')).toBeTruthy()

    rerender(<ParentProjection parent={parent()} pullDistance={140} progress={1} />)
    expect(screen.getByText('松手返回父会话')).toBeTruthy()
    expect(screen.getByTestId('parent-projection').getAttribute('data-ready')).toBe('true')
  })
})

function parent(): SessionGraphNodeView {
  return {
    sessionId: 'parent', sceneId: 'scene', currentMode: 'claude-code',
    workStatus: 'needs-input', providerRestoreState: 'none', canFork: true,
    title: '父会话', cwd: '/tmp', activeChildCount: 2, stoppedChildCount: 0,
    childModeCounts: { shell: 1, claudeCode: 1 }, latestLines: ['较早一行', '最近一行'],
    lastUserInteractionSeq: 2
  }
}
