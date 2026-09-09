// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'
import { RemoveNodeDialog } from './RemoveNodeDialog'

afterEach(() => {
  cleanup()
  setCurrentLocale('zh-CN')
})

describe('Remove node dialog in English', () => {
  it('states the descendant impact and names the scope choices in English', () => {
    render(<LocaleProvider initialLocale="en">
      <RemoveNodeDialog title="Login flow" current={node('root')}
        descendants={[node('a'), node('b'), node('c')]}
        onCancel={vi.fn()} onConfirm={vi.fn()} />
    </LocaleProvider>)

    expect(screen.getByRole('alertdialog', { name: 'Remove node "Login flow"?' })).toBeTruthy()
    expect(screen.getByText('This node and its 3 descendant sessions will be removed.')).toBeTruthy()
    expect(screen.getByRole('radio', { name: /Remove this node and all descendants/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Remove this node' })).toBeTruthy()
  })

  it('names the confirm button of a leaf node "Remove"', () => {
    render(<LocaleProvider initialLocale="en">
      <RemoveNodeDialog title="Login flow" current={node('root')} descendants={[]}
        onCancel={vi.fn()} onConfirm={vi.fn()} />
    </LocaleProvider>)

    expect(screen.getByRole('button', { name: 'Remove' })).toBeTruthy()
    expect(screen.getByText(
      'Affects 1 session, 0 owned worktrees. ' +
      'Once removed, the session disappears from the session list and the DAG.'
    )).toBeTruthy()
  })
})

function node(sessionId: string): SessionGraphNodeView {
  return {
    sessionId, sceneId: 'scene-1', currentMode: 'shell', workStatus: 'exited',
    providerRestoreState: 'none', canFork: false, title: sessionId, cwd: '/tmp',
    hasOwnedWorktree: false, activeChildCount: 0, stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}
