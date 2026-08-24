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
