// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'
import { DagSearch } from './DagSearch'

afterEach(() => {
  cleanup()
  setCurrentLocale('zh-CN')
})

describe('DagSearch in English', () => {
  it('names the search box and the result list in English', async () => {
    render(<LocaleProvider initialLocale="en">
      <DagSearch nodes={[node('a', 'Alpha'), node('b', 'Beta')]}
        onPreview={vi.fn()} onChoose={vi.fn()} />
    </LocaleProvider>)

    const box = screen.getByRole('searchbox', { name: 'Search sessions' })
    expect(box.getAttribute('placeholder')).toBe('Search by name, path, branch or output…')

    await userEvent.setup().type(box, 'beta')
    expect(screen.getByRole('listbox', { name: 'Search results' })).toBeTruthy()
    expect(screen.getByRole('option', { name: /Beta/ })).toBeTruthy()
  })

  it('reports an empty result set in English', async () => {
    render(<LocaleProvider initialLocale="en">
      <DagSearch nodes={[node('a', 'Alpha')]} onPreview={vi.fn()} onChoose={vi.fn()} />
    </LocaleProvider>)

    await userEvent.setup().type(screen.getByRole('searchbox', { name: 'Search sessions' }), 'zzz')
    expect(screen.getByText('No matching sessions')).toBeTruthy()
  })
})

function node(sessionId: string, title: string): SessionGraphNodeView {
  return {
    sessionId, sceneId: 'scene', currentMode: 'shell', workStatus: 'idle',
    providerRestoreState: 'none', canFork: false, title, cwd: '/tmp', activeChildCount: 0,
    stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
    latestLines: [], lastUserInteractionSeq: 0
  }
}
