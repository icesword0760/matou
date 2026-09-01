import { useState } from 'react'

import type {
  SessionEnvironmentActionResult,
  SessionEnvironmentOpenResult,
  SessionEnvironmentTarget
} from '@matou/contracts'
import type { SessionEnvironment } from '@matou/domain'

export interface SessionEnvironmentActions {
  open(sessionId: string): Promise<SessionEnvironmentOpenResult>
  restore(sessionId: string): Promise<SessionEnvironmentActionResult>
  locate(sessionId: string, path: string): Promise<SessionEnvironmentActionResult>
  handoff(sessionId: string, target: SessionEnvironmentTarget): Promise<SessionEnvironmentActionResult>
}

export function EnvironmentControlMenu(props: {
  sessionId: string
  environment: SessionEnvironment
  hasOwnedWorktree: boolean
  actions: SessionEnvironmentActions
  mutationDisabledReason?: string
  onClose(): void
}) {
  const { environment } = props
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const mutationDisabled = Boolean(props.mutationDisabledReason) || Boolean(busy)

  const run = async (label: string, action: () => Promise<unknown>) => {
    if (busy) return
    setBusy(label)
    setError('')
    try {
      await action()
      props.onClose()
    } catch (reason) {
      setError(environmentErrorText(reason))
    } finally {
      setBusy('')
    }
  }
  const open = async (kind: 'finder' | 'terminal') => {
    await run(kind === 'finder' ? '正在打开 Finder…' : '正在打开系统终端…', async () => {
      const result = await props.actions.open(props.sessionId)
      if (kind === 'finder') await window.matouDesktop?.revealDirectory(result.path)
      else await window.matouDesktop?.openDirectoryInTerminal(result.path)
    })
  }
  const locate = async () => {
    if (mutationDisabled) return
    const path = await window.matouDesktop?.selectSessionEnvironmentDirectory()
    if (!path) return
    await run('正在确认 Worktree…', async () => {
      const result = await props.actions.locate(props.sessionId, path)
      if (result.kind === 'rejected') throw new Error(locateReasonText(result.reason))
    })
  }
  const restore = () => run('正在恢复 Worktree…', async () => {
    const result = await props.actions.restore(props.sessionId)
    if (result.kind === 'rejected') throw new Error(locateReasonText(result.reason))
  })
  const handoff = (target: SessionEnvironmentTarget) => run(
    target === 'local' ? '正在交接到 Local…' : '正在交接到 Worktree…',
    async () => {
      const result = await props.actions.handoff(props.sessionId, target)
      if (result.kind === 'rejected') throw new Error(locateReasonText(result.reason))
    }
  )

  const ready = environment.state === 'ready'
  const worktreeNeedsRecovery = environment.kind === 'worktree' &&
    (environment.state === 'missing' || environment.state === 'failed')

  return <div className="environment-menu-overlay" onPointerDown={(event) => {
    if (event.currentTarget === event.target) props.onClose()
  }}>
    <section className="environment-control-menu" role="dialog" aria-label="运行环境">
      <header className="environment-control-menu__header">
        <div><span aria-hidden="true">▣</span><strong>{environmentLabel(environment)}</strong></div>
        <button type="button" aria-label="关闭运行环境菜单" onClick={props.onClose}>×</button>
      </header>
      <div className="environment-control-menu__body">
        <div className="environment-path"><span>当前路径</span><code title={environment.path}>{environment.path}</code></div>
        {environment.error && <div className="environment-warning" role="status">{environmentReasonText(environment.error)}</div>}
        <div className="environment-action-grid">
          <button type="button" disabled={!ready || Boolean(busy)} onClick={() => void open('finder')}>在 Finder 中显示</button>
          <button type="button" disabled={!ready || Boolean(busy)} onClick={() => void open('terminal')}>在系统终端中打开</button>
        </div>
        {worktreeNeedsRecovery && <div className="environment-action-stack">
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void restore()}>恢复原 Worktree</button>
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void locate()}>定位已移动的 Worktree</button>
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void handoff('local')}>交接到 Local</button>
        </div>}
        {ready && environment.kind === 'worktree' && <div className="environment-action-stack">
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void handoff('local')}>交接到 Local</button>
        </div>}
        {ready && environment.kind === 'local' && props.hasOwnedWorktree && <div className="environment-action-stack">
          <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
            onClick={() => void handoff('worktree')}>交接到自有 Worktree</button>
        </div>}
        {environment.kind === 'local' && environment.state === 'failed' && props.hasOwnedWorktree &&
          <div className="environment-action-stack">
            <button type="button" disabled={mutationDisabled} title={props.mutationDisabledReason}
              onClick={() => void handoff('worktree')}>交接到自有 Worktree</button>
          </div>}
      </div>
      {(busy || error) && <footer className={`environment-control-menu__feedback${error ? ' is-error' : ''}`} role="status">
        {error || busy}
      </footer>}
    </section>
  </div>
}

export function environmentLabel(environment: SessionEnvironment): string {
  if (environment.state === 'missing') return '待恢复'
  if (environment.state === 'recovering') return '恢复中'
  if (environment.state === 'handoff') return '交接中'
  if (environment.state === 'failed') return '待处理'
  return environment.kind === 'worktree' ? 'Worktree' : 'Local'
}

function locateReasonText(reason: string): string {
  const labels: Record<string, string> = {
    'path-missing': '所选目录不存在',
    'not-worktree': '所选目录不是 Git Worktree',
    'wrong-repository': '所选目录不属于当前仓库',
    'not-listed-by-git': 'Git 未登记这个 Worktree',
    'wrong-branch': '所选 Worktree 的分支与原会话不一致',
    'wrong-head': '所选 Worktree 的提交与原会话不一致',
    'path-owned-by-another-session': '这个 Worktree 已属于另一个会话',
    'path-conflict': '所选路径与现有 Worktree 冲突'
  }
  return labels[reason] ?? reason
}

function environmentReasonText(reason: string): string {
  return locateReasonText(reason.replace(/^(restore|locate)-failed:/, ''))
}

function environmentErrorText(reason: unknown): string {
  return reason instanceof Error ? environmentReasonText(reason.message) : '运行环境操作失败'
}
