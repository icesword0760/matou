// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { TerminalPane } from './TerminalPane'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: (props: { sessionId: string; visible: boolean; inputDisabled: boolean }) =>
    <div data-testid={`surface-${props.sessionId}`} data-visible={props.visible}
      data-input-disabled={props.inputDisabled}><textarea className="xterm-helper-textarea" aria-label="Terminal input" /></div>
}))

afterEach(cleanup)

describe('Terminal pane', () => {
  it('keeps the Session surface mounted while its Scene is inactive', () => {
    const props = fixture()
    const view = render(<TerminalPane {...props} visible />)
    view.rerender(<TerminalPane {...props} active={false} visible={false} />)

    expect(screen.getByTestId('surface-session-1').dataset.visible).toBe('false')
  })

  it('reasserts the Session focus when its terminal input wins a projection race', () => {
    const onActivate = vi.fn()
    render(<TerminalPane {...fixture()} active={false} onActivate={onActivate} />)

    screen.getByRole('textbox', { name: 'Terminal input' }).focus()
    expect(onActivate).toHaveBeenCalledWith('session-1')
  })

  it('deletes a non-final Session without a dialog', async () => {
    const onDelete = vi.fn()
    const user = userEvent.setup()
    render(<TerminalPane {...fixture()} workspaceSessionCount={2} onDelete={onDelete} />)

    await user.click(screen.getByRole('button', { name: '删除终端：Claude 主会话' }))
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(onDelete).toHaveBeenCalledWith('session-1', false)
  })

  it('matches Kooky by protecting the Workspace final Session', async () => {
    const onDelete = vi.fn()
    const user = userEvent.setup()
    render(<TerminalPane {...fixture()} workspaceSessionCount={1} onDelete={onDelete} />)

    await user.click(screen.getByRole('button', { name: '删除终端：Claude 主会话' }))
    expect(screen.getByRole('alertdialog', { name: '提示' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '我知道了' }))
    expect(onDelete).not.toHaveBeenCalled()
  })

  it('matches the Kooky fork source by showing Fork and Detach together only for a resumable Claude pane', async () => {
    const onFork = vi.fn()
    const onDetach = vi.fn()
    const user = userEvent.setup()
    render(<TerminalPane {...fixture()} resumable onFork={onFork} onDetach={onDetach} />)

    expect(screen.getByRole('button', { name: '从“Claude 主会话”创建子分支' })).toBeTruthy()

    await user.pointer({ keys: '[MouseRight]', target: screen.getByText('Claude 主会话') })

    expect(screen.getByText('⑂ Fork 会话')).toBeTruthy()
    expect(screen.getByText('↗ 独立窗口')).toBeTruthy()
    await user.click(screen.getByText('⑂ Fork 会话'))
    expect(onFork).toHaveBeenCalledWith('session-1')

    await user.pointer({ keys: '[MouseRight]', target: screen.getByText('Claude 主会话') })
    await user.click(screen.getByText('↗ 独立窗口'))
    expect(onDetach).toHaveBeenCalledWith('session-1')
  })

  it('opens the pane actions when the user right-clicks the terminal content area', async () => {
    const user = userEvent.setup()
    render(<TerminalPane {...fixture()} resumable onFork={vi.fn()} onDetach={vi.fn()} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByTestId('surface-session-1') })

    expect(screen.getByRole('menuitem', { name: '⑂ Fork 会话' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: '↗ 独立窗口' })).toBeTruthy()
  })

  it.each([
    ['Shell', { kind: 'shell' as const }, false],
    ['team teammate', { kind: 'agent-team-member' as const }, true]
  ])('does not expose Fork for %s', async (_label, sessionPatch, resumable) => {
    const user = userEvent.setup()
    const props = fixture()
    render(<TerminalPane {...props} session={{ ...props.session, ...sessionPatch }}
      resumable={resumable} onDetach={vi.fn()} onFork={vi.fn()} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByText('Claude 主会话') })

    expect(screen.queryByText('⑂ Fork 会话')).toBeNull()
    expect(screen.queryByRole('button', { name: '从“Claude 主会话”创建子分支' })).toBeNull()
    expect(screen.getByText('↗ 独立窗口')).toBeTruthy()
  })

  it('keeps Fork visible but disabled until the Claude conversation is ready', () => {
    const props = fixture()
    render(<TerminalPane {...props} forkReady={false} onFork={vi.fn()} />)

    const button = screen.getByRole('button', { name: '从“Claude 主会话”创建子分支' })
    expect(button).toHaveProperty('disabled', true)
    expect(button.getAttribute('title')).toContain('完成首轮对话')
  })

  it('confirms before ending a running parent while preserving its children as live work', async () => {
    const user = userEvent.setup()
    const onDelete = vi.fn()
    render(<TerminalPane {...fixture()} workspaceSessionCount={4} workStatus="running"
      childNodes={[childNode('child-1'), childNode('child-2')]} onDelete={onDelete} />)

    await user.click(screen.getByRole('button', { name: '删除终端：Claude 主会话' }))
    expect(screen.getByRole('alertdialog', { name: '结束会话' }).textContent)
      .toContain('正在运行，并有 2 个子会话')
    await user.click(screen.getByRole('button', { name: '结束会话' }))
    expect(onDelete).toHaveBeenCalledWith('session-1', true)
  })

  it('keeps a failed Claude restore usable as Shell with one retry action', async () => {
    const user = userEvent.setup()
    const onRetryRestore = vi.fn()
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'shell', title: 'Shell' }}
      providerRestoreState="failed" restoreError="provider session not found"
      onRetryRestore={onRetryRestore} />)

    expect(screen.getByRole('status').textContent).toContain('Claude Code 恢复失败')
    expect(screen.getByText('provider session not found')).toBeTruthy()
    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '重试恢复' }))
    expect(onRetryRestore).toHaveBeenCalledWith('session-1')
  })

  it('shows an ordinary Shell after a manual Claude exit', () => {
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'shell', title: 'Shell' }}
      providerRestoreState="none" />)

    expect(screen.queryByText('Claude Code 恢复失败')).toBeNull()
    expect(screen.queryByText('Claude Code 已退出')).toBeNull()
    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
  })

  it('keeps a failed Fork as a retryable card without starting a terminal process', async () => {
    const user = userEvent.setup()
    const onRetryFork = vi.fn()
    const onRemoveFailedFork = vi.fn()
    render(<TerminalPane {...fixture()} forkState="failed" forkError="依赖安装失败"
      onRetryFork={onRetryFork} onRemoveFailedFork={onRemoveFailedFork} />)

    expect(screen.getByRole('status').textContent).toContain('分支创建失败')
    expect(screen.getByText('依赖安装失败')).toBeTruthy()
    expect(screen.queryByTestId('surface-session-1')).toBeNull()
    await user.click(screen.getByRole('button', { name: '重试创建分支' }))
    expect(onRetryFork).toHaveBeenCalledWith('session-1')
    await user.click(screen.getByRole('button', { name: '移除失败分支' }))
    expect(onRemoveFailedFork).toHaveBeenCalledWith('session-1')
  })

  it('keeps child navigation on a Shell-shaped parent after Claude exits', async () => {
    const user = userEvent.setup()
    const onOpenChildren = vi.fn()
    const props = fixture()
    render(<TerminalPane {...props} session={{ ...props.session, kind: 'shell', title: 'Shell' }}
      childNodes={[{
        sessionId: 'child-1', sceneId: 'scene-1', parentSessionId: 'session-1',
        currentMode: 'claude-code', workStatus: 'running', providerRestoreState: 'none',
        canFork: true, title: '子会话', cwd: '/tmp', activeChildCount: 0,
        historicalChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
        latestLines: [], lastUserInteractionSeq: 0
      }]} historicalChildCount={1} onOpenChildren={onOpenChildren} />)

    await user.click(screen.getByRole('button', { name: '查看 2 个子会话' }))
    expect(onOpenChildren).toHaveBeenCalledWith('session-1')
  })

  it('keeps the full working path discoverable while prioritizing Git and shared-worktree status', () => {
    const longPath = `/repo/${'nested-directory/'.repeat(16)}project`
    render(<TerminalPane {...fixture()} cwd={longPath}
      git={{ branch: 'feature/dag', dirty: true }} sharedWorkingDirectory />)

    expect(screen.getByTitle(longPath).textContent).toBe(longPath)
    expect(screen.getByText('feature/dag*')).toBeTruthy()
    expect(screen.getByText('共享工作树')).toBeTruthy()
  })
})

function fixture() {
  return {
    session: {
      id: 'session-1', taskId: 'task-1', title: 'Claude 主会话',
      kind: 'claude-code' as const, executionContextId: 'context-1'
    },
    active: true,
    workspaceSessionCount: 2,
    taskName: '修复登录',
    pathValid: true,
    onActivate: vi.fn(),
    onDelete: vi.fn()
  }
}

function childNode(sessionId: string) {
  return {
    sessionId, sceneId: 'scene-1', parentSessionId: 'session-1',
    currentMode: 'shell' as const, workStatus: 'idle' as const,
    providerRestoreState: 'none' as const, canFork: false, title: sessionId,
    cwd: '/tmp', activeChildCount: 0, historicalChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}
