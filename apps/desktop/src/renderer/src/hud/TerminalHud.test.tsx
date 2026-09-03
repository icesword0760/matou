// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { GitRepositoryStatus, RpcMethod } from '@matou/contracts'

import { TerminalHud } from './TerminalHud'
import type { GitRequestClient } from './GitControlMenu'
import type { SessionHudView } from '../hierarchy/hierarchy-types'
import type { SessionEnvironmentActions } from './EnvironmentControlMenu'

afterEach(cleanup)

describe('PRD 02 bottom HUD', () => {
  it('renders the reference product Shell field order and hides unavailable data silently', () => {
    const { container } = render(<TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', shell: 'zsh', cwd: '/Users/demo/project',
      gitBranch: 'feature/hud', gitDirty: true, startedAt: Date.now() - 70_000
    }} />)

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
    }} runtimeClient={runtimeClient} />)

    await user.click(screen.getByRole('button', { name: '打开 Git' }))
    expect(await screen.findByRole('dialog', { name: 'Git 与 Worktree' })).toBeTruthy()
    await user.click(await screen.findByRole('button', { name: /feature\/menu/ }))
    expect(request).toHaveBeenCalledWith(
      'git.checkout', expect.objectContaining({ input: expect.objectContaining({
        cwd: '/Users/demo/project', branch: 'feature/menu'
      }) }), { timeoutMs: 120_000 }
    )
  })

  it('lets the Git control consume Escape for its second-level navigation', async () => {
    const user = userEvent.setup()
    const repository = gitStatus()
    const runtimeClient: GitRequestClient = {
      request: async function<T>(): Promise<T> { return repository as T }
    }
    render(<TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', cwd: '/Users/demo/project',
      gitBranch: 'main', gitDirty: false, startedAt: Date.now()
    }} runtimeClient={runtimeClient} />)

    await user.click(screen.getByRole('button', { name: '打开 Git' }))
    await user.click(await screen.findByRole('button', { name: '管理 Worktree… 0' }))
    await user.keyboard('{Escape}')
    expect(screen.getByRole('dialog', { name: 'Git 与 Worktree' })).toBeTruthy()
    expect(screen.getByPlaceholderText('搜索 matou 分支')).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog', { name: 'Git 控制' })).toBeNull()
  })

  it('renders the complete Agent HUD from authoritative session metrics', () => {
    const hud: SessionHudView = {
      sessionId: 'session-1', mode: 'agent', shell: 'zsh', cwd: '/Users/demo/project',
      gitBranch: 'main', gitDirty: false, startedAt: Date.now() - 3_700_000,
      permissionMode: 'acceptEdits', modelStrategy: 'claude-opus-4-6', model: 'Claude Opus 4.6',
      contextPercent: 86, contextWindowSize: 1_000_000, taskStatus: 'running', subagentCount: 2,
      sessionName: 'adaptive-painting-hoare',
      teamRole: 'Leader', teamStatus: 'running',
      usageWindows: [{ label: 'Weekly', percent: 8, resetsAt: Date.now() + 4 * 86_400_000 + 22 * 3_600_000 }],
      configCounts: { instructionFiles: 1, mcpServers: 2, hooks: 11 },
      mcpErrors: ['browser-bridge'],
      toolCounts: [{ name: 'Read', count: 9 }, { name: 'Bash', count: 9 }],
      lastTool: { name: 'WebFetch', target: 'example.com', status: 'error' },
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
    const { container } = render(<TerminalHud hud={hud} />)

    const text = container.querySelector('.status-info')?.textContent ?? ''
    expect(text).toContain('AEOpus 4.6 (1M context)86%Weekly 8%')
    expect(text).not.toContain('adaptive-painting-hoare')
    expect(text).toContain('1 CLAUDE.md2 MCPs11 hooks')
    expect(text).toContain('⚠ browser-bridge')
    expect(container.querySelector('.status-tool-running, .status-last-tool, .status-tool-done')).toBeNull()
    expect(text).not.toContain('WebFetch')
    expect(text).toContain('任务中Agent:2Leader')
    expect(text).toContain('▸完成 HUD 实现(1/2)')
    expect(text).toMatch(/~\/projectmain⏱1h1m$/)
    expect(container.querySelector('.context-ring-fg')?.getAttribute('stroke')).toBe('#f85149')
  })

  it.each([
    [69, '#3fb950'], [70, '#d29922'], [84, '#d29922'], [85, '#f85149'], [130, '#f85149']
  ])('uses the reference product risk color for %s%% context', (contextPercent, color) => {
    const { container } = render(<TerminalHud hud={agent({ contextPercent })}
      />)
    expect(container.querySelector('.context-ring-fg')?.getAttribute('stroke')).toBe(color)
    expect(screen.getByText(`${contextPercent}%`)).toBeTruthy()
  })

  it.each([
    ['running', '任务中'], ['needs-input', '待输入'], ['error', '错误']
  ] as const)('shows the reference product Agent task label for %s', (taskStatus, label) => {
    render(<TerminalHud hud={agent({ taskStatus })} />)
    expect(screen.getByText(label)).toBeTruthy()
  })

  it('closes the permission menu by Escape or outside click', async () => {
    const user = userEvent.setup()
    render(<TerminalHud hud={agent()} onPermissionMode={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    expect(screen.getByRole('menu', { name: '权限模式' })).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu', { name: '权限模式' })).toBeNull()

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('menu', { name: '权限模式' })).toBeNull()
  })

  it('keeps the permission switch interactive while the model remains read-only and live', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn()
    const { rerender } = render(<TerminalHud hud={agent({
      permissionMode: 'default', model: 'Claude Opus 4.6', contextWindowSize: 1_000_000
    })} onPermissionMode={onPermissionMode} />)

    const permission = screen.getByRole('button', { name: /当前权限模式：Default/ })
    expect(permission.textContent).toBe('D')
    await user.click(permission)
    expect(screen.getByRole('menuitem', { name: /^Default/ })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Accept Edits' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Plan Mode' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: 'Bypass Permissions' })).toBeTruthy()
    await user.click(screen.getByRole('menuitem', { name: 'Plan Mode' }))
    expect(onPermissionMode).toHaveBeenCalledWith('session-1', 'plan', false)
    expect(screen.getByText('Opus 4.6 (1M context)').tagName).toBe('SPAN')
    expect(screen.queryByRole('button', { name: /切换模型/ })).toBeNull()

    rerender(<TerminalHud hud={agent({
      permissionMode: 'auto', model: 'Claude Fable 5', contextWindowSize: 1_000_000
    })} onPermissionMode={onPermissionMode} />)
    expect(screen.getByRole('button', { name: /当前权限模式：Auto/ }).textContent).toBe('A')
    expect(screen.getByText('Fable 5 (1M context)')).toBeTruthy()
  })

  it('keeps tool activity out of the compact bottom HUD', () => {
    const { container } = render(<TerminalHud hud={agent({
      sessionName: 'duplicate-card-title',
      lastTool: { name: 'Bash', target: '/Users/demo/project/scripts/verify.sh', status: 'completed' },
      toolCounts: [{ name: 'Bash', count: 5 }, { name: 'Read', count: 2 }, { name: 'Skill', count: 2 }]
    })} onPermissionMode={vi.fn()} />)

    const text = container.querySelector('.status-info')?.textContent ?? ''
    expect(text).not.toContain('duplicate-card-title')
    expect(text).not.toContain('/Users/demo/project/scripts/verify.sh')
    expect(container.querySelector('.status-tool-running, .status-last-tool, .status-tool-done')).toBeNull()
    expect(text).not.toContain('Bash')
    expect(text).not.toContain('Read')
    expect(text).not.toContain('Skill')
  })

  it('closes open agent controls immediately when read-only recovery starts', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn()
    const view = render(<TerminalHud hud={agent({ resumable: true })}
      onPermissionMode={onPermissionMode} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Bypass Permissions' }))
    expect(screen.getByRole('alertdialog')).toBeTruthy()

    view.rerender(<TerminalHud hud={agent({ resumable: true })}
      disabledReason="数据库处于只读恢复模式"
      onPermissionMode={onPermissionMode} />)

    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(screen.getByRole('button', { name: /当前权限模式/ }).hasAttribute('disabled')).toBe(true)
    expect(screen.getByRole('button', { name: /当前权限模式/ }).title).toBe('数据库处于只读恢复模式')
    expect(onPermissionMode).not.toHaveBeenCalled()
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
      onPermissionMode={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: '打开 Git' }))
    expect(await screen.findByRole('dialog', { name: 'Git 与 Worktree' })).toBeTruthy()
    request.mockClear()

    view.rerender(<TerminalHud hud={hud} runtimeClient={runtimeClient}
      disabledReason="数据库处于只读恢复模式"
      onPermissionMode={vi.fn()} />)

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
      onPermissionMode={vi.fn()} />)

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
      onPermissionMode={vi.fn()} />)

    expect(screen.getByRole('button', { name: '打开 Git' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '打开 Git' }).textContent).toBe('Git 不可用')
    expect(screen.getByRole('button', { name: '打开运行环境：待恢复' }).textContent).toBe('待恢复')
  })

  it('keeps recovery actions enabled when only the current Environment blocks normal Session mutations', async () => {
    render(<TerminalHud hud={undefined} sessionId="session-1"
      disabledReason="当前运行环境需要先恢复或交接"
      environment={{
        kind: 'worktree', state: 'missing', path: '/missing', error: 'path-missing',
        localExecutionContextId: 'local-context', worktreeId: 'worktree-1',
        worktreeExecutionContextId: 'worktree-context'
      }}
      git={{ state: 'unavailable', dirty: false }}
      environmentActions={environmentActions()}
      onPermissionMode={vi.fn()} />)

    await userEvent.setup().click(screen.getByRole('button', { name: '打开运行环境：待恢复' }))

    expect(screen.getByRole('button', { name: '恢复原 Worktree' })).toHaveProperty('disabled', false)
    expect(screen.getByRole('button', { name: '定位已移动的 Worktree' })).toHaveProperty('disabled', false)
    expect(screen.getByRole('button', { name: '交接到 Local' })).toHaveProperty('disabled', false)
  })

  it('clears the Git badge when a live Shell leaves its repository', () => {
    render(<TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', shell: 'zsh', cwd: '/outside', startedAt: Date.now()
    }} sessionId="session-1"
      environment={{
        kind: 'local', state: 'ready', path: '/outside', localExecutionContextId: 'local-context'
      }}
      git={{ state: 'ready', branch: 'stale-main', dirty: true }}
      environmentActions={environmentActions()}
      onPermissionMode={vi.fn()} />)

    expect(screen.queryByRole('button', { name: '打开 Git' })).toBeNull()
    expect(screen.queryByText('Git 不可用')).toBeNull()
    expect(screen.getByRole('button', { name: '打开运行环境：Local' })).toBeTruthy()
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
      onPermissionMode={vi.fn()} />)

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
      onPermissionMode={vi.fn()} />)

    const git = screen.getByRole('button', { name: '打开 Git' })
    expect(git.textContent).toBe('Git 不可用')
    expect(git).toHaveProperty('disabled', true)
  })

  it('keeps Git outside narrow-width priority hiding', () => {
    const { container } = render(<div style={{ width: 240 }}><TerminalHud hud={{
      sessionId: 'session-1', mode: 'shell', cwd: '/repo', gitBranch: 'main',
      gitDirty: false, startedAt: Date.now()
    }} onPermissionMode={vi.fn()} /></div>)

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

function gitStatus(): GitRepositoryStatus {
  return {
    repositoryRoot: '/Users/demo/project', cwd: '/Users/demo/project',
    currentBranch: 'main', defaultBranch: 'main', dirty: false,
    stagedCount: 0, unstagedCount: 0, untrackedCount: 0,
    additions: 0, deletions: 0, ahead: 0, behind: 0,
    hasRemote: false, canPush: false,
    branches: [{ name: 'main', current: true, commitTimestamp: 1 }],
    worktrees: [{ path: '/Users/demo/project', branch: 'main', head: 'abc',
      current: true, main: true, dirty: false, managed: false, sessionCount: 1 }]
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
