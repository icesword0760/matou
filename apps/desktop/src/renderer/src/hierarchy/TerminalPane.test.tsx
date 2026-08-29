// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { TerminalPane } from './TerminalPane'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: (props: { sessionId: string; visible: boolean; inputDisabled: boolean }) =>
    <div data-testid={`surface-${props.sessionId}`} data-visible={props.visible}
      data-input-disabled={props.inputDisabled} />
}))

afterEach(cleanup)

describe('Terminal pane', () => {
  it('keeps the Session surface mounted while its Scene is inactive', () => {
    const props = fixture()
    const view = render(<TerminalPane {...props} visible />)
    view.rerender(<TerminalPane {...props} active={false} visible={false} />)

    expect(screen.getByTestId('surface-session-1').dataset.visible).toBe('false')
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
    ['identity-less Claude', { kind: 'claude-code' as const }, false],
    ['team teammate', { kind: 'agent-team-member' as const }, true]
  ])('does not expose Fork for %s', async (_label, sessionPatch, resumable) => {
    const user = userEvent.setup()
    const props = fixture()
    render(<TerminalPane {...props} session={{ ...props.session, ...sessionPatch }}
      resumable={resumable} onDetach={vi.fn()} onFork={vi.fn()} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByText('Claude 主会话') })

    expect(screen.queryByText('⑂ Fork 会话')).toBeNull()
    expect(screen.getByText('↗ 独立窗口')).toBeTruthy()
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
