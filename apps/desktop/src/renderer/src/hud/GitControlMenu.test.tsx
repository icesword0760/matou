// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { GitRepositoryStatus, RpcMethod } from '@matou/contracts'

import { GitControlMenu, type GitRequestClient } from './GitControlMenu'

afterEach(() => {
  cleanup()
  Reflect.deleteProperty(window, 'matouDesktop')
})

describe('GitControlMenu', () => {
  it('filters local branches without issuing a Git mutation', async () => {
    const user = userEvent.setup()
    const { client, request } = clientFor(status())
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    expect(await screen.findByRole('button', { name: /feature\/one/ })).toBeTruthy()
    await user.type(screen.getByPlaceholderText('搜索分支'), 'two')
    expect(screen.queryByRole('button', { name: /feature\/one/ })).toBeNull()
    expect(screen.getByRole('button', { name: /feature\/two/ })).toBeTruthy()
    expect(request.mock.calls.filter(([method]) => method !== 'git.status')).toHaveLength(0)
  })

  it('commits the selected scope and keeps a single operation feedback', async () => {
    const user = userEvent.setup()
    const initial = status({ stagedCount: 1, unstagedCount: 1, additions: 8, deletions: 2, dirty: true })
    const { client, request } = clientFor(initial)
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    await screen.findByRole('dialog', { name: 'Git 与 Worktree' })
    await user.click(screen.getByRole('button', { name: /更改 2/ }))
    await user.type(screen.getByPlaceholderText('提交信息'), 'feat: save changes')
    await user.click(screen.getByRole('button', { name: '提交' }))

    expect(request).toHaveBeenCalledWith('git.commit', expect.objectContaining({
      input: expect.objectContaining({ message: 'feat: save changes', includeUnstaged: true })
    }), { timeoutMs: 120_000 })
    expect(await screen.findByText('提交已完成')).toBeTruthy()
    expect(screen.queryAllByRole('status')).toHaveLength(1)
  })

  it('opens Worktrees in Finder but leaves Session switching to the environment control', async () => {
    const user = userEvent.setup()
    const revealDirectory = vi.fn()
    Object.defineProperty(window, 'matouDesktop', { configurable: true, value: { revealDirectory } })
    const withWorktree = status({
      worktrees: [
        status().worktrees[0]!,
        { path: '/repo-worktrees/feature', branch: 'feature/one', head: 'def', current: false,
          main: false, dirty: false, managed: true, sessionCount: 0, worktreeId: 'worktree-1' }
      ]
    })
    const { client, request } = clientFor(withWorktree)
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1"
      context={{ windowId: 'window-1', sceneId: 'scene-1' }} onClose={vi.fn()} />)

    await screen.findByRole('dialog', { name: 'Git 与 Worktree' })
    await user.click(screen.getByRole('button', { name: /Worktree 2/ }))
    await user.click(screen.getAllByRole('button', { name: 'Finder' })[1]!)
    expect(revealDirectory).toHaveBeenCalledWith('/repo-worktrees/feature')
    expect(screen.queryByRole('button', { name: '进入' })).toBeNull()
    expect(request.mock.calls.some(([method]) => method === 'git.worktree-open')).toBe(false)
  })
})

function clientFor(repositoryStatus: GitRepositoryStatus): {
  client: GitRequestClient
  request: ReturnType<typeof vi.fn>
} {
  const request = vi.fn(async (
    method: RpcMethod, _payload: unknown, _options?: { timeoutMs?: number }
  ) => {
    if (method === 'git.checkout') return { kind: 'switched', status: repositoryStatus }
    return repositoryStatus
  })
  return {
    request,
    client: {
      request: async function<T>(method: RpcMethod, payload: unknown, options?: { timeoutMs?: number }): Promise<T> {
        return await request(method, payload, options) as T
      }
    }
  }
}

function status(patch: Partial<GitRepositoryStatus> = {}): GitRepositoryStatus {
  return {
    repositoryRoot: '/repo', cwd: '/repo', currentBranch: 'main', defaultBranch: 'main',
    dirty: false, stagedCount: 0, unstagedCount: 0, untrackedCount: 0,
    additions: 0, deletions: 0, ahead: 0, behind: 0, hasRemote: true, canPush: false,
    branches: [
      { name: 'main', current: true, commitTimestamp: 3 },
      { name: 'feature/one', current: false, commitTimestamp: 2 },
      { name: 'feature/two', current: false, commitTimestamp: 1 }
    ],
    worktrees: [{ path: '/repo', branch: 'main', head: 'abc', current: true,
      main: true, dirty: false, managed: false, sessionCount: 1 }],
    ...patch
  }
}
