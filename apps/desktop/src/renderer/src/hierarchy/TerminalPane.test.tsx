// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { SessionEnvironment } from '@matou/domain'

import { TerminalPane } from './TerminalPane'
import { foregroundTerminalModels } from '../terminal/terminal-model-cache'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: (props: {
    sessionId: string; visible: boolean; inputDisabled: boolean; spawnRevision?: number
    readOnly?: boolean
    onStatusChange?(status: string): void; onRuntimeError?(message: string): void
    onVisualReady?(): void
    onUserInput?(): void
    onStorageFault?(fault: {
      type: 'terminal.storage-fault'; protocolVersion: 1; sessionId: string; sequence: number
      code: 'STORAGE_WRITE_FAILED'; message: string; retainedBytes: number
    }): void
  }) =>
    <div data-testid={`surface-${props.sessionId}`} data-visible={props.visible}
      data-input-disabled={props.inputDisabled} data-read-only={props.readOnly}
      data-spawn-revision={props.spawnRevision}>
      <textarea className="xterm-helper-textarea" aria-label="Terminal input"
        onInput={() => props.onUserInput?.()} />
      <button type="button" aria-label="触发启动失败" onClick={() => {
        props.onRuntimeError?.('spawn ENOENT: /missing/SHELL')
        props.onStatusChange?.('error')
      }} />
      <button type="button" aria-label="触发终端首帧" onClick={() => props.onVisualReady?.()} />
      <button type="button" aria-label="触发存储异常" onClick={() => props.onStorageFault?.({
        type: 'terminal.storage-fault', protocolVersion: 1, sessionId: props.sessionId,
        sequence: 1, code: 'STORAGE_WRITE_FAILED', message: 'disk offline', retainedBytes: 128
      })} />
    </div>
}))

afterEach(() => {
  cleanup()
  foregroundTerminalModels.clear()
})

describe('Terminal pane', () => {
  it('keeps every Session in the foreground list bound even when a card scrolls offscreen', () => {
    const props = fixture()
    const view = render(<TerminalPane {...props} visible />)
    view.rerender(<TerminalPane {...props} active={false} visible={false} foreground />)

    expect(screen.getByTestId('surface-session-1').dataset.visible).toBe('false')
  })

  it('releases the terminal surface when its list moves to the background', () => {
    render(<TerminalPane {...fixture()} active={false} visible={false} foreground={false} />)

    expect(screen.queryByTestId('surface-session-1')).toBeNull()
    expect(screen.getByTestId('background-session-session-1')).toBeTruthy()
  })

  it.each(['queued', 'restoring'] as const)('covers the whole card while recovery is %s', (recoveryState) => {
    render(<TerminalPane {...fixture()} recoveryState={recoveryState} />)

    const pane = screen.getByTestId('terminal-pane')
    expect(pane.getAttribute('aria-busy')).toBe('true')
    expect(screen.getByRole('status', { name: '正在恢复终端：Claude 主会话' })).toBeTruthy()
    const water = screen.getByTestId('session-recovery-water')
    expect(water.querySelector('canvas')).toBeTruthy()
    expect(water.querySelector('.session-recovery-water__skeleton')).toBeNull()
    expect(water.querySelector('.session-recovery-water__glint')).toBeNull()
    expect(screen.queryByTestId('session-recovery-dialog')).toBeNull()
    expect(screen.getByText('恢复中')).toBeTruthy()
    expect(screen.queryByTestId('surface-session-1')).toBeNull()
  })

  it('keeps the recovery water visible until the restored terminal paints its first frame', () => {
    const props = fixture()
    const view = render(<TerminalPane {...props} recoveryState="restoring" />)

    view.rerender(<TerminalPane {...props} recoveryState="ready" />)

    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
    expect(screen.getByTestId('session-recovery-water')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: '触发终端首帧' }))
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
  })

  it('covers a recovered card that first mounts after Runtime already reported ready', () => {
    render(<TerminalPane {...fixture()} recoveryState="ready" />)

    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
    expect(screen.getByTestId('session-recovery-water')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: '触发终端首帧' }))
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
  })

  it('covers a newly created terminal with loading water until its first frame', () => {
    render(<TerminalPane {...fixture()} />)

    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
    expect(screen.getByRole('status', { name: '正在加载终端：Claude 主会话' })).toBeTruthy()
    expect(screen.getByText('加载中')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: '触发终端首帧' }))
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
  })

  it('covers an activated content-heavy Session until its resized terminal frame paints', () => {
    const props = fixture()
    const view = render(<TerminalPane {...props} active={false} visible />)
    fireEvent.click(screen.getByRole('button', { name: '触发终端首帧' }))
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()

    view.rerender(<TerminalPane {...props} active visible />)

    expect(screen.getByRole('status', { name: '正在加载终端：Claude 主会话' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: '触发终端首帧' }))
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
  })

  it('shows a cached Session immediately when returning to it', () => {
    foregroundTerminalModels.acquire('session-1', () => ({ dispose: vi.fn() }))
    foregroundTerminalModels.release('session-1')
    const props = fixture()
    const view = render(<TerminalPane {...props} active={false} visible />)

    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
    view.rerender(<TerminalPane {...props} active visible />)
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
  })

  it('reveals a startup failure that arrives before the recovered terminal paints', () => {
    render(<TerminalPane {...fixture()} recoveryState="ready" />)
    expect(screen.getByTestId('session-recovery-water')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: '触发启动失败' }))

    expect(screen.getByText('会话启动失败')).toBeTruthy()
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
  })

  it('isolates a failed card and lets the user retry only that recovery', async () => {
    const onRetryRecovery = vi.fn()
    render(<TerminalPane {...fixture()} recoveryState="failed" recoveryError="进程恢复失败"
      onRetryRecovery={onRetryRecovery} />)

    expect(screen.getByRole('status', { name: '终端恢复失败：Claude 主会话' }).textContent)
      .toContain('进程恢复失败')
    expect(screen.getByTestId('session-recovery-dialog')).toBeTruthy()
    expect(screen.queryByTestId('session-recovery-water')).toBeNull()
    expect(screen.queryByTestId('surface-session-1')).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: '重试恢复终端：Claude 主会话' }))
    expect(onRetryRecovery).toHaveBeenCalledWith('session-1')
  })

  it('keeps Worktree repair actions above a concurrent recovery failure', async () => {
    const restore = vi.fn()
    render(<TerminalPane {...fixture()} recoveryState="failed" recoveryError="启动未完成"
      environment={worktreeEnvironment('missing')} onRestoreEnvironment={restore}
      onLocateEnvironment={vi.fn()} onHandoffEnvironment={vi.fn()} />)

    expect(screen.queryByRole('status', { name: '终端恢复失败：Claude 主会话' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: '恢复 Worktree' }))
    expect(restore).toHaveBeenCalledWith('session-1')
  })

  it('keeps storage repair actions above a recovery failure after a reconnect race', async () => {
    const props = fixture()
    const view = render(<TerminalPane {...props} />)
    await userEvent.setup().click(screen.getByRole('button', { name: '触发存储异常' }))

    view.rerender(<TerminalPane {...props} recoveryState="failed" recoveryError="重连恢复失败" />)

    expect(screen.getByRole('status', { name: '终端记录写入异常：Claude 主会话' })).toBeTruthy()
    expect(screen.getByRole('button', { name: '重试写入' })).toBeTruthy()
    expect(screen.queryByRole('status', { name: '终端恢复失败：Claude 主会话' })).toBeNull()
  })

  it('keeps durable Fork progress authoritative while generic recovery is still restoring', () => {
    render(<TerminalPane {...fixture()} recoveryState="restoring" forkProgress={{
      operationId: 'operation-1', sessionId: 'session-1', submissionKey: 'submission-1',
      stage: 'restoring-provider', completedSteps: 3, totalSteps: 5, attempt: 1
    }} />)

    expect(screen.getByRole('status', { name: '正在创建分支：正在恢复智能体会话' })).toBeTruthy()
    expect(screen.queryByRole('status', { name: '正在恢复终端：Claude 主会话' })).toBeNull()
  })

  it('keeps durable Fork retry actions visible when both authorities report failure', async () => {
    const retryFork = vi.fn()
    render(<TerminalPane {...fixture()} recoveryState="failed" recoveryError="restore failed"
      forkState="failed" forkError="setup failed" onRetryFork={retryFork}
      onRemoveBranch={vi.fn()} />)

    expect(screen.queryByRole('status', { name: '终端恢复失败：Claude 主会话' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: '重试创建分支' }))
    expect(retryFork).toHaveBeenCalledWith('session-1')
  })

  it('keeps programmatic terminal focus from stealing the active Session', async () => {
    const onActivate = vi.fn()
    render(<TerminalPane {...fixture()} active={false} onActivate={onActivate} />)

    const input = screen.getByRole('textbox', { name: 'Terminal input' })
    input.focus()
    expect(onActivate).not.toHaveBeenCalled()

    await userEvent.setup().click(input)
    expect(onActivate).toHaveBeenCalledWith('session-1')
  })


  it('matches the reference product fork source by showing Fork and Detach together only for a resumable Claude pane', async () => {
    const onFork = vi.fn()
    const onDetach = vi.fn()
    const user = userEvent.setup()
    render(<TerminalPane {...fixture()} resumable onFork={onFork} onForkPeer={onFork}
      onDetach={onDetach} />)

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

  it('matches the approved card header hierarchy and keeps branch actions on the right', async () => {
    const onFork = vi.fn()
    const onForkSibling = vi.fn()
    const children = [
      { ...childNode('child-1'), workStatus: 'running' as const },
      childNode('child-2')
    ]
    render(<TerminalPane {...fixture()} resumable git={{ state: 'ready', branch: 'feat/notification', dirty: false }}
      childNodes={children} onOpenChildren={vi.fn()} onLoadSession={vi.fn()}
      onFork={onFork} onForkSibling={onForkSibling} />)

    const header = screen.getByRole('banner')
    expect(header.textContent).toContain('Claude 主会话')
    expect(header.textContent).toContain('feat/notification')
    expect(header.textContent).toContain('2 分支 · 1 运行中')
    expect(header.querySelector('.pane-header-content .child-session-badge')).toBeNull()
    const actions = header.querySelector('.terminal-pane-actions')
    expect(actions?.querySelector('.child-session-badge')).not.toBeNull()
    expect(actions?.firstElementChild?.classList.contains('child-session-badge-wrap')).toBe(true)
    expect(screen.getByRole('button', { name: '从“Claude 主会话”创建子分支' })
      .querySelector('svg')?.dataset.icon).toBe('layers-plus')
    expect(screen.queryByRole('button', { name: '删除终端：Claude 主会话' })).toBeNull()

    const user = userEvent.setup()
    await user.click(screen.getByRole('button', { name: '从“Claude 主会话”创建子分支' }))
    await user.click(screen.getByRole('button', { name: '从共同父会话创建“Claude 主会话”的兄弟分支' }))
    expect(onFork).toHaveBeenCalledWith('session-1')
    expect(onForkSibling).toHaveBeenCalledWith('session-1')
  })

  it('offers session loading directly in every Shell and Claude card header', async () => {
    const onLoadSession = vi.fn()
    const user = userEvent.setup()
    const props = fixture()
    const view = render(<TerminalPane {...props} onLoadSession={onLoadSession} />)

    await user.click(screen.getByRole('button', { name: '载入 Claude Code 会话到“Claude 主会话”' }))
    expect(onLoadSession).toHaveBeenCalledWith('session-1')

    view.rerender(<TerminalPane {...props}
      session={{ ...props.session, kind: 'shell', title: 'Shell' }}
      onLoadSession={onLoadSession} />)
    await user.click(screen.getByRole('button', { name: '载入 Claude Code 会话到“Shell”' }))
    expect(onLoadSession).toHaveBeenLastCalledWith('session-1')
  })

  it('offers a sibling Fork on a Shell child when its common Claude parent is fork-ready', async () => {
    const onForkSibling = vi.fn()
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'shell', title: '检查日志' }}
      forkReady={false} onForkSibling={onForkSibling} />)

    const button = screen.getByRole('button', {
      name: '从共同父会话创建“检查日志”的兄弟分支'
    })
    await userEvent.setup().click(button)
    expect(onForkSibling).toHaveBeenCalledWith('session-1')
  })

  it('offers both removal scopes with projected Session and owned Worktree impact', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    render(<TerminalPane {...fixture()} workStatus="running" parentSessionId="grandparent"
      hasOwnedWorktree childNodes={[childNode('child-1'), childNode('child-2')]}
      descendantNodes={[
        { ...childNode('child-1'), hasOwnedWorktree: true },
        childNode('child-2'),
        { ...childNode('grandchild-1'), parentSessionId: 'child-1', workStatus: 'running' },
        { ...childNode('grandchild-2'), parentSessionId: 'child-2', workStatus: 'needs-input' }
      ]}
      onRemoveBranch={onRemoveBranch} />)

    expect(screen.getByRole('button', { name: '移出节点：Claude 主会话' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: '更多会话操作：Claude 主会话' })).toBeNull()
    expect(screen.queryByRole('menuitem', { name: '停止运行' })).toBeNull()
    expect(screen.queryByRole('menuitem', { name: '重新启动' })).toBeNull()

    await user.click(screen.getByRole('button', { name: '移出节点：Claude 主会话' }))
    const dialog = screen.getByRole('alertdialog', { name: '移除节点“Claude 主会话”？' })
    const nodeOnly = screen.getByRole('radio', { name: /仅移除当前节点/ })
    const branch = screen.getByRole('radio', { name: /移除当前节点及全部后代/ })
    expect(nodeOnly).toHaveProperty('checked', true)
    expect(nodeOnly.closest('label')?.textContent).toContain('影响 1 个会话')
    expect(nodeOnly.closest('label')?.textContent).toContain('1 个自有 Worktree')
    expect(nodeOnly.closest('label')?.textContent).toContain('后代会话将重连到当前节点的父级')
    expect(branch.closest('label')?.textContent).toContain('影响 5 个会话')
    expect(branch.closest('label')?.textContent).toContain('2 个自有 Worktree')
    expect(dialog.textContent).toContain('2 个运行中、1 个待输入')

    await user.click(screen.getByRole('button', { name: '移除当前节点' }))
    expect(onRemoveBranch).toHaveBeenCalledWith('session-1', 'node-only')
  })

  it('removes the selected complete descendant branch', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    render(<TerminalPane {...fixture()} childNodes={[childNode('child-1')]}
      descendantNodes={[childNode('child-1')]}
      onRemoveBranch={onRemoveBranch} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })
    await user.click(screen.getByRole('menuitem', { name: '移除节点…' }))
    await user.click(screen.getByRole('radio', { name: /移除当前节点及全部后代/ }))
    await user.click(screen.getByRole('button', { name: '移除 2 个会话' }))

    expect(onRemoveBranch).toHaveBeenCalledWith('session-1', 'node-and-descendants')
  })

  it('closes a pending structural removal when read-only recovery starts', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    const props = fixture()
    const view = render(<TerminalPane {...props} onRemoveBranch={onRemoveBranch} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })
    await user.click(screen.getByRole('menuitem', { name: '移除节点…' }))
    expect(screen.getByRole('alertdialog')).toBeTruthy()

    view.rerender(<TerminalPane {...props} readOnly onRemoveBranch={onRemoveBranch} />)

    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(onRemoveBranch).not.toHaveBeenCalled()
  })

  it('presents a leaf Session as one direct removal instead of an entire branch', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    render(<TerminalPane {...fixture()} onRemoveBranch={onRemoveBranch} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })
    await user.click(screen.getByRole('menuitem', { name: '移除节点…' }))
    const dialog = screen.getByRole('alertdialog', { name: '移除节点“Claude 主会话”？' })
    expect(dialog.textContent).toContain('影响 1 个会话')
    expect(dialog.textContent).not.toContain('移除当前节点及全部后代')
    const confirm = screen.getByRole('button', { name: '移除' })
    expect(confirm.classList.contains('is-danger')).toBe(true)

    await user.click(confirm)
    expect(onRemoveBranch).toHaveBeenCalledWith('session-1', 'node-only')
  })

  it('keeps sibling Fork in the card header and uses only current-session Fork in the right-click menu', async () => {
    const user = userEvent.setup()
    const onFork = vi.fn()
    const onForkPeer = vi.fn()
    const onForkSibling = vi.fn()
    render(<TerminalPane {...fixture()} resumable forkReady onFork={onFork}
      onForkPeer={onForkPeer} onForkSibling={onForkSibling} onDetach={vi.fn()} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByTestId('surface-session-1') })
    expect(screen.queryByRole('menu')).toBeNull()

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })

    expect(screen.queryByRole('menuitem', { name: '⑂ Fork 兄弟分支' })).toBeNull()
    expect(screen.getByRole('menuitem', { name: '↗ 独立窗口' })).toBeTruthy()
    await user.click(screen.getByRole('menuitem', { name: '⑂ Fork 会话' }))
    expect(onForkPeer).toHaveBeenCalledWith('session-1')
    expect(onFork).not.toHaveBeenCalled()

    await user.click(screen.getByRole('button', {
      name: '从共同父会话创建“Claude 主会话”的兄弟分支'
    }))
    expect(onForkSibling).toHaveBeenCalledWith('session-1')
  })

  it('renames a card from its header menu and lets a manual Claude title return to the provider title', async () => {
    const user = userEvent.setup()
    const onRename = vi.fn()
    const onRestoreAutoTitle = vi.fn()
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, titleSource: 'manual' }}
      onRename={onRename} onRestoreAutoTitle={onRestoreAutoTitle} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })
    await user.click(screen.getByRole('menuitem', { name: '重命名…' }))
    const input = screen.getByRole('textbox', { name: '会话名称' })
    await user.clear(input)
    await user.type(input, '发布问题排查')
    await user.click(screen.getByRole('button', { name: '确定' }))
    expect(onRename).toHaveBeenCalledWith('session-1', '发布问题排查')

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })
    await user.click(screen.getByRole('menuitem', { name: '恢复 Claude 自动标题' }))
    expect(onRestoreAutoTitle).toHaveBeenCalledWith('session-1')
  })

  it('dismisses pane actions on a pointer press anywhere outside the menu', async () => {
    const user = userEvent.setup()
    render(<TerminalPane {...fixture()} resumable onFork={vi.fn()} onDetach={vi.fn()} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })
    expect(screen.getByRole('menu')).toBeTruthy()

    fireEvent.pointerDown(document.body)

    expect(screen.queryByRole('menu')).toBeNull()
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

  it('shows an Agent Teams teammate as a read-only live summary instead of a Shell', () => {
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'agent-team-member', title: 'MATOU_QA_TEAMMATE' }}
      workStatus="idle" latestLines={['TEAMMATE_REAL_READY', '队友已完成当前任务']} />)

    expect(screen.getByRole('status', { name: '队友会话摘要' }).textContent)
      .toContain('TEAMMATE_REAL_READY')
    expect(screen.getByText('空闲')).toBeTruthy()
    expect(screen.queryByTestId('surface-session-1')).toBeNull()
    expect(screen.queryByRole('textbox', { name: 'Terminal input' })).toBeNull()
    expect(screen.queryByText('② Fork 会话')).toBeNull()
  })

  it('keeps an unready Fork visibly inactive and explains the next step when clicked', async () => {
    const props = fixture()
    const onFork = vi.fn()
    render(<TerminalPane {...props} forkReady={false} onFork={onFork} />)

    const button = screen.getByRole('button', { name: '从“Claude 主会话”创建子分支' })
    expect(button.getAttribute('aria-disabled')).toBe('true')
    expect(button.getAttribute('title')).toContain('在当前会话输入一次')
    await userEvent.setup().click(button)

    expect(screen.getByRole('status', { name: '创建子分支条件说明' }).textContent)
      .toContain('在当前会话输入一次，并等待 Claude Code 完成回复')
    expect(onFork).not.toHaveBeenCalled()
  })

  it('keeps Fork discoverable in the card menu while the current Claude reply is still running', async () => {
    const props = fixture()
    const onFork = vi.fn()
    const onForkPeer = vi.fn()
    const user = userEvent.setup()
    render(<TerminalPane {...props} workStatus="running" forkReady={false}
      onFork={onFork} onForkPeer={onForkPeer} onDetach={vi.fn()} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })

    const menuItem = screen.getByRole('menuitem', { name: '⑂ Fork 会话' })
    expect(menuItem.getAttribute('aria-disabled')).toBe('true')
    expect(menuItem.getAttribute('title')).toContain('当前回复完成后')
    await user.click(menuItem)

    expect(screen.getByRole('status', { name: '创建子分支条件说明' }).textContent)
      .toContain('当前回复完成后即可 Fork')
    expect(onFork).not.toHaveBeenCalled()
    expect(onForkPeer).not.toHaveBeenCalled()
  })


  it('dismisses an expired Claude identity notice after the user starts using the Shell', async () => {
    const onRetryRestore = vi.fn()
    const user = userEvent.setup()
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'shell', title: 'Shell' }}
      providerRestoreState="failed" restoreError="provider session not found"
      onRetryRestore={onRetryRestore} />)

    expect(screen.getByRole('status').textContent).toContain('原 Claude Code 对话已失效')
    expect(screen.getByRole('status').textContent).toContain('当前已切换到 Shell')
    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
    expect(screen.queryByRole('button', { name: '重试恢复' })).toBeNull()
    expect(onRetryRestore).not.toHaveBeenCalled()

    await user.type(screen.getByRole('textbox', { name: 'Terminal input' }), 'pwd')
    expect(screen.queryByText('原 Claude Code 对话已失效')).toBeNull()
  })

  it('keeps a failed Claude card visible with retry and explicit fresh-start actions', async () => {
    const onRetryRestore = vi.fn()
    const onStartFreshProvider = vi.fn()
    const user = userEvent.setup()
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'claude-code', title: '测试1' }}
      providerRestoreState="failed" restoreError="provider session not found"
      onRetryRestore={onRetryRestore} onStartFreshProvider={onStartFreshProvider} />)

    expect(screen.getByRole('status').textContent).toContain('Claude Code 恢复失败')
    expect(screen.getByRole('status').textContent).toContain('provider session not found')
    expect(screen.getByRole('button', { name: '重试恢复' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '新开 Claude Code' }))
    expect(onStartFreshProvider).toHaveBeenCalledWith('session-1')
    expect(screen.queryByTestId('surface-session-1')).toBeNull()
  })

  it('shows immediate progress while retrying a transient Claude restore failure', async () => {
    const user = userEvent.setup()
    let resolveRetry: (() => void) | undefined
    const onRetryRestore = vi.fn(() => new Promise<void>((resolve) => { resolveRetry = resolve }))
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'shell', title: 'Shell' }}
      providerRestoreState="failed" restoreError="temporary transport error"
      onRetryRestore={onRetryRestore} />)

    await user.click(screen.getByRole('button', { name: '重试恢复' }))
    expect(screen.getByRole('button', { name: '正在恢复…' })).toHaveProperty('disabled', true)
    expect(onRetryRestore).toHaveBeenCalledWith('session-1')
    resolveRetry?.()
    await waitFor(() => expect(screen.getByRole('button', { name: '重试恢复' })).toBeTruthy())
  })

  it('leaves Claude task errors in the terminal without adding a failure banner', () => {
    render(<TerminalPane {...fixture()} workStatus="error"
      latestLines={[
        'Reply exactly STA007_RECOVERED',
        'Connection refused — a firewall or proxy may be blocking it (ConnectionRefused) · attempt 10/10'
      ]} />)

    expect(screen.queryByRole('status', { name: 'Claude Code 任务失败' })).toBeNull()
    expect(screen.queryByRole('button', { name: '重试本轮任务' })).toBeNull()
    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
  })

  it('disables every card-changing action when storage pauses after a button was focused', async () => {
    const user = userEvent.setup()
    const onFork = vi.fn()
    const onRemoveBranch = vi.fn()
    const onLoadSession = vi.fn()
    const onStartFreshProvider = vi.fn()
    const props = fixture()
    const view = render(<TerminalPane {...props} resumable workStatus="error"
      latestLines={['Connection refused — attempt 10/10']}
      onFork={onFork} onRemoveBranch={onRemoveBranch}
      onLoadSession={onLoadSession} onStartFreshProvider={onStartFreshProvider} />)

    const loadSession = screen.getByRole('button', { name: '载入 Claude Code 会话到“Claude 主会话”' })
    loadSession.focus()
    expect(document.activeElement).toBe(loadSession)
    await user.click(screen.getByRole('button', { name: '触发存储异常' }))

    for (const name of [
      '载入 Claude Code 会话到“Claude 主会话”',
      '从“Claude 主会话”创建子分支',
      '移出节点：Claude 主会话'
    ]) {
      const button = screen.getByRole('button', { name })
      expect(button).toHaveProperty('disabled', true)
      expect(button.getAttribute('title')).toBe('终端存储异常，请先恢复或结束当前会话')
      fireEvent.click(button)
    }
    expect(onFork).not.toHaveBeenCalled()
    expect(onRemoveBranch).not.toHaveBeenCalled()
    expect(onLoadSession).not.toHaveBeenCalled()

    view.rerender(<TerminalPane {...props} resumable providerRestoreState="failed"
      restoreError="provider session not found" onStartFreshProvider={onStartFreshProvider} />)
    const startFresh = screen.getByRole('button', { name: '新开 Claude Code' })
    expect(startFresh).toHaveProperty('disabled', true)
    expect(startFresh.getAttribute('title')).toBe('终端存储异常，请先恢复或结束当前会话')
    fireEvent.click(startFresh)
    expect(onStartFreshProvider).not.toHaveBeenCalled()
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
    const onRemoveBranch = vi.fn()
    render(<TerminalPane {...fixture()} forkState="failed" forkError="依赖安装失败"
      onRetryFork={onRetryFork} onRemoveBranch={onRemoveBranch} />)

    expect(screen.getByRole('status').textContent).toContain('分支创建失败')
    expect(screen.getByText('依赖安装失败')).toBeTruthy()
    expect(screen.queryByTestId('surface-session-1')).toBeNull()
    await user.click(screen.getByRole('button', { name: '重试创建分支' }))
    expect(onRetryFork).toHaveBeenCalledWith('session-1')
    await user.click(screen.getByRole('button', { name: '移除节点…' }))
    expect(screen.getByRole('alertdialog', { name: '移除节点“Claude 主会话”？' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '移除' }))
    expect(onRemoveBranch).toHaveBeenCalledWith('session-1', 'node-only')
  })

  it('explains an invalidated Fork parent in user terms with a concrete next step', () => {
    render(<TerminalPane {...fixture()} forkState="failed"
      forkError="provider session not found"
      onRetryFork={vi.fn()} onRemoveBranch={vi.fn()} />)

    const status = screen.getByRole('status')
    expect(status.textContent).toContain('父会话已失效')
    expect(status.textContent).toContain('返回父会话继续')
    expect(status.textContent).toContain('移除此失败节点')
    expect(status.textContent).not.toContain('provider session not found')
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
        stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
        latestLines: [], lastUserInteractionSeq: 0
      }]} onOpenChildren={onOpenChildren} />)

    await user.click(screen.getByRole('button', { name: '查看 1 个子会话' }))
    expect(onOpenChildren).toHaveBeenCalledWith('session-1')
  })

  it('keeps the full working path discoverable while prioritizing Git and shared-worktree status', () => {
    const longPath = `/repo/${'nested-directory/'.repeat(16)}project`
    render(<TerminalPane {...fixture()} cwd={longPath}
      git={{ state: 'ready', branch: 'feature/dag', dirty: true }} sharedWorkingDirectory />)

    expect(screen.getByTitle(longPath).textContent).toBe(longPath)
    expect(screen.getByText('feature/dag*')).toBeTruthy()
    expect(screen.getByText('共享工作树')).toBeTruthy()
  })

  it('keeps a real startup failure actionable without removing sibling Sessions', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    render(<TerminalPane {...fixture()} onRemoveBranch={onRemoveBranch} />)

    await user.click(screen.getByRole('button', { name: '触发启动失败' }))
    expect(screen.getByRole('status').textContent).toContain('会话启动失败')
    expect(screen.getByText('spawn ENOENT: /missing/SHELL')).toBeTruthy()
    expect(screen.getByTestId('surface-session-1').dataset.spawnRevision).toBe('0')

    await user.click(screen.getByRole('button', { name: '重试启动' }))
    expect(screen.getByTestId('surface-session-1').dataset.spawnRevision).toBe('1')
    await user.click(screen.getByRole('button', { name: '触发启动失败' }))
    await user.click(screen.getByRole('button', { name: '移除节点…' }))
    expect(screen.getByRole('alertdialog', { name: '移除节点“Claude 主会话”？' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '移除' }))
    expect(onRemoveBranch).toHaveBeenCalledWith('session-1', 'node-only')
  })

  it('automatically restarts a failed Session after its Workspace is relinked', async () => {
    const props = fixture()
    const user = userEvent.setup()
    const view = render(<TerminalPane {...props} pathValid={false} />)

    await user.click(screen.getByRole('button', { name: '触发启动失败' }))
    expect(screen.getByText('会话启动失败')).toBeTruthy()

    view.rerender(<TerminalPane {...props} pathValid />)

    await waitFor(() => {
      expect(screen.queryByText('会话启动失败')).toBeNull()
      expect(screen.getByTestId('surface-session-1').dataset.spawnRevision).toBe('1')
    })
  })

  it.each([
    ['missing', 'Worktree 需要恢复'],
    ['recovering', '正在恢复运行环境'],
    ['handoff', '正在交接运行环境'],
    ['failed', '运行环境需要处理']
  ] as const)('keeps history visible but locks the whole card while its Worktree is %s', (state, title) => {
    render(<TerminalPane {...fixture()} environment={worktreeEnvironment(state)}
      onLoadSession={vi.fn()}
      onRestoreEnvironment={vi.fn()} onLocateEnvironment={vi.fn()}
      onHandoffEnvironment={vi.fn()} />)

    const surface = screen.getByTestId('surface-session-1')
    expect(surface.dataset.readOnly).toBe('true')
    expect(surface.dataset.inputDisabled).toBe('true')
    expect(screen.getByRole('status', { name: `运行环境${title}` })).toBeTruthy()
    expect(screen.getByRole('button', { name: '载入 Claude Code 会话到“Claude 主会话”' }))
      .toHaveProperty('disabled', true)
  })

  it('offers real recovery actions from a missing Worktree card', async () => {
    const restore = vi.fn()
    const locate = vi.fn()
    const handoff = vi.fn()
    render(<TerminalPane {...fixture()} environment={worktreeEnvironment('missing')}
      onRestoreEnvironment={restore} onLocateEnvironment={locate}
      onHandoffEnvironment={handoff} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '恢复 Worktree' }))
    expect(restore).toHaveBeenCalledWith('session-1')
    await user.click(screen.getByRole('button', { name: '定位目录' }))
    expect(locate).toHaveBeenCalledWith('session-1')
    await user.click(screen.getByRole('button', { name: '交接到 Local' }))
    expect(handoff).toHaveBeenCalledWith('session-1', 'local')
  })

  it('consumes a close shortcut while recovery blocks the card instead of replaying it after recovery', () => {
    const onRemoveBranch = vi.fn()
    const props = fixture()
    const view = render(<TerminalPane {...props} onRemoveBranch={onRemoveBranch} closeRequest={1}
      environment={worktreeEnvironment('missing')} />)
    expect(onRemoveBranch).not.toHaveBeenCalled()

    view.rerender(<TerminalPane {...props} onRemoveBranch={onRemoveBranch} closeRequest={1}
      environment={worktreeEnvironment('ready')} />)
    expect(onRemoveBranch).not.toHaveBeenCalled()

    view.rerender(<TerminalPane {...props} onRemoveBranch={onRemoveBranch} closeRequest={2}
      environment={worktreeEnvironment('ready')} />)
    expect(screen.getByRole('alertdialog', { name: '移除节点“Claude 主会话”？' })).toBeTruthy()
    expect(onRemoveBranch).not.toHaveBeenCalled()
  })
})

function fixture() {
  return {
    session: {
      id: 'session-1', taskId: 'task-1', title: 'Claude 主会话',
      kind: 'claude-code' as const, executionContextId: 'context-1'
    },
    active: true,
    pathValid: true,
    onActivate: vi.fn()
  }
}

function childNode(sessionId: string) {
  return {
    sessionId, sceneId: 'scene-1', parentSessionId: 'session-1',
    currentMode: 'shell' as const, workStatus: 'idle' as const,
    providerRestoreState: 'none' as const, canFork: false, title: sessionId,
    cwd: '/tmp', hasOwnedWorktree: false, activeChildCount: 0, stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}

function worktreeEnvironment(
  state: 'ready' | 'missing' | 'recovering' | 'handoff' | 'failed'
): SessionEnvironment {
  const base = {
    kind: 'worktree' as const,
    path: '/tmp/matou-worktree',
    localExecutionContextId: 'context-1',
    worktreeId: 'worktree-1',
    worktreeExecutionContextId: 'worktree-context-1'
  }
  if (state === 'ready') return { ...base, state }
  if (state === 'missing' || state === 'failed') return { ...base, state, error: 'path-missing' }
  return { ...base, state }
}
