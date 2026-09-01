// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { SessionEnvironment } from '@matou/domain'

import { EnvironmentControlMenu, type SessionEnvironmentActions } from './EnvironmentControlMenu'

afterEach(() => {
  cleanup()
  Reflect.deleteProperty(window, 'matouDesktop')
})

describe('EnvironmentControlMenu', () => {
  it('opens the authoritative Local directory and hands back to the owned Worktree', async () => {
    const revealDirectory = vi.fn()
    const openDirectoryInTerminal = vi.fn()
    Object.defineProperty(window, 'matouDesktop', {
      configurable: true,
      value: { revealDirectory, openDirectoryInTerminal }
    })
    const actions = environmentActions()
    const onClose = vi.fn()
    render(<EnvironmentControlMenu sessionId="session-1" environment={localEnvironment()}
      hasOwnedWorktree actions={actions} onClose={onClose} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '在 Finder 中显示' }))
    expect(actions.open).toHaveBeenCalledWith('session-1')
    expect(revealDirectory).toHaveBeenCalledWith('/repo')
    expect(onClose).toHaveBeenCalled()

    onClose.mockClear()
    await user.click(screen.getByRole('button', { name: '在系统终端中打开' }))
    expect(openDirectoryInTerminal).toHaveBeenCalledWith('/repo')
    expect(onClose).toHaveBeenCalled()

    onClose.mockClear()
    await user.click(screen.getByRole('button', { name: '交接到自有 Worktree' }))
    expect(actions.handoff).toHaveBeenCalledWith('session-1', 'worktree')
    expect(onClose).toHaveBeenCalled()
  })

  it('keeps a missing Worktree actionable and translates an identity mismatch for the user', async () => {
    const actions = environmentActions()
    actions.restore = vi.fn(async () => ({
      kind: 'rejected' as const, sessionId: 'session-1', reason: 'wrong-branch' as const
    }))
    const onClose = vi.fn()
    render(<EnvironmentControlMenu sessionId="session-1" environment={missingWorktree()}
      hasOwnedWorktree actions={actions} onClose={onClose} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '恢复原 Worktree' }))

    expect(actions.restore).toHaveBeenCalledWith('session-1')
    expect(screen.getByText('所选 Worktree 的分支与原会话不一致')).toBeTruthy()
    expect(onClose).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: '定位已移动的 Worktree' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '交接到 Local' })).toBeTruthy()
  })

  it('uses the system directory picker before locating a moved Worktree', async () => {
    Object.defineProperty(window, 'matouDesktop', {
      configurable: true,
      value: { selectSessionEnvironmentDirectory: vi.fn(async () => '/moved/worktree') }
    })
    const actions = environmentActions()
    render(<EnvironmentControlMenu sessionId="session-1" environment={missingWorktree()}
      hasOwnedWorktree actions={actions} onClose={vi.fn()} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '定位已移动的 Worktree' }))

    expect(actions.locate).toHaveBeenCalledWith('session-1', '/moved/worktree')
  })
})

function environmentActions(): SessionEnvironmentActions & Record<string, ReturnType<typeof vi.fn>> {
  return {
    open: vi.fn(async () => ({ sessionId: 'session-1', kind: 'local' as const, path: '/repo' })),
    restore: vi.fn(async () => ({
      kind: 'environment' as const, sessionId: 'session-1', activeTarget: 'worktree' as const,
      state: 'ready' as const, path: '/worktree', restartRequired: true
    })),
    locate: vi.fn(async () => ({
      kind: 'environment' as const, sessionId: 'session-1', activeTarget: 'worktree' as const,
      state: 'ready' as const, path: '/worktree', restartRequired: true
    })),
    handoff: vi.fn(async (_sessionId: string, target: 'local' | 'worktree') => ({
      kind: 'environment' as const, sessionId: 'session-1', activeTarget: target,
      state: 'ready' as const, path: target === 'local' ? '/repo' : '/worktree', restartRequired: true
    }))
  }
}

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
