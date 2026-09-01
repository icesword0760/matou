import { useEffect, useMemo, useState } from 'react'

import type {
  GitCheckoutResult, GitRepositoryStatus, RpcMethod
} from '@matou/contracts'

import { ConfirmDialog } from '../hierarchy/ConfirmDialog'

export interface GitControlContext {
  windowId: string
  sceneId: string
}

export interface GitRequestClient {
  request<T = unknown>(method: RpcMethod, payload: unknown, options?: { timeoutMs?: number }): Promise<T>
}

export function GitControlMenu(props: {
  client: GitRequestClient
  cwd: string
  sessionId: string
  context?: GitControlContext
  onClose(): void
}) {
  const [status, setStatus] = useState<GitRepositoryStatus>()
  const [section, setSection] = useState<'branches' | 'changes' | 'worktrees'>('branches')
  const [query, setQuery] = useState('')
  const [message, setMessage] = useState('')
  const [includeUnstaged, setIncludeUnstaged] = useState(true)
  const [newBranch, setNewBranch] = useState('')
  const [busy, setBusy] = useState('')
  const [notice, setNotice] = useState('')
  const [error, setError] = useState('')
  const [blocked, setBlocked] = useState<Extract<GitCheckoutResult, {
    kind: 'blocked-by-working-tree-changes'
  }>>()
  const [pendingCheckout, setPendingCheckout] = useState('')

  const request = async <T,>(method: RpcMethod, input: Record<string, unknown>): Promise<T> => {
    const commandId = crypto.randomUUID()
    return props.client.request<T>(method, {
      command: {
        commandId, commandType: method, requestHash: `${method}:${commandId}`
      },
      input: { ...input, now: Date.now() }
    }, { timeoutMs: 120_000 })
  }
  const refresh = async () => {
    const next = await request<GitRepositoryStatus>('git.status', { cwd: props.cwd })
    setStatus(next)
    return next
  }
  useEffect(() => {
    let active = true
    void request<GitRepositoryStatus>('git.status', { cwd: props.cwd }).then((next) => {
      if (active) setStatus(next)
    }).catch((reason: unknown) => active && setError(errorText(reason)))
    return () => { active = false }
  }, [props.cwd])

  const run = async (label: string, action: () => Promise<GitRepositoryStatus | void>) => {
    if (busy) return
    setBusy(label); setError(''); setNotice('')
    try {
      const next = await action()
      if (next) setStatus(next)
    } catch (reason) {
      setError(errorText(reason))
    } finally {
      setBusy('')
    }
  }
  const checkout = async (branch: string) => run('正在切换分支…', async () => {
    const result = await request<GitCheckoutResult>('git.checkout', { cwd: props.cwd, branch })
    if (result.kind === 'blocked-by-working-tree-changes') {
      setBlocked(result)
      return result.status
    }
    setNotice(`已切换到 ${branch}`)
    return result.status
  })
  const commit = async (thenPush = false) => run(thenPush ? '正在提交并推送…' : '正在提交…', async () => {
    let checkoutStillBlocked = false
    let next = await request<GitRepositoryStatus>('git.commit', {
      cwd: props.cwd, message, includeUnstaged
    })
    setMessage('')
    if (thenPush) next = await request<GitRepositoryStatus>('git.push', { cwd: props.cwd })
    if (pendingCheckout) {
      const target = pendingCheckout
      setPendingCheckout('')
      const result = await request<GitCheckoutResult>('git.checkout', { cwd: props.cwd, branch: target })
      next = result.status
      if (result.kind === 'blocked-by-working-tree-changes') {
        checkoutStillBlocked = true
        setBlocked(result)
      }
      else setSection('branches')
    }
    if (!checkoutStillBlocked) setNotice(thenPush ? '提交与推送已完成' : '提交已完成')
    return next
  })
  const branches = useMemo(() => {
    const keyword = query.trim().toLocaleLowerCase()
    return (status?.branches ?? []).filter(({ name }) => !keyword || name.toLocaleLowerCase().includes(keyword))
  }, [status?.branches, query])
  const pendingFiles = status
    ? status.stagedCount + status.unstagedCount + status.untrackedCount
    : 0
  const canCommit = Boolean(message.trim() && status && (
    includeUnstaged ? pendingFiles > 0 : status.stagedCount > 0
  ))

  return <>
    <div className="git-menu-overlay" onPointerDown={(event) => {
      if (event.currentTarget === event.target) props.onClose()
    }}>
      <section className="git-control-menu" role="dialog" aria-label="Git 与 Worktree">
        <header className="git-control-menu__header">
          <div><span className="git-branch-icon">⑂</span><strong>{status?.currentBranch ?? status?.detachedHead ?? 'Git'}</strong></div>
          <button type="button" aria-label="关闭 Git 菜单" onClick={props.onClose}>×</button>
        </header>
        <nav className="git-control-menu__tabs" aria-label="Git 功能">
          <button className={section === 'branches' ? 'is-active' : ''} onClick={() => setSection('branches')}>分支</button>
          <button className={section === 'changes' ? 'is-active' : ''} onClick={() => setSection('changes')}>更改{pendingFiles > 0 ? ` ${pendingFiles}` : ''}</button>
          <button className={section === 'worktrees' ? 'is-active' : ''} onClick={() => setSection('worktrees')}>Worktree{status && status.worktrees.length > 1 ? ` ${status.worktrees.length}` : ''}</button>
        </nav>
        {!status && !error && <div className="git-control-menu__empty">正在读取仓库状态…</div>}
        {status && section === 'branches' && <div className="git-control-menu__body">
          <input className="git-control-menu__search" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索分支" autoFocus />
          <div className="git-control-menu__list">
            {branches.map((branch) => <button type="button" className={`git-branch-row${branch.current ? ' is-current' : ''}`}
              key={branch.name} disabled={branch.current || Boolean(branch.checkedOutPath) || Boolean(busy)}
              title={branch.checkedOutPath && !branch.current ? `已在 ${branch.checkedOutPath} 中打开` : undefined}
              onClick={() => void checkout(branch.name)}>
              <span>⑂</span><span>{branch.name}</span>
              {branch.current && <span className="git-row-meta">✓</span>}
              {!branch.current && branch.checkedOutPath && <span className="git-row-meta">已在 Worktree 中</span>}
            </button>)}
          </div>
          <form className="git-inline-form" onSubmit={(event) => {
            event.preventDefault()
            void run('正在创建分支…', async () => {
              const next = await request<GitRepositoryStatus>('git.create-branch', {
                cwd: props.cwd, branch: newBranch
              })
              setNewBranch(''); setNotice(`已创建并切换到 ${next.currentBranch}`)
              return next
            })
          }}>
            <input value={newBranch} onChange={(event) => setNewBranch(event.target.value)} placeholder="新分支名称" />
            <button disabled={!newBranch.trim() || Boolean(busy)}>创建并切换</button>
          </form>
        </div>}
        {status && section === 'changes' && <div className="git-control-menu__body git-changes-panel">
          <div className="git-change-summary">
            <strong>{pendingFiles === 0 ? '工作区无待提交更改' : `${pendingFiles} 项更改`}</strong>
            <span className="git-line-stats"><b>+{status.additions}</b><i>-{status.deletions}</i></span>
          </div>
          <textarea value={message} onChange={(event) => setMessage(event.target.value)} placeholder="提交信息" rows={3} />
          <label className="git-checkbox"><input type="checkbox" checked={includeUnstaged}
            onChange={(event) => setIncludeUnstaged(event.target.checked)} /> 包含未暂存的更改</label>
          <div className="git-action-stack">
            <button className="git-primary-action" disabled={!canCommit || Boolean(busy)} onClick={() => void commit(false)}>提交</button>
            <button disabled={!canCommit || !status.hasRemote || Boolean(busy)} onClick={() => void commit(true)}>提交并推送</button>
            <button disabled={!status.hasRemote || !status.currentBranch || status.ahead === 0 || Boolean(busy)}
              onClick={() => void run('正在推送…', async () => {
                const next = await request<GitRepositoryStatus>('git.push', { cwd: props.cwd })
                setNotice('推送已完成'); return next
              })}>推送{status.ahead > 0 ? ` ${status.ahead}` : ''}</button>
          </div>
          {(status.ahead > 0 || status.behind > 0) && <small className="git-sync-summary">领先 {status.ahead} · 落后 {status.behind}</small>}
        </div>}
        {status && section === 'worktrees' && <div className="git-control-menu__body">
          <div className="git-control-menu__list git-worktree-list">
            {status.worktrees.map((worktree) => <article className={`git-worktree-row${worktree.current ? ' is-current' : ''}`} key={worktree.path}>
              <div><strong>{worktree.branch}</strong><span>{compactPath(worktree.path)}</span></div>
              <div className="git-worktree-badges">
                {worktree.current && <span>当前</span>}{worktree.dirty && <span>有更改</span>}
                {worktree.sessionCount > 0 && <span>{worktree.sessionCount} 会话</span>}
              </div>
              <div className="git-worktree-actions">
                <button onClick={() => void window.matouDesktop?.revealDirectory(worktree.path)}>Finder</button>
                {worktree.managed && !worktree.current && <button className="is-danger" disabled={worktree.sessionCount > 0 || Boolean(busy)}
                  title={worktree.sessionCount > 0 ? '先移出关联会话' : undefined}
                  onClick={() => void run('正在移除 Worktree…', async () => {
                    const next = await request<GitRepositoryStatus>('git.worktree-remove', { worktreeId: worktree.worktreeId })
                    setNotice(worktree.dirty ? '本地更改已保留，Worktree 未移除' : 'Worktree 已移除')
                    return next
                  })}>移除</button>}
              </div>
            </article>)}
          </div>
          <form className="git-inline-form" onSubmit={(event) => {
            event.preventDefault()
            void run('正在创建 Worktree…', async () => {
              const next = await request<GitRepositoryStatus>('git.worktree-create', {
                cwd: props.cwd, sessionId: props.sessionId, branch: newBranch,
                baseRef: status.currentBranch ?? 'HEAD'
              })
              setNewBranch(''); setNotice('Worktree 已创建')
              return next
            })
          }}>
            <input value={newBranch} onChange={(event) => setNewBranch(event.target.value)} placeholder="新 Worktree 分支" />
            <button disabled={!newBranch.trim() || Boolean(busy)}>创建</button>
          </form>
        </div>}
        {(busy || notice || error) && <footer className={`git-control-menu__feedback${error ? ' is-error' : ''}`} role="status">
          {error || busy || notice}
          {!busy && <button onClick={() => { setError(''); setNotice(''); void refresh() }}>刷新</button>}
        </footer>}
      </section>
    </div>
    {blocked && <ConfirmDialog title="切换前需要处理当前更改"
      body={`${blocked.conflictingPaths.length > 0 ? `${blocked.conflictingPaths.length} 个文件会被覆盖。` : '当前更改会与目标分支冲突。'}提交后将继续切换到 ${blocked.targetBranch}。`}
      confirmLabel="填写提交信息" onCancel={() => setBlocked(undefined)} onConfirm={() => {
        setPendingCheckout(blocked.targetBranch)
        setBlocked(undefined)
        setSection('changes')
      }} />}
  </>
}

function errorText(reason: unknown): string {
  return reason instanceof Error ? reason.message : 'Git 操作失败'
}

function compactPath(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/').filter(Boolean)
  return parts.length <= 3 ? path : `…/${parts.slice(-3).join('/')}`
}
