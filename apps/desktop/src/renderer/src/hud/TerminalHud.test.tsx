// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { GitRepositoryStatus, RpcMethod } from '@matou/contracts'

import { TerminalHud } from './TerminalHud'
import type { GitRequestClient } from './GitControlMenu'
import type { SessionHudView } from '../hierarchy/hierarchy-types'

afterEach(cleanup)

describe('PRD 02 bottom HUD', () => {
  it('renders the reference product Shell field order and hides unavailable data silently', () => {
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

    await user.click(screen.getByRole('button', { name: '打开 Git 控制' }))
    expect(await screen.findByRole('dialog', { name: 'Git 控制' })).toBeTruthy()
    await user.click(await screen.findByRole('option', { name: /feature\/menu/ }))
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
    }} runtimeClient={runtimeClient} onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: '打开 Git 控制' }))
    await user.click(await screen.findByRole('button', { name: '管理 Worktree… 0' }))
    await user.keyboard('{Escape}')
    expect(screen.getByRole('dialog', { name: 'Git 控制' })).toBeTruthy()
    expect(screen.getByPlaceholderText('搜索 matou 分支')).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog', { name: 'Git 控制' })).toBeNull()
  })

  it('renders the current reference product Agent order and process fields without hidden metrics', () => {
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
    expect(text).toContain('Accept Edits86%任务中Agent:2Leader')
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
  ])('uses the reference product risk color for %s%% context', (contextPercent, color) => {
    const { container } = render(<TerminalHud hud={agent({ contextPercent })}
      onPermissionMode={vi.fn()} onModel={vi.fn()} />)
    expect(container.querySelector('.context-ring-fg')?.getAttribute('stroke')).toBe(color)
    expect(screen.getByText(`${contextPercent}%`)).toBeTruthy()
  })

  it.each([
    ['running', '任务中'], ['needs-input', '待输入'], ['error', '错误']
  ] as const)('shows the reference product Agent task label for %s', (taskStatus, label) => {
    render(<TerminalHud hud={agent({ taskStatus })} onPermissionMode={vi.fn()} onModel={vi.fn()} />)
    expect(screen.getByText(label)).toBeTruthy()
  })

  it('removes the session model entry and closes the permission menu by Escape or outside click', async () => {
    const user = userEvent.setup()
    render(<TerminalHud hud={agent()} onPermissionMode={vi.fn()} onModel={vi.fn()} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    expect(screen.getByRole('menu', { name: '权限模式' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: '点击切换模型' })).toBeNull()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu', { name: '权限模式' })).toBeNull()

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('menu', { name: '权限模式' })).toBeNull()
  })

  it('switches ordinary permission modes immediately without a session model command', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn()
    const onModel = vi.fn()
    render(<TerminalHud hud={agent()} onPermissionMode={onPermissionMode} onModel={onModel} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Plan Mode' }))
    expect(screen.getByRole('button', { name: /当前权限模式：Plan Mode/ })).toBeTruthy()
    expect(onPermissionMode).toHaveBeenCalledWith('session-1', 'plan', false)

    expect(screen.queryByRole('button', { name: '点击切换模型' })).toBeNull()
    expect(onModel).not.toHaveBeenCalled()
  })

  it('uses reference product confirmation copy across the Bypass boundary and keeps the old mode on cancel', async () => {
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

  it('keeps the old badge and shows reference product failure feedback when a Bypass respawn fails', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn().mockRejectedValue(new Error('进程启动失败'))
    render(<TerminalHud hud={agent({ resumable: true })} onPermissionMode={onPermissionMode} onModel={vi.fn()} />)
    await user.click(screen.getByRole('button', { name: /当前权限模式/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Bypass Permissions' }))
    await user.click(screen.getByRole('button', { name: '确认切换' }))

    expect(await screen.findByText('切换失败：进程启动失败')).toBeTruthy()
    expect(screen.getByRole('button', { name: /当前权限模式：Default/ })).toBeTruthy()
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
