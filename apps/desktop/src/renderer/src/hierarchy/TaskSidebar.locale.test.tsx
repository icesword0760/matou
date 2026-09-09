// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'
import { TaskSidebar } from './TaskSidebar'
import { WorkspaceKanbanBoard } from './WorkspaceKanbanBoard'
import { WorkspaceSwitcher } from './WorkspaceSwitcher'
import { commands, fixture } from './hierarchy-test-fixtures'

describe('Hierarchy chrome in English', () => {
  afterEach(() => {
    cleanup()
    setCurrentLocale('zh-CN')
  })

  it('labels the create button and workspace switcher', () => {
    const projection = fixture()
    render(<LocaleProvider initialLocale="en">
      <TaskSidebar projection={projection} commands={commands()} />
      <WorkspaceSwitcher projection={projection} commands={commands()} />
    </LocaleProvider>)

    expect(screen.getByRole('button', { name: 'New task in Frontend' })).toBeTruthy()
    // Playwright matches an accessible name as a case-insensitive substring, so the
    // downstream `New task` lookup still resolves against the per-workspace name.
    expect(screen.getByRole('button', { name: /New task/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Switch workspace' })).toBeTruthy()
  })

  it('names the board columns in English', () => {
    const projection = fixture()
    render(<LocaleProvider initialLocale="en">
      <WorkspaceKanbanBoard workspace={projection.workspaces[0]!} tasks={projection.tasks}
        sessionCountByTask={{}} onMoveTask={() => undefined} />
    </LocaleProvider>)

    expect(screen.getAllByRole('group').map((column) => column.getAttribute('aria-label')))
      .toEqual(['Ready column', 'Active column', 'Blocked column', 'Done column'])
  })
})
