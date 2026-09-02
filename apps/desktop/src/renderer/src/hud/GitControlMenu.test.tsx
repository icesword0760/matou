// @vitest-environment jsdom
import { cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { GitRepositoryStatus, RpcMethod } from '@matou/contracts'

import { GitControlMenu, type GitRequestClient } from './GitControlMenu'

afterEach(() => {
  cleanup()
  Reflect.deleteProperty(window, 'matouDesktop')
})

describe('GitControlMenu', () => {
  it('opens directly as the compact branch picker with secondary Git actions', async () => {
    render(<GitControlMenu client={clientFor(status()).client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    const dialog = await screen.findByRole('dialog', { name: 'Git 控制' })
    const search = within(dialog).getByPlaceholderText('搜索 matou 分支')
    expect(search).toBe(document.activeElement)
    expect(search.closest('label')?.querySelector('svg')?.dataset.icon).toBe('search')
    expect(within(dialog).getByRole('option', { name: /main/ }).querySelector('svg')?.dataset.icon).toBe('git-branch')
    expect(within(dialog).queryByRole('navigation')).toBeNull()
    expect(within(dialog).queryByRole('button', { name: '关闭 Git 菜单' })).toBeNull()
    expect(within(dialog).getByRole('button', { name: '创建并检出新分支…' })).toBeTruthy()
    const worktreeAction = within(dialog).getByRole('button', { name: '管理 Worktree… 0' })
    expect(worktreeAction).toBeTruthy()
    expect(worktreeAction.querySelector('svg')?.dataset.icon).toBe('network')
    expect(within(worktreeAction).queryByText('0')).toBeNull()
    expect(within(dialog).getByRole('button', { name: '提交与推送…' })
      .querySelector('svg')?.dataset.icon).toBe('git-commit-horizontal')
  })

  it('filters local branches without issuing a Git mutation', async () => {
    const user = userEvent.setup()
    const { client, request } = clientFor(status())
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    expect(await screen.findByRole('option', { name: /feature\/one/ })).toBeTruthy()
    await user.type(screen.getByPlaceholderText('搜索 matou 分支'), 'two')
    expect(screen.queryByRole('option', { name: /feature\/one/ })).toBeNull()
    expect(screen.getByRole('option', { name: /feature\/two/ })).toBeTruthy()
    expect(request.mock.calls.filter(([method]) => method !== 'git.status')).toHaveLength(0)
  })

  it('switches the keyboard-selected branch with ArrowDown and Enter', async () => {
    const user = userEvent.setup()
    const { client, request } = clientFor(status())
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    const search = await screen.findByPlaceholderText('搜索 matou 分支')
    await user.type(search, '{ArrowDown}{Enter}')

    expect(request).toHaveBeenCalledWith('git.checkout', expect.objectContaining({
      input: expect.objectContaining({ branch: 'feature/one' })
    }), { timeoutMs: 120_000 })
  })

  it('creates a branch from the inline second-level view', async () => {
    const user = userEvent.setup()
    const { client, request } = clientFor(status())
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    await user.click(await screen.findByRole('button', { name: '创建并检出新分支…' }))
    expect(screen.getByText((_, element) => element?.classList.contains('git-base-row') === true &&
      element.textContent === '基于当前分支 main')).toBeTruthy()
    await user.type(screen.getByPlaceholderText('例如 feature/improve-git-menu'), 'feature/compact-menu')
    await user.click(screen.getByRole('button', { name: '创建并检出' }))

    expect(request).toHaveBeenCalledWith('git.create-branch', expect.objectContaining({
      input: expect.objectContaining({ branch: 'feature/compact-menu' })
    }), { timeoutMs: 120_000 })
  })

  it('commits the selected scope and keeps a single operation feedback', async () => {
    const user = userEvent.setup()
    const initial = status({ stagedCount: 1, unstagedCount: 1, additions: 8, deletions: 2, dirty: true })
    const { client, request } = clientFor(initial)
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    await screen.findByRole('dialog', { name: 'Git 控制' })
    await user.click(screen.getByRole('button', { name: '提交与推送…' }))
    await user.type(screen.getByPlaceholderText('提交信息（留空将自动生成）…'), 'feat: save changes')
    await user.click(screen.getByRole('button', { name: '提交' }))

    expect(request).toHaveBeenCalledWith('git.commit', expect.objectContaining({
      input: expect.objectContaining({ message: 'feat: save changes', includeUnstaged: true })
    }), { timeoutMs: 120_000 })
    expect(await screen.findByText('提交已完成')).toBeTruthy()
    expect(screen.queryAllByRole('status')).toHaveLength(1)
  })

  it('explains why commit and push actions are disabled for a clean synchronized branch', async () => {
    const user = userEvent.setup()
    render(<GitControlMenu client={clientFor(status()).client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    await user.click(await screen.findByRole('button', { name: '提交与推送…' }))
    expect(screen.getByRole('button', { name: /^提交$/ }).getAttribute('title'))
      .toBe('当前没有可提交的更改')
    expect(screen.getByRole('button', { name: '提交并推送' }).getAttribute('title'))
      .toBe('当前没有可提交的更改')
    expect(screen.getByRole('button', { name: /^推送$/ }).getAttribute('title'))
      .toBe('当前没有待推送的提交')
  })

  it('generates a deterministic commit message when the field is left blank', async () => {
    const user = userEvent.setup()
    const initial = status({ stagedCount: 1, unstagedCount: 1, untrackedCount: 1, dirty: true })
    const { client, request } = clientFor(initial)
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    await user.click(await screen.findByRole('button', { name: '提交与推送…' }))
    await user.click(screen.getByRole('button', { name: '提交' }))

    expect(request).toHaveBeenCalledWith('git.commit', expect.objectContaining({
      input: expect.objectContaining({ message: 'chore: update 3 files', includeUnstaged: true })
    }), { timeoutMs: 120_000 })
  })

  it('commits before pushing when the combined action is selected', async () => {
    const user = userEvent.setup()
    const initial = status({ stagedCount: 1, dirty: true, hasRemote: true })
    const { client, request } = clientFor(initial)
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    await user.click(await screen.findByRole('button', { name: '提交与推送…' }))
    await user.type(screen.getByPlaceholderText('提交信息（留空将自动生成）…'), 'feat: ship control')
    await user.click(screen.getByRole('button', { name: '提交并推送' }))

    const mutations = request.mock.calls.map(([method]) => method)
      .filter((method) => method === 'git.commit' || method === 'git.push')
    expect(mutations).toEqual(['git.commit', 'git.push'])
    expect(await screen.findByText('提交与推送已完成')).toBeTruthy()
  })

  it('creates and removes managed Worktrees from the compact manager', async () => {
    const user = userEvent.setup()
    const withWorktree = status({
      worktrees: [
        status().worktrees[0]!,
        { path: '/repo-worktrees/old', branch: 'feature/old', head: 'def', current: false,
          main: false, dirty: false, managed: true, sessionCount: 0, worktreeId: 'worktree-old' }
      ]
    })
    const { client, request } = clientFor(withWorktree)
    render(<GitControlMenu client={client} cwd="/repo" sessionId="session-1" onClose={vi.fn()} />)

    await user.click(await screen.findByRole('button', { name: '管理 Worktree… 1' }))
    await user.click(screen.getByRole('button', { name: '创建新 Worktree…' }))
    await user.type(screen.getByPlaceholderText('例如 feature/new-worktree'), 'feature/fresh')
    await user.click(screen.getByRole('button', { name: '创建' }))
    expect(request).toHaveBeenCalledWith('git.worktree-create', expect.objectContaining({
      input: expect.objectContaining({ branch: 'feature/fresh', baseRef: 'main' })
    }), { timeoutMs: 120_000 })

    await user.click(screen.getByRole('button', { name: 'feature/old 更多操作' }))
    await user.click(screen.getByRole('button', { name: '移除 Worktree' }))
    expect(request).toHaveBeenCalledWith('git.worktree-remove', expect.objectContaining({
      input: expect.objectContaining({ worktreeId: 'worktree-old' })
    }), { timeoutMs: 120_000 })
  })

  it('opens a Worktree in the current canvas and exposes Finder separately', async () => {
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

    await screen.findByRole('dialog', { name: 'Git 控制' })
    await user.click(screen.getByRole('button', { name: '管理 Worktree… 1' }))
    await user.click(screen.getByRole('button', { name: 'feature/one 更多操作' }))
    await user.click(screen.getByRole('button', { name: '在 Finder 中显示' }))
    expect(revealDirectory).toHaveBeenCalledWith('/repo-worktrees/feature')
    expect(screen.queryByRole('button', { name: '进入' })).toBeNull()
    expect(request.mock.calls.some(([method]) => method === 'git.worktree-open')).toBe(false)
  })

  it('returns from a second-level view on the first Escape and closes on the second', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    render(<GitControlMenu client={clientFor(status()).client} cwd="/repo" sessionId="session-1" onClose={onClose} />)

    await user.click(await screen.findByRole('button', { name: '管理 Worktree… 0' }))
    expect(screen.getByText('Worktree')).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.getByPlaceholderText('搜索 matou 分支')).toBeTruthy()
    expect(onClose).not.toHaveBeenCalled()
    await user.keyboard('{Escape}')
    expect(onClose).toHaveBeenCalledTimes(1)
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
