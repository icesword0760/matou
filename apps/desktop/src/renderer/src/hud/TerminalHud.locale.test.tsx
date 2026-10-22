// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { GitRepositoryStatus } from '@matou/contracts'
import type { SessionEnvironment } from '@matou/domain'

import { LocaleProvider } from '../i18n/LocaleProvider'
import { TerminalHud } from './TerminalHud'
import type { GitRequestClient } from './GitControlMenu'
import type { SessionEnvironmentActions } from './EnvironmentControlMenu'

afterEach(cleanup)

describe('TerminalHud in English', () => {
  it('names the Git control and the environment trigger in English', () => {
    render(<LocaleProvider initialLocale="en"><TerminalHud hud={undefined} sessionId="session-1"
      environment={localEnvironment()} git={{ state: 'ready', branch: 'main', dirty: false }}
      environmentActions={environmentActions()} runtimeClient={{ request: vi.fn() }} /></LocaleProvider>)

    const git = screen.getByRole('button', { name: 'Open Git' })
    expect(git.textContent).toBe('main')
    expect(git.title).toBe('Git branch: main')
    const environment = screen.getByRole('button', { name: 'Open environment: Local' })
    expect(environment.title).toBe('Environment')
  })

  it('opens the environment menu with English recovery actions', async () => {
    render(<LocaleProvider initialLocale="en"><TerminalHud hud={undefined} sessionId="session-1"
      environment={missingWorktree()} git={{ state: 'unavailable', dirty: false }}
      environmentActions={environmentActions()} /></LocaleProvider>)

    expect(screen.getByRole('button', { name: 'Open Git' }).textContent).toBe('Git unavailable')
    await userEvent.setup().click(screen.getByRole('button', { name: 'Open environment: Needs recovery' }))

    expect(screen.getByRole('dialog', { name: 'Environment' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Restore the original worktree' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Locate the moved worktree' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Hand off to local' })).toBeTruthy()
  })

  it('opens the Git control with English branch copy', async () => {
    const runtimeClient: GitRequestClient = {
      request: async function<T>(): Promise<T> { return repositoryStatus() as T }
    }
    render(<LocaleProvider initialLocale="en"><TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', cwd: '/repo', gitBranch: 'main', startedAt: Date.now()
    }} runtimeClient={runtimeClient} /></LocaleProvider>)

    await userEvent.setup().click(screen.getByRole('button', { name: 'Open Git' }))

    expect(await screen.findByRole('dialog', { name: 'Git and worktrees' })).toBeTruthy()
    expect(screen.getByPlaceholderText('Search Matou branches')).toBeTruthy()
    expect(screen.getByText('3 uncommitted files')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Manage worktrees… 0' })).toBeTruthy()
  })

  it('names the permission control and the model field in English', () => {
    render(<LocaleProvider initialLocale="en"><TerminalHud hud={{
      sessionId: 'session-1', mode: 'agent', permissionMode: 'default', modelStrategy: 'opusplan',
      contextWindowSize: 1_000_000, taskStatus: 'needs-input', startedAt: Date.now()
    }} onPermissionMode={vi.fn()} /></LocaleProvider>)

    expect(screen.getByRole('button', { name: 'Permission mode: Default — click to switch' })).toBeTruthy()
    expect(screen.getByText('Opus Plan (1M context)')).toBeTruthy()
    expect(screen.queryByText('Needs input')).toBeNull()
  })

  it('omits the task error field in English', () => {
    render(<LocaleProvider initialLocale="en"><TerminalHud hud={{
      sessionId: 'session-1', mode: 'agent', modelStrategy: 'opusplan',
      taskStatus: 'error', startedAt: Date.now()
    }} /></LocaleProvider>)

    expect(screen.queryByText('Error')).toBeNull()
    expect(screen.getByText('Opus Plan')).toBeTruthy()
  })
})

function localEnvironment(): SessionEnvironment {
  return { kind: 'local', state: 'ready', path: '/repo', localExecutionContextId: 'local-context' }
}

function missingWorktree(): SessionEnvironment {
  return {
    kind: 'worktree', state: 'missing', path: '/worktree', error: 'path-missing',
    localExecutionContextId: 'local-context', worktreeId: 'worktree-1',
    worktreeExecutionContextId: 'worktree-context'
  }
}

function repositoryStatus(): GitRepositoryStatus {
  return {
    repositoryRoot: '/repo', cwd: '/repo', currentBranch: 'main', defaultBranch: 'main',
    dirty: true, stagedCount: 1, unstagedCount: 1, untrackedCount: 1,
    additions: 4, deletions: 2, ahead: 0, behind: 0, hasRemote: false, canPush: false,
    branches: [{ name: 'main', current: true, commitTimestamp: 1 }],
    worktrees: [{
      path: '/repo', branch: 'main', head: 'abc',
      current: true, main: true, dirty: false, managed: false, sessionCount: 1
    }]
  }
}

function environmentActions(): SessionEnvironmentActions {
  const environment = {
    kind: 'environment' as const, sessionId: 'session-1', activeTarget: 'worktree' as const,
    state: 'ready' as const, path: '/worktree', restartRequired: true
  }
  return {
    open: vi.fn(async () => ({ sessionId: 'session-1', kind: 'local' as const, path: '/repo' })),
    restore: vi.fn(async () => environment),
    locate: vi.fn(async () => environment),
    handoff: vi.fn(async (_sessionId: string, target: 'local' | 'worktree') => ({
      ...environment, activeTarget: target, path: target === 'local' ? '/repo' : '/worktree'
    }))
  }
}

it('localizes agent states, stale detail and all English duration tiers', async () => {
  const now = Date.now()
  render(<LocaleProvider initialLocale="en"><TerminalHud hud={{ sessionId: 'live', mode: 'agent', startedAt: now,
    agents: [
      { id: 'a', label: 'One', source: 'external', state: 'running', startedAt: now - 12000 },
      { id: 'b', label: 'Two', source: 'external', state: 'waiting', startedAt: now - 372000 },
      { id: 'c', label: 'Three', source: 'external', state: 'warning', startedAt: now - 3660000 },
      { id: 'd', label: 'Four', source: 'external', state: 'error', detail: '状态已停止更新' }
    ]
  }} /></LocaleProvider>)
  await userEvent.setup().hover(screen.getByText('⚠ 4 Agents'))
  for (const text of ['Running', 'Waiting', 'Attention', 'Error', '12s', '6m 12s', '1h 01m', 'Status stopped updating']) expect(screen.getByText(text)).toBeTruthy()
})
