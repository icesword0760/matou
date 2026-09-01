// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { SessionEnvironment } from '@matou/domain'

import { TerminalPane } from './TerminalPane'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: (props: {
    sessionId: string; visible: boolean; inputDisabled: boolean; spawnRevision?: number
    readOnly?: boolean
    onStatusChange?(status: string): void; onRuntimeError?(message: string): void
    onUserInput?(): void
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
    </div>
}))

afterEach(cleanup)

describe('Terminal pane', () => {
  it('keeps the Session surface mounted while its Scene is inactive', () => {
    const props = fixture()
    const view = render(<TerminalPane {...props} visible />)
    view.rerender(<TerminalPane {...props} active={false} visible={false} />)

    expect(screen.getByTestId('surface-session-1').dataset.visible).toBe('false')
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

  it('keeps only structural actions in the card header without process controls', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    render(<TerminalPane {...fixture()} workStatus="running"
      childNodes={[childNode('child-1'), childNode('child-2')]}
      descendantCount={4} descendantImpact={{ running: 2, needsInput: 1 }}
      onRemoveBranch={onRemoveBranch} />)

    expect(screen.queryByRole('button', { name: '更多会话操作：Claude 主会话' })).toBeNull()
    expect(screen.queryByRole('menuitem', { name: '停止运行' })).toBeNull()
    expect(screen.queryByRole('menuitem', { name: '重新启动' })).toBeNull()

    await user.click(screen.getByRole('button', { name: '移出节点：Claude 主会话' }))
    const dialog = screen.getByRole('alertdialog', { name: '移除“Claude 主会话”及其整个分支？' })
    expect(dialog.textContent).toContain('2 个直接子节点')
    expect(dialog.textContent).toContain('共 4 个后代节点')
    expect(dialog.textContent).toContain('2 个运行中、1 个待输入')
    expect(dialog.textContent).toContain('项目文件和工作树不会被删除')
    await user.click(screen.getByRole('button', { name: '停止 3 个会话并移除' }))
    expect(onRemoveBranch).toHaveBeenCalledWith('session-1', true)
  })

  it('closes a pending structural removal when read-only recovery starts', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    const props = fixture()
    const view = render(<TerminalPane {...props} onRemoveBranch={onRemoveBranch} />)

    await user.click(screen.getByRole('button', { name: '移出节点：Claude 主会话' }))
    expect(screen.getByRole('alertdialog')).toBeTruthy()

    view.rerender(<TerminalPane {...props} readOnly onRemoveBranch={onRemoveBranch} />)

    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(onRemoveBranch).not.toHaveBeenCalled()
  })

  it('presents a leaf Session as one direct removal instead of an entire branch', async () => {
    const user = userEvent.setup()
    const onRemoveBranch = vi.fn()
    render(<TerminalPane {...fixture()} onRemoveBranch={onRemoveBranch} />)

    await user.click(screen.getByRole('button', { name: '移出节点：Claude 主会话' }))
    const dialog = screen.getByRole('alertdialog', { name: '移除“Claude 主会话”？' })
    expect(dialog.textContent).toContain('“Claude 主会话”会从会话列表和 DAG 中消失')
    expect(dialog.textContent).not.toContain('整个分支')
    const confirm = screen.getByRole('button', { name: '移除' })
    expect(confirm.classList.contains('is-danger')).toBe(true)

    await user.click(confirm)
    expect(onRemoveBranch).toHaveBeenCalledWith('session-1', false)
  })

  it('opens pane actions only from the card header', async () => {
    const user = userEvent.setup()
    render(<TerminalPane {...fixture()} resumable onFork={vi.fn()} onDetach={vi.fn()} />)

    await user.pointer({ keys: '[MouseRight]', target: screen.getByTestId('surface-session-1') })
    expect(screen.queryByRole('menu')).toBeNull()

    await user.pointer({ keys: '[MouseRight]', target: screen.getByRole('banner') })

    expect(screen.getByRole('menuitem', { name: '⑂ Fork 会话' })).toBeTruthy()
    expect(screen.getByRole('menuitem', { name: '↗ 独立窗口' })).toBeTruthy()
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
    expect(button.getAttribute('title')).toContain('完成首轮对话')
    await userEvent.setup().click(button)

    expect(screen.getByRole('status', { name: '创建子分支条件说明' }).textContent)
      .toContain('在当前会话输入一次，并等待 Claude Code 完成回复')
    expect(onFork).not.toHaveBeenCalled()
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

  it('does not describe a live Claude card as Shell when an older failure projection arrives', () => {
    const props = fixture()
    render(<TerminalPane {...props}
      session={{ ...props.session, kind: 'claude-code', title: '测试1' }}
      providerRestoreState="failed" restoreError="provider session not found" />)

    expect(screen.queryByText('原 Claude Code 对话已失效')).toBeNull()
    expect(screen.queryByText('当前已切换到 Shell，可继续使用终端')).toBeNull()
    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
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

  it('shows a real Claude round failure with its reason and retries in the same pane', async () => {
    const user = userEvent.setup()
    const onRetryWork = vi.fn()
    render(<TerminalPane {...fixture()} workStatus="error"
      latestLines={[
        'Reply exactly STA007_RECOVERED',
        'Connection refused — a firewall or proxy may be blocking it (ConnectionRefused) · attempt 10/10'
      ]}
      onRetryWork={onRetryWork} />)

    const status = screen.getByRole('status', { name: 'Claude Code 任务失败' })
    expect(status.textContent).toContain('连接被拒绝')
    expect(screen.getByTestId('surface-session-1')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '重试本轮任务' }))
    expect(onRetryWork).toHaveBeenCalledWith('session-1')
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

  it('explains an invalidated Fork parent in user terms with a concrete next step', () => {
    render(<TerminalPane {...fixture()} forkState="failed"
      forkError="provider session not found"
      onRetryFork={vi.fn()} onRemoveFailedFork={vi.fn()} />)

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
    const onDelete = vi.fn()
    render(<TerminalPane {...fixture()} onDelete={onDelete} />)

    await user.click(screen.getByRole('button', { name: '触发启动失败' }))
    expect(screen.getByRole('status').textContent).toContain('会话启动失败')
    expect(screen.getByText('spawn ENOENT: /missing/SHELL')).toBeTruthy()
    expect(screen.getByTestId('surface-session-1').dataset.spawnRevision).toBe('0')

    await user.click(screen.getByRole('button', { name: '重试启动' }))
    expect(screen.getByTestId('surface-session-1').dataset.spawnRevision).toBe('1')
    await user.click(screen.getByRole('button', { name: '触发启动失败' }))
    await user.click(screen.getByRole('button', { name: '移除失败会话' }))
    expect(onDelete).toHaveBeenCalledWith('session-1', true)
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
    const onDelete = vi.fn()
    const props = fixture()
    const view = render(<TerminalPane {...props} onDelete={onDelete} closeRequest={1}
      environment={worktreeEnvironment('missing')} />)
    expect(onDelete).not.toHaveBeenCalled()

    view.rerender(<TerminalPane {...props} onDelete={onDelete} closeRequest={1}
      environment={worktreeEnvironment('ready')} />)
    expect(onDelete).not.toHaveBeenCalled()

    view.rerender(<TerminalPane {...props} onDelete={onDelete} closeRequest={2}
      environment={worktreeEnvironment('ready')} />)
    expect(onDelete).toHaveBeenCalledWith('session-1', false)
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
    cwd: '/tmp', activeChildCount: 0, stoppedChildCount: 0,
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
