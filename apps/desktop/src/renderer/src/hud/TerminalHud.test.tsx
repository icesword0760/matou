// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
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
    }} runtimeClient={runtimeClient} />)

    await user.click(screen.getByRole('button', { name: '打开 Git 控制' }))
    await user.click(await screen.findByRole('button', { name: '管理 Worktree… 0' }))
    await user.keyboard('{Escape}')
    expect(screen.getByRole('dialog', { name: 'Git 控制' })).toBeTruthy()
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
    expect(text).toContain('Accept EditsOpus 4.6 (1M context)86%Weekly 8%')
    expect(text).not.toContain('adaptive-painting-hoare')
    expect(text).toContain('1 CLAUDE.md2 MCPs11 hooks')
    expect(text).toContain('⚠ browser-bridge')
    expect(text).toContain('✓Read×9')
    expect(text).toContain('✓Bash×9')
    expect(text).toContain('⚠WebFetch: example.com')
    expect(text).toContain('任务中Agent:2Leader')
    expect(text).toContain('Read')
    expect(text).toContain('Edit')
    expect([...container.querySelectorAll('.status-tool-running')].map((node) => node.textContent).join('')).not.toContain('Bash')
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

  it('keeps the permission switch interactive while the model remains read-only and live', async () => {
    const user = userEvent.setup()
    const onPermissionMode = vi.fn()
    const { rerender } = render(<TerminalHud hud={agent({
      permissionMode: 'default', model: 'Claude Opus 4.6', contextWindowSize: 1_000_000
    })} onPermissionMode={onPermissionMode} />)

    await user.click(screen.getByRole('button', { name: /当前权限模式：Default/ }))
    await user.click(screen.getByRole('menuitem', { name: 'Plan Mode' }))
    expect(onPermissionMode).toHaveBeenCalledWith('session-1', 'plan', false)
    expect(screen.getByText('Opus 4.6 (1M context)').tagName).toBe('SPAN')
    expect(screen.queryByRole('button', { name: /切换模型/ })).toBeNull()

    rerender(<TerminalHud hud={agent({
      permissionMode: 'auto', model: 'Claude Fable 5', contextWindowSize: 1_000_000
    })} onPermissionMode={onPermissionMode} />)
    expect(screen.getByRole('button', { name: /当前权限模式：Auto/ })).toBeTruthy()
    expect(screen.getByText('Fable 5 (1M context)')).toBeTruthy()
  })

  it('hides a duplicate session title and the concrete Bash target while retaining its count', () => {
    const { container } = render(<TerminalHud hud={agent({
      sessionName: 'duplicate-card-title',
      lastTool: { name: 'Bash', target: '/Users/demo/project/scripts/verify.sh', status: 'completed' },
      toolCounts: [{ name: 'Bash', count: 5 }]
    })} onPermissionMode={vi.fn()} />)

    const text = container.querySelector('.status-info')?.textContent ?? ''
    expect(text).not.toContain('duplicate-card-title')
    expect(text).not.toContain('/Users/demo/project/scripts/verify.sh')
    expect(text).toContain('Bash×5')
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
