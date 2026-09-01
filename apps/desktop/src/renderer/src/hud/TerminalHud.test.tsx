// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { RpcMethod } from '@matou/contracts'

import { TerminalHud } from './TerminalHud'
import type { GitRequestClient } from './GitControlMenu'
import type { SessionHudView } from '../hierarchy/hierarchy-types'
import type { SessionEnvironmentActions } from './EnvironmentControlMenu'

afterEach(cleanup)

describe('PRD 02 bottom HUD', () => {
  it('renders the Kooky Shell field order and hides unavailable data silently', () => {
    const { container } = render(<TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', shell: 'zsh', cwd: '/Users/demo/project',
      gitBranch: 'feature/hud', gitDirty: true, startedAt: Date.now() - 70_000
    }} onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    expect(container.querySelector('.status-info')?.textContent).toMatch(
      /^zsh~\/projectfeature\/hud\*⏱1m$/
    )
    expect(screen.queryByText(/unknown|N\/A|--/i)).toBeNull()
    expect(container.querySelector('.status-git')?.textContent).toBe('feature/hud*')
  })

  it('opens the Git control from the branch field and switches branches', async () => {
    const user = userEvent.setup()
    const status = {
      repositoryRoot: '/Users/demo/project', cwd: '/Users/demo/project',
      currentBranch: 'main', defaultBranch: 'main', dirty: false,
      stagedCount: 0, unstagedCount: 0, untrackedCount: 0,
      additions: 0, deletions: 0, ahead: 0, behind: 0,
      hasRemote: false, canPush: false,
      branches: [
        { name: 'main', current: true, commitTimestamp: 2 },
        { name: 'feature/menu', current: false, commitTimestamp: 1 }
      ],
      worktrees: [{
        path: '/Users/demo/project', branch: 'main', head: 'abc',
        current: true, main: true, dirty: false, managed: false, sessionCount: 1
      }]
    }
    const request = vi.fn(async (method: string, _payload: unknown, _options?: { timeoutMs?: number }) => method === 'git.checkout'
        ? { kind: 'switched', status: { ...status, currentBranch: 'feature/menu' } }
        : status)
    const runtimeClient: GitRequestClient = {
      request: async function<T>(method: RpcMethod, payload: unknown, options?: { timeoutMs?: number }): Promise<T> {
        return await request(method, payload, options) as T
      }
    }
    render(<TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', cwd: '/Users/demo/project',
      gitBranch: 'main', gitDirty: false, startedAt: Date.now()
    }} runtimeClient={runtimeClient} onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: '打开 Git' }))
    expect(await screen.findByRole('dialog', { name: 'Git 与 Worktree' })).toBeTruthy()
    await user.click(await screen.findByRole('button', { name: /feature\/menu/ }))
    expect(request).toHaveBeenCalledWith(
      'git.checkout', expect.objectContaining({ input: expect.objectContaining({
        cwd: '/Users/demo/project', branch: 'feature/menu'
      }) }), { timeoutMs: 120_000 }
    )
  })

  it('renders the current Kooky Agent order and process fields without hidden metrics', () => {
    const hud: SessionHudView = {
      sessionId: 'session-1', mode: 'agent', shell: 'zsh', cwd: '/Users/demo/project',
      gitBranch: 'main', gitDirty: false, startedAt: Date.now() - 3_700_000,
      permissionMode: 'acceptEdits', modelStrategy: 'claude-opus-4-6', model: 'Claude Opus 4.6',
      contextPercent: 86, taskStatus: 'running', subagentCount: 2,
      teamRole: 'Leader', teamStatus: 'running',
      runningTools: [
        { name: 'Bash', target: 'pnpm test' },
        { name: 'Read', target: '/Users/demo/project/src/very-long-component-name.tsx' },
        { name: 'Edit', target: '/Users/demo/project/src/App.tsx' }
      ],
      todos: [
        { content: '完成 HUD 实现', status: 'in_progress' },
        { content: '运行回归', status: 'completed' }
      ]
    }
    const { container } = render(<TerminalHud hud={hud} onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    const text = container.querySelector('.status-info')?.textContent ?? ''
    expect(text).toContain('Accept EditsOpus86%任务中Agent:2Leader')
    expect(text).toContain('Read')
    expect(text).toContain('Edit')
    expect(text).not.toContain('Bash')
    expect(text).toContain('▸完成 HUD 实现(1/2)')
    expect(text).toMatch(/~\/projectmain⏱1h1m$/)
    expect(text).not.toMatch(/cost|tok\/s|MCP/i)
    expect(container.querySelector('.context-ring-fg')?.getAttribute('stroke')).toBe('#f85149')
  })

  it.each([
    [69, '#3fb950'], [70, '#d29922'], [84, '#d29922'], [85, '#f85149'], [130, '#f85149']
  ])('uses the Kooky risk color for %s%% context', (contextPercent, color) => {
    const { container } = render(<TerminalHud hud={agent({ contextPercent })}
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)
    expect(container.querySelector('.context-ring-fg')?.getAttribute('stroke')).toBe(color)
    expect(screen.getByText(`${contextPercent}%`)).toBeTruthy()
  })

  it.each([
    ['running', '任务中'], ['needs-input', '待输入'], ['error', '错误']
  ] as const)('shows the Kooky Agent task label for %s', (taskStatus, label) => {
    render(<TerminalHud hud={agent({ taskStatus })} onPermissionMode={vi.fn()} onModel={vi.fn()} />)
    expect(screen.getByText(label)).toBeTruthy()
  })

  it('keeps only one Kooky menu open and closes it by Escape or outside click', async () => {
    const user = userEvent.setup()
    render(<TerminalHud hud={agent()} onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    expect(screen.getByRole('menu', { name: '权限模式' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '点击切换模型' }))
    expect(screen.queryByRole('menu', { name: '权限模式' })).toBeNull()
    expect(screen.getByRole('menu', { name: '模型' })).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu', { name: '模型' })).toBeNull()

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('menu', { name: '权限模式' })).toBeNull()
  })

  it('switches ordinary permission modes immediately and optimistically switches models', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn()
    const onModel = vi.fn()
    render(<TerminalHud hud={agent()} onPermissionMode={onPermissionMode} onModel={onModel} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Plan Mode' }))
    expect(screen.getByRole('button', { name: /当前权限模式：Plan Mode/ })).toBeTruthy()
    expect(onPermissionMode).toHaveBeenCalledWith('session-1', 'plan', false)

    await user.click(screen.getByRole('button', { name: '点击切换模型' }))
    await user.click(screen.getByRole('menuitem', { name: 'Claude Sonnet 4.6' }))
    expect(screen.getByRole('button', { name: '点击切换模型' }).textContent).toBe('Sonnet')
    expect(onModel).toHaveBeenCalledWith('session-1', 'claude-sonnet-4-6')
  })

  it('uses Kooky confirmation copy across the Bypass boundary and keeps the old mode on cancel', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn()
    render(<TerminalHud hud={agent({ resumable: true })} onPermissionMode={onPermissionMode} onModel={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Bypass Permissions' }))
    expect(screen.getByRole('alertdialog', { name: '切换到高权限模式' }).textContent).toContain(
      '重启后会自动 resume 恢复会话历史'
    )
    await user.click(screen.getByRole('button', { name: '取消' }))
    expect(screen.getByRole('button', { name: /当前权限模式：Default/ })).toBeTruthy()
    expect(onPermissionMode).not.toHaveBeenCalled()

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Bypass Permissions' }))
    await user.click(screen.getByRole('button', { name: '确认切换' }))
    expect(onPermissionMode).toHaveBeenCalledWith('session-1', 'bypassPermissions', true)
  })

  it('warns that a new conversation starts when there is no resumable identity', async () => {
    const user = userEvent.setup()
    render(<TerminalHud hud={agent({ resumable: false })} onPermissionMode={vi.fn()} onModel={vi.fn()} />)
    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Bypass Permissions' }))
    expect(screen.getByRole('alertdialog').textContent).toContain(
      '当前 Claude 会话还没有生成可恢复的 sessionId'
    )
  })

  it('keeps the old badge and shows Kooky failure feedback when a Bypass respawn fails', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn().mockRejectedValue(new Error('进程启动失败'))
    render(<TerminalHud hud={agent({ resumable: true })} onPermissionMode={onPermissionMode} onModel={vi.fn()} />)
    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Bypass Permissions' }))
    await user.click(screen.getByRole('button', { name: '确认切换' }))

    expect(await screen.findByText('切换失败：进程启动失败')).toBeTruthy()
    expect(screen.getByRole('button', { name: /当前权限模式：Default/ })).toBeTruthy()
  })

  it('closes open agent controls immediately when read-only recovery starts', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn()
    const onModel = vi.fn()
    const view = render(<TerminalHud hud={agent({ resumable: true })}
      onPermissionMode={onPermissionMode} onModel={onModel} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Bypass Permissions' }))
    expect(screen.getByRole('alertdialog')).toBeTruthy()

    view.rerender(<TerminalHud hud={agent({ resumable: true })}
      disabledReason="数据库处于只读恢复模式"
      onPermissionMode={onPermissionMode} onModel={onModel} />)

    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(screen.getByRole('button', { name: /当前权限模式/ }).hasAttribute('disabled')).toBe(true)
    expect(screen.getByRole('button', { name: /当前权限模式/ }).title).toBe('数据库处于只读恢复模式')
    expect(onPermissionMode).not.toHaveBeenCalled()
    expect(onModel).not.toHaveBeenCalled()
  })

  it('closes an open repository control immediately when read-only recovery starts', async () => {
    const user = userEvent.setup()
    const request = vi.fn(async (
      _method: RpcMethod, _payload: unknown, _options?: { timeoutMs?: number }
    ) => ({
      repositoryRoot: '/tmp/project', cwd: '/tmp/project', currentBranch: 'main',
      defaultBranch: 'main', dirty: false, stagedCount: 0, unstagedCount: 0,
      untrackedCount: 0, additions: 0, deletions: 0, ahead: 0, behind: 0,
      hasRemote: false, canPush: false, branches: [], worktrees: []
    }))
    const runtimeClient: GitRequestClient = {
      request: async function<T>(method: RpcMethod, payload: unknown, options?: { timeoutMs?: number }): Promise<T> {
        return await request(method, payload, options) as T
      }
    }
    const hud: SessionHudView = {
      sessionId: 'session-1', mode: 'shell', cwd: '/tmp/project', gitBranch: 'main', startedAt: Date.now()
    }
    const view = render(<TerminalHud hud={hud} runtimeClient={runtimeClient}
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: '打开 Git' }))
    expect(await screen.findByRole('dialog', { name: 'Git 与 Worktree' })).toBeTruthy()
    request.mockClear()

    view.rerender(<TerminalHud hud={hud} runtimeClient={runtimeClient}
      disabledReason="数据库处于只读恢复模式"
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    expect(screen.queryByRole('dialog', { name: 'Git 与 Worktree' })).toBeNull()
    expect(screen.getByRole('button', { name: '打开 Git' }).hasAttribute('disabled')).toBe(true)
    expect(request).not.toHaveBeenCalled()
  })

  it('keeps Environment and detached Git as separate right-side controls without a live HUD', async () => {
    const actions = environmentActions()
    render(<TerminalHud hud={undefined} sessionId="session-1"
      environment={{
        kind: 'local', state: 'ready', path: '/repo', localExecutionContextId: 'local-context'
      }}
      git={{ state: 'ready', detachedHead: '1234567890abcdef', dirty: true }}
      environmentActions={actions} runtimeClient={{ request: vi.fn() }}
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    expect(screen.getByRole('button', { name: '打开 Git' }).textContent).toBe('HEAD 1234567*')
    expect(screen.getByRole('button', { name: '打开运行环境：Local' }).textContent).toBe('Local')
    await userEvent.setup().click(screen.getByRole('button', { name: '打开运行环境：Local' }))
    expect(screen.getByRole('dialog', { name: '运行环境' })).toBeTruthy()
  })

  it('shows unavailable Git independently instead of hiding it behind an environment error', () => {
    render(<TerminalHud hud={undefined} sessionId="session-1"
      environment={{
        kind: 'worktree', state: 'missing', path: '/missing', error: 'path-missing',
        localExecutionContextId: 'local-context', worktreeId: 'worktree-1',
        worktreeExecutionContextId: 'worktree-context'
      }}
      git={{ state: 'unavailable', dirty: false }}
      environmentActions={environmentActions()}
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    expect(screen.getByRole('button', { name: '打开 Git' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '打开 Git' }).textContent).toBe('Git 不可用')
    expect(screen.getByRole('button', { name: '打开运行环境：待恢复' }).textContent).toBe('待恢复')
  })

  it('opens Git from the authoritative Environment path instead of a stale HUD cwd', async () => {
    const request = vi.fn(async (
      _method: RpcMethod, _payload: unknown, _options?: { timeoutMs?: number }
    ) => ({
      repositoryRoot: '/authoritative/worktree', cwd: '/authoritative/worktree',
      currentBranch: 'main', defaultBranch: 'main', dirty: false,
      stagedCount: 0, unstagedCount: 0, untrackedCount: 0,
      additions: 0, deletions: 0, ahead: 0, behind: 0,
      hasRemote: false, canPush: false, branches: [], worktrees: []
    }))
    const runtimeClient: GitRequestClient = {
      request: async function<T>(method: RpcMethod, payload: unknown, options?: { timeoutMs?: number }): Promise<T> {
        return await request(method, payload, options) as T
      }
    }
    render(<TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', cwd: '/stale/local',
      gitBranch: 'main', startedAt: Date.now()
    }} environment={{
      kind: 'worktree', state: 'ready', path: '/authoritative/worktree',
      localExecutionContextId: 'local-context', worktreeId: 'worktree-1',
      worktreeExecutionContextId: 'worktree-context'
    }} git={{ state: 'ready', branch: 'main', dirty: false }}
      environmentActions={environmentActions()} runtimeClient={runtimeClient}
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '打开 Git' }))
    expect(await screen.findByRole('dialog', { name: 'Git 与 Worktree' })).toBeTruthy()
    expect(request).toHaveBeenCalledWith(
      'git.status', expect.objectContaining({ input: expect.objectContaining({
        cwd: '/authoritative/worktree'
      }) }), { timeoutMs: 120_000 }
    )
    expect(request.mock.calls.some(([, payload]) =>
      (payload as { input?: { cwd?: string } }).input?.cwd === '/stale/local'
    )).toBe(false)
  })

  it('still renders an explicit unavailable Git projection when Environment has no Git state', () => {
    render(<TerminalHud hud={undefined} sessionId="session-1"
      environment={{
        kind: 'local', state: 'ready', path: '/repo', localExecutionContextId: 'local-context'
      }} environmentActions={environmentActions()}
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    const git = screen.getByRole('button', { name: '打开 Git' })
    expect(git.textContent).toBe('Git 不可用')
    expect(git).toHaveProperty('disabled', true)
  })

  it('keeps Git outside narrow-width priority hiding', () => {
    const { container } = render(<div style={{ width: 240 }}><TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', cwd: '/repo', gitBranch: 'main',
      gitDirty: false, startedAt: Date.now()
    }} onPermissionMode={vi.fn()} onModel={vi.fn()} /></div>)

    const git = screen.getByRole('button', { name: '打开 Git' })
    expect(git.className).not.toMatch(/status-priority-/)
    expect(container.querySelector('.status-git')).toBe(git)
  })
})

function agent(patch: Partial<SessionHudView> = {}): SessionHudView {
  return {
    sessionId: 'session-1', mode: 'agent', permissionMode: 'default',
    modelStrategy: 'opusplan', model: 'Claude Opus 4.6', startedAt: Date.now(),
    ...patch
  }
}

function environmentActions(): SessionEnvironmentActions {
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
    handoff: vi.fn(async (_sessionId, target) => ({
      kind: 'environment' as const, sessionId: 'session-1', activeTarget: target,
      state: 'ready' as const, path: target === 'local' ? '/repo' : '/worktree', restartRequired: true
    }))
  }
}
