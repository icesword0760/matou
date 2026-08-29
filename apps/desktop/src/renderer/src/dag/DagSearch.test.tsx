// @vitest-environment jsdom

import { cleanup, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { DagSearch } from './DagSearch'

afterEach(cleanup)

describe('DagSearch', () => {
  it('selects search results with keyboard and centers before opening', async () => {
    const onPreview = vi.fn()
    const onChoose = vi.fn()
    render(<DagSearch nodes={[node('a', 'Alpha'), node('b', 'Beta')]} onPreview={onPreview} onChoose={onChoose} />)
    const user = userEvent.setup()

    await user.type(screen.getByRole('searchbox', { name: '搜索会话' }), 'beta')
    expect(screen.getByRole('option', { name: /Beta/ })).toBeTruthy()
    await user.keyboard('{Enter}')

    expect(onPreview).toHaveBeenCalledWith('b')
    await waitFor(() => expect(onChoose).toHaveBeenCalledWith('b'))
  })
})

function node(sessionId: string, title: string): SessionGraphNodeView {
  return {
    sessionId, sceneId: 'scene', currentMode: 'shell', workStatus: 'idle',
    providerRestoreState: 'none', canFork: false, title, cwd: '/tmp', activeChildCount: 0,
    historicalChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [], lastUserInteractionSeq: 0
  }
}
