import { useEffect, useMemo, useRef, useState } from 'react'

import type {
  GitCheckoutResult, GitRepositoryStatus, GitWorktreeSummary, RpcMethod
} from '@matou/contracts'

import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { AppIcon } from '../ui/AppIcon'

export interface GitControlContext {
  windowId: string
  sceneId: string
}

export interface GitRequestClient {
  request<T = unknown>(method: RpcMethod, payload: unknown, options?: { timeoutMs?: number }): Promise<T>
}

type GitControlView = 'branches' | 'create-branch' | 'worktrees' | 'create-worktree' | 'commit'

export function GitControlMenu(props: {
  client: GitRequestClient
  cwd: string
  sessionId: string
  context?: GitControlContext
  dialogLabel?: string
  branchRowsAsButtons?: boolean
  onClose(): void
}) {
  const [status, setStatus] = useState<GitRepositoryStatus>()
  const [view, setView] = useState<GitControlView>('branches')
  const [query, setQuery] = useState('')
  const [selectedBranchIndex, setSelectedBranchIndex] = useState(0)
  const [message, setMessage] = useState('')
  const [includeUnstaged, setIncludeUnstaged] = useState(true)
  const [newBranch, setNewBranch] = useState('')
  const [busy, setBusy] = useState('')
  const [notice, setNotice] = useState('')
  const [error, setError] = useState('')
  const [worktreeMenuPath, setWorktreeMenuPath] = useState('')
  const [blocked, setBlocked] = useState<Extract<GitCheckoutResult, {
    kind: 'blocked-by-working-tree-changes'
  }>>()
  const [pendingCheckout, setPendingCheckout] = useState('')
  const runningRef = useRef(false)

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

  useEffect(() => setSelectedBranchIndex(0), [query])
  useEffect(() => {
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      event.stopPropagation()
      setWorktreeMenuPath('')
      if (view === 'branches') props.onClose()
      else if (view === 'create-worktree') setView('worktrees')
      else setView('branches')
    }
    window.addEventListener('keydown', onEscape)
    return () => window.removeEventListener('keydown', onEscape)
  }, [view, props.onClose])

  const run = async (label: string, action: () => Promise<GitRepositoryStatus | void>) => {
    if (runningRef.current) return
    runningRef.current = true
    setBusy(label); setError(''); setNotice(''); setWorktreeMenuPath('')
    try {
      const next = await action()
      if (next) setStatus(next)
    } catch (reason) {
      setError(errorText(reason))
    } finally {
      runningRef.current = false
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
    props.onClose()
    return result.status
  })
  const commit = async (thenPush = false) => run(thenPush ? '正在提交并推送…' : '正在提交…', async () => {
    if (!status) return
    let checkoutStillBlocked = false
    let next = await request<GitRepositoryStatus>('git.commit', {
      cwd: props.cwd,
      message: message.trim() || generatedCommitMessage(status),
      includeUnstaged
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
      } else {
        setView('branches')
      }
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
  const additionalWorktreeCount = status?.worktrees.filter(({ main }) => !main).length ?? 0
  const canCommit = Boolean(status && (includeUnstaged ? pendingFiles > 0 : status.stagedCount > 0))
  const selectedBranch = branches[selectedBranchIndex]
  const showView = (next: GitControlView) => {
    setView(next)
    setWorktreeMenuPath('')
    setNotice('')
    setError('')
  }
  const selectBranchByKeyboard = (direction: -1 | 1) => {
    if (branches.length === 0) return
    setSelectedBranchIndex((current) => Math.max(0, Math.min(branches.length - 1, current + direction)))
  }

  return <>
    <div className="git-menu-overlay" onPointerDown={(event) => {
      if (event.currentTarget === event.target) props.onClose()
    }}>
      <section className="git-control-menu" role="dialog" aria-label={props.dialogLabel ?? 'Git 控制'}>
        {!status && !error && <div className="git-control-menu__empty">正在读取仓库状态…</div>}
        {status && view === 'branches' && <div className="git-picker-view">
          <label className="git-search-field">
            <SearchIcon />
            <span className="sr-only">搜索分支</span>
            <input value={query} onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'ArrowDown') { event.preventDefault(); selectBranchByKeyboard(1) }
                if (event.key === 'ArrowUp') { event.preventDefault(); selectBranchByKeyboard(-1) }
                if (event.key === 'Enter' && selectedBranch && !selectedBranch.current && !selectedBranch.checkedOutPath) {
                  event.preventDefault(); void checkout(selectedBranch.name)
                }
              }}
              placeholder="搜索 matou 分支" autoFocus />
            {query && <button type="button" className="git-search-clear" aria-label="清空搜索"
              onClick={() => setQuery('')}><AppIcon name="x" /></button>}
          </label>
          <div className="git-section-label">分支</div>
          <div className="git-branch-list" role="listbox" aria-label="Git 分支">
            {branches.map((branch, index) => <button type="button"
              {...(props.branchRowsAsButtons ? {} : { role: 'option', 'aria-selected': branch.current })}
              className={`git-branch-row${branch.current ? ' is-current' : ''}${selectedBranchIndex === index ? ' is-keyboard' : ''}`}
              key={branch.name} disabled={Boolean(busy)}
              title={branch.checkedOutPath && !branch.current ? `已在 ${branch.checkedOutPath} 中打开` : undefined}
              onPointerMove={() => setSelectedBranchIndex(index)}
              onClick={() => {
                if (branch.current || branch.checkedOutPath) return
                void checkout(branch.name)
              }}>
              <BranchIcon />
              <span className="git-branch-copy"><strong>{branch.name}</strong>
                {branch.current && pendingFiles > 0 && <small>未提交：{pendingFiles.toLocaleString('zh-CN')} 个文件</small>}
                {!branch.current && branch.checkedOutPath && <small>已在 Worktree 中</small>}
              </span>
              {branch.current && <span className="git-row-check">✓</span>}
            </button>)}
            {branches.length === 0 && <div className="git-list-empty">没有匹配的分支</div>}
          </div>
          <div className="git-picker-actions">
            <button type="button" onClick={() => showView('create-branch')}><PlusIcon /><span>创建并检出新分支…</span></button>
            <button type="button" aria-label={`管理 Worktree… ${additionalWorktreeCount}`} onClick={() => showView('worktrees')}><WorktreeIcon /><span>管理 Worktree…</span>{additionalWorktreeCount > 0 && <small>{additionalWorktreeCount}</small>}</button>
            <button type="button" aria-label="提交与推送…" onClick={() => showView('commit')}><CommitIcon /><span>提交与推送…</span>{pendingFiles > 0 && <small>{pendingFiles.toLocaleString('zh-CN')}</small>}</button>
          </div>
        </div>}

        {status && view === 'create-branch' && <div className="git-subview git-create-view">
          <SubviewHeader title="创建新分支" backLabel="返回分支列表" onBack={() => showView('branches')} />
          <form onSubmit={(event) => {
            event.preventDefault()
            if (!newBranch.trim()) return
            void run('正在创建分支…', async () => {
              const next = await request<GitRepositoryStatus>('git.create-branch', {
                cwd: props.cwd, branch: newBranch.trim()
              })
              setNewBranch(''); setNotice(`已创建并切换到 ${next.currentBranch}`)
              props.onClose()
              return next
            })
          }}>
            <label htmlFor="git-new-branch">分支名称</label>
            <input id="git-new-branch" value={newBranch} onChange={(event) => setNewBranch(event.target.value)}
              placeholder="例如 feature/improve-git-menu" autoFocus />
            <div className="git-base-row"><BranchIcon />基于当前分支 <strong>{status.currentBranch ?? 'HEAD'}</strong></div>
            <div className="git-form-actions"><button type="button" onClick={() => showView('branches')}>取消</button>
              <button className="is-primary" disabled={!newBranch.trim() || Boolean(busy)}>创建并检出</button></div>
          </form>
        </div>}

        {status && view === 'worktrees' && <div className="git-subview git-worktree-view">
          <SubviewHeader title="Worktree" backLabel="返回分支列表" onBack={() => showView('branches')} />
          <div className="git-worktree-list">
            {status.worktrees.map((worktree) => <article className="git-worktree-row" key={worktree.path}>
              <WorktreeIcon />
              <div className="git-worktree-copy"><strong>{worktree.branch}</strong><small>{compactPath(worktree.path)}</small>
                <div className="git-worktree-tags">{worktree.current && <span className="is-current">当前</span>}
                  {worktree.dirty && <span>有更改</span>}{worktree.sessionCount > 0 && <span>{worktree.sessionCount} 会话</span>}</div>
              </div>
              <div className="git-worktree-row-actions">
                <button type="button" className="git-worktree-more" aria-label={`${worktree.branch} 更多操作`}
                  onClick={() => setWorktreeMenuPath((path) => path === worktree.path ? '' : worktree.path)}><AppIcon name="ellipsis" /></button>
              </div>
              {worktreeMenuPath === worktree.path && <div className="git-worktree-menu">
                <button type="button" onClick={() => {
                  setWorktreeMenuPath('')
                  void window.matouDesktop?.revealDirectory(worktree.path)
                }}>在 Finder 中显示</button>
                {worktree.managed && !worktree.current && <button type="button" className="is-danger"
                  disabled={worktree.sessionCount > 0 || Boolean(busy)}
                  title={worktree.sessionCount > 0 ? '先移出关联会话' : undefined}
                  onClick={() => void removeWorktree(worktree)}>移除 Worktree</button>}
              </div>}
            </article>)}
          </div>
          <div className="git-subview-footer"><button type="button" onClick={() => showView('create-worktree')}><PlusIcon />创建新 Worktree…</button></div>
        </div>}

        {status && view === 'create-worktree' && <div className="git-subview git-create-view">
          <SubviewHeader title="创建 Worktree" backLabel="返回 Worktree 列表" onBack={() => showView('worktrees')} />
          <form onSubmit={(event) => {
            event.preventDefault()
            if (!newBranch.trim()) return
            void run('正在创建 Worktree…', async () => {
              const next = await request<GitRepositoryStatus>('git.worktree-create', {
                cwd: props.cwd, sessionId: props.sessionId, branch: newBranch.trim(),
                baseRef: status.currentBranch ?? 'HEAD'
              })
              setNewBranch(''); setNotice('Worktree 已创建'); setView('worktrees')
              return next
            })
          }}>
            <label htmlFor="git-new-worktree">新 Worktree 分支</label>
            <input id="git-new-worktree" value={newBranch} onChange={(event) => setNewBranch(event.target.value)}
              placeholder="例如 feature/new-worktree" autoFocus />
            <div className="git-base-row"><WorktreeIcon />创建在 Matou Worktree 目录</div>
            <div className="git-form-actions"><button type="button" onClick={() => showView('worktrees')}>取消</button>
              <button className="is-primary" disabled={!newBranch.trim() || Boolean(busy)}>创建</button></div>
          </form>
        </div>}

        {status && view === 'commit' && <div className="git-subview git-commit-view">
          <SubviewHeader title="提交与推送" backLabel="返回分支列表" onBack={() => showView('branches')} />
          <div className="git-commit-branch"><BranchIcon /><strong>{status.currentBranch ?? status.detachedHead ?? 'HEAD'}</strong><span>⌄</span></div>
          <textarea value={message} onChange={(event) => setMessage(event.target.value)}
            onKeyDown={(event) => {
              if ((event.metaKey || event.ctrlKey) && event.key === 'Enter' && canCommit && !busy) {
                event.preventDefault(); void commit(false)
              }
            }}
            placeholder="提交信息（留空将自动生成）…" rows={4} autoFocus />
          <label className="git-commit-scope"><input type="checkbox" aria-label="包含未暂存的更改" checked={includeUnstaged}
            onChange={(event) => setIncludeUnstaged(event.target.checked)} /><span>包含未暂存的更改</span>
            <span className="git-line-stats"><span>◌</span><b>+{status.additions.toLocaleString('zh-CN')}</b><i>-{status.deletions.toLocaleString('zh-CN')}</i></span></label>
          <div className="git-commit-actions">
            <button type="button" aria-label="提交" disabled={!canCommit || Boolean(busy)}
              title={!canCommit ? '当前没有可提交的更改' : undefined}
              onClick={() => void commit(false)}><CommitIcon />提交<span className="git-shortcut">⌘↵</span></button>
            <button type="button" disabled={!canCommit || !status.hasRemote || Boolean(busy)}
              title={!canCommit ? '当前没有可提交的更改' : !status.hasRemote ? '仓库尚未配置远端' : undefined}
              onClick={() => void commit(true)}><PushIcon />提交并推送</button>
            <button type="button" disabled={!status.hasRemote || !status.currentBranch || !status.canPush || Boolean(busy)}
              title={!status.hasRemote ? '仓库尚未配置远端' : !status.canPush ? '当前没有待推送的提交' : undefined}
              onClick={() => void run('正在推送…', async () => {
                const next = await request<GitRepositoryStatus>('git.push', { cwd: props.cwd })
                setNotice('推送已完成'); return next
              })}><PushIcon />推送</button>
          </div>
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
        setView('commit')
      }} />}
  </>

  function removeWorktree(worktree: GitWorktreeSummary) {
    return run('正在移除 Worktree…', async () => {
      const next = await request<GitRepositoryStatus>('git.worktree-remove', { worktreeId: worktree.worktreeId })
      setNotice(worktree.dirty ? '本地更改已保留，Worktree 未移除' : 'Worktree 已移除')
      return next
    })
  }
}

function SubviewHeader(props: { title: string; backLabel: string; onBack(): void }) {
  return <header className="git-subview-header"><button type="button" aria-label={props.backLabel} onClick={props.onBack}>‹</button>
    <strong>{props.title}</strong><span /></header>
}

function SearchIcon() {
  return <AppIcon name="search" />
}
function BranchIcon() {
  return <AppIcon className="git-icon" name="git-branch" />
}
function WorktreeIcon() {
  return <AppIcon className="git-icon" name="network" />
}
function PlusIcon() {
  return <AppIcon className="git-icon" name="plus" />
}
function CommitIcon() {
  return <AppIcon className="git-icon" name="git-commit-horizontal" />
}
function PushIcon() {
  return <AppIcon className="git-icon" name="upload" />
}

function generatedCommitMessage(status: GitRepositoryStatus): string {
  const count = status.stagedCount + status.unstagedCount + status.untrackedCount
  return `chore: update ${count} ${count === 1 ? 'file' : 'files'}`
}

function errorText(reason: unknown): string {
  return reason instanceof Error ? reason.message : 'Git 操作失败'
}

function compactPath(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/').filter(Boolean)
  return parts.length <= 3 ? path : `…/${parts.slice(-3).join('/')}`
}
