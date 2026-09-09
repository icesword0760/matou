import { useEffect, useMemo, useRef, useState } from 'react'

import type {
  GitCheckoutResult, GitRepositoryStatus, GitWorktreeSummary, RpcMethod
} from '@matou/contracts'

import { ConfirmDialog } from '../hierarchy/ConfirmDialog'
import { AppIcon } from '../ui/AppIcon'
import { useLocale, useMessages } from '../i18n/LocaleProvider'
import type { Messages } from '../i18n/messages'

type HudMessages = Messages['hud']

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
  const messages = useMessages()
  const m = messages.hud
  const locale = useLocale()
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
    }).catch((reason: unknown) => active && setError(errorText(reason, m)))
    return () => { active = false }
  }, [props.cwd, m])

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
      setError(errorText(reason, m))
    } finally {
      runningRef.current = false
      setBusy('')
    }
  }
  const checkout = async (branch: string) => run(m.switchingBranch, async () => {
    const result = await request<GitCheckoutResult>('git.checkout', { cwd: props.cwd, branch })
    if (result.kind === 'blocked-by-working-tree-changes') {
      setBlocked(result)
      return result.status
    }
    setNotice(m.switchedToBranch(branch))
    props.onClose()
    return result.status
  })
  const commit = async (thenPush = false) => run(thenPush ? m.committingAndPushing : m.committing, async () => {
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
    if (!checkoutStillBlocked) setNotice(thenPush ? m.commitAndPushDone : m.commitDone)
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
      <section className="git-control-menu" role="dialog" aria-label={props.dialogLabel ?? m.gitControl}>
        {!status && !error && <div className="git-control-menu__empty">{m.readingRepository}</div>}
        {status && view === 'branches' && <div className="git-picker-view">
          <label className="git-search-field">
            <SearchIcon />
            <span className="sr-only">{m.searchBranches}</span>
            <input value={query} onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'ArrowDown') { event.preventDefault(); selectBranchByKeyboard(1) }
                if (event.key === 'ArrowUp') { event.preventDefault(); selectBranchByKeyboard(-1) }
                if (event.key === 'Enter' && selectedBranch && !selectedBranch.current && !selectedBranch.checkedOutPath) {
                  event.preventDefault(); void checkout(selectedBranch.name)
                }
              }}
              placeholder={m.searchBranchesPlaceholder} autoFocus />
            {query && <button type="button" className="git-search-clear" aria-label={m.clearSearch}
              onClick={() => setQuery('')}><AppIcon name="x" /></button>}
          </label>
          <div className="git-section-label">{m.branchesSection}</div>
          <div className="git-branch-list" role="listbox" aria-label={m.branchList}>
            {branches.map((branch, index) => <button type="button"
              {...(props.branchRowsAsButtons ? {} : { role: 'option', 'aria-selected': branch.current })}
              className={`git-branch-row${branch.current ? ' is-current' : ''}${selectedBranchIndex === index ? ' is-keyboard' : ''}`}
              key={branch.name} disabled={Boolean(busy)}
              title={branch.checkedOutPath && !branch.current ? m.branchCheckedOutAt(branch.checkedOutPath) : undefined}
              onPointerMove={() => setSelectedBranchIndex(index)}
              onClick={() => {
                if (branch.current || branch.checkedOutPath) return
                void checkout(branch.name)
              }}>
              <BranchIcon />
              <span className="git-branch-copy"><strong>{branch.name}</strong>
                {branch.current && pendingFiles > 0 && <small>{m.uncommittedFiles(pendingFiles, pendingFiles.toLocaleString(locale))}</small>}
                {!branch.current && branch.checkedOutPath && <small>{m.branchInWorktree}</small>}
              </span>
              {branch.current && <span className="git-row-check">✓</span>}
            </button>)}
            {branches.length === 0 && <div className="git-list-empty">{m.noMatchingBranches}</div>}
          </div>
          <div className="git-picker-actions">
            <button type="button" onClick={() => showView('create-branch')}><PlusIcon /><span>{m.createBranchAction}</span></button>
            <button type="button" aria-label={`${m.manageWorktrees} ${additionalWorktreeCount}`} onClick={() => showView('worktrees')}><WorktreeIcon /><span>{m.manageWorktrees}</span>{additionalWorktreeCount > 0 && <small>{additionalWorktreeCount}</small>}</button>
            <button type="button" aria-label={m.commitAndPushAction} onClick={() => showView('commit')}><CommitIcon /><span>{m.commitAndPushAction}</span>{pendingFiles > 0 && <small>{pendingFiles.toLocaleString(locale)}</small>}</button>
          </div>
        </div>}

        {status && view === 'create-branch' && <div className="git-subview git-create-view">
          <SubviewHeader title={m.createBranchTitle} backLabel={m.backToBranches} onBack={() => showView('branches')} />
          <form onSubmit={(event) => {
            event.preventDefault()
            if (!newBranch.trim()) return
            void run(m.creatingBranch, async () => {
              const next = await request<GitRepositoryStatus>('git.create-branch', {
                cwd: props.cwd, branch: newBranch.trim()
              })
              setNewBranch(''); setNotice(m.branchCreated(next.currentBranch ?? 'HEAD'))
              props.onClose()
              return next
            })
          }}>
            <label htmlFor="git-new-branch">{m.branchName}</label>
            <input id="git-new-branch" value={newBranch} onChange={(event) => setNewBranch(event.target.value)}
              placeholder={m.branchNamePlaceholder} autoFocus />
            <div className="git-base-row"><BranchIcon />{m.basedOnBranch}{' '}<strong>{status.currentBranch ?? 'HEAD'}</strong></div>
            <div className="git-form-actions"><button type="button" onClick={() => showView('branches')}>{messages.common.cancel}</button>
              <button className="is-primary" disabled={!newBranch.trim() || Boolean(busy)}>{m.createAndCheckout}</button></div>
          </form>
        </div>}

        {status && view === 'worktrees' && <div className="git-subview git-worktree-view">
          <SubviewHeader title={m.worktreesTitle} backLabel={m.backToBranches} onBack={() => showView('branches')} />
          <div className="git-worktree-list">
            {status.worktrees.map((worktree) => <article className="git-worktree-row" key={worktree.path}>
              <WorktreeIcon />
              <div className="git-worktree-copy"><strong>{worktree.branch}</strong><small>{compactPath(worktree.path)}</small>
                <div className="git-worktree-tags">{worktree.current && <span className="is-current">{m.worktreeCurrent}</span>}
                  {worktree.dirty && <span>{m.worktreeDirty}</span>}{worktree.sessionCount > 0 && <span>{m.worktreeSessions(worktree.sessionCount)}</span>}</div>
              </div>
              <div className="git-worktree-row-actions">
                <button type="button" className="git-worktree-more" aria-label={m.worktreeMoreActions(worktree.branch)}
                  onClick={() => setWorktreeMenuPath((path) => path === worktree.path ? '' : worktree.path)}><AppIcon name="ellipsis" /></button>
              </div>
              {worktreeMenuPath === worktree.path && <div className="git-worktree-menu">
                <button type="button" onClick={() => {
                  setWorktreeMenuPath('')
                  void window.matouDesktop?.revealDirectory(worktree.path)
                }}>{messages.hierarchyShell.taskSidebar.revealInFinder}</button>
                {worktree.managed && !worktree.current && <button type="button" className="is-danger"
                  disabled={worktree.sessionCount > 0 || Boolean(busy)}
                  title={worktree.sessionCount > 0 ? m.removeWorktreeBlocked : undefined}
                  onClick={() => void removeWorktree(worktree)}>{m.removeWorktree}</button>}
              </div>}
            </article>)}
          </div>
          <div className="git-subview-footer"><button type="button" onClick={() => showView('create-worktree')}><PlusIcon />{m.createWorktreeAction}</button></div>
        </div>}

        {status && view === 'create-worktree' && <div className="git-subview git-create-view">
          <SubviewHeader title={m.createWorktreeTitle} backLabel={m.backToWorktrees} onBack={() => showView('worktrees')} />
          <form onSubmit={(event) => {
            event.preventDefault()
            if (!newBranch.trim()) return
            void run(m.creatingWorktree, async () => {
              const next = await request<GitRepositoryStatus>('git.worktree-create', {
                cwd: props.cwd, sessionId: props.sessionId, branch: newBranch.trim(),
                baseRef: status.currentBranch ?? 'HEAD'
              })
              setNewBranch(''); setNotice(m.worktreeCreated); setView('worktrees')
              return next
            })
          }}>
            <label htmlFor="git-new-worktree">{m.newWorktreeBranch}</label>
            <input id="git-new-worktree" value={newBranch} onChange={(event) => setNewBranch(event.target.value)}
              placeholder={m.newWorktreeBranchPlaceholder} autoFocus />
            <div className="git-base-row"><WorktreeIcon />{m.worktreeLocationHint}</div>
            <div className="git-form-actions"><button type="button" onClick={() => showView('worktrees')}>{messages.common.cancel}</button>
              <button className="is-primary" disabled={!newBranch.trim() || Boolean(busy)}>{m.create}</button></div>
          </form>
        </div>}

        {status && view === 'commit' && <div className="git-subview git-commit-view">
          <SubviewHeader title={m.commitAndPushTitle} backLabel={m.backToBranches} onBack={() => showView('branches')} />
          <div className="git-commit-branch"><BranchIcon /><strong>{status.currentBranch ?? status.detachedHead ?? 'HEAD'}</strong><span>⌄</span></div>
          <textarea value={message} onChange={(event) => setMessage(event.target.value)}
            onKeyDown={(event) => {
              if ((event.metaKey || event.ctrlKey) && event.key === 'Enter' && canCommit && !busy) {
                event.preventDefault(); void commit(false)
              }
            }}
            placeholder={m.commitMessagePlaceholder} rows={4} autoFocus />
          <label className="git-commit-scope"><input type="checkbox" aria-label={m.includeUnstaged} checked={includeUnstaged}
            onChange={(event) => setIncludeUnstaged(event.target.checked)} /><span>{m.includeUnstaged}</span>
            <span className="git-line-stats"><span>◌</span><b>+{status.additions.toLocaleString(locale)}</b><i>-{status.deletions.toLocaleString(locale)}</i></span></label>
          <div className="git-commit-actions">
            <button type="button" aria-label={m.commit} disabled={!canCommit || Boolean(busy)}
              title={!canCommit ? m.nothingToCommit : undefined}
              onClick={() => void commit(false)}><CommitIcon />{m.commit}<span className="git-shortcut">⌘↵</span></button>
            <button type="button" disabled={!canCommit || !status.hasRemote || Boolean(busy)}
              title={!canCommit ? m.nothingToCommit : !status.hasRemote ? m.noRemote : undefined}
              onClick={() => void commit(true)}><PushIcon />{m.commitAndPush}</button>
            <button type="button" disabled={!status.hasRemote || !status.currentBranch || !status.canPush || Boolean(busy)}
              title={!status.hasRemote ? m.noRemote : !status.canPush ? m.nothingToPush : undefined}
              onClick={() => void run(m.pushing, async () => {
                const next = await request<GitRepositoryStatus>('git.push', { cwd: props.cwd })
                setNotice(m.pushDone); return next
              })}><PushIcon />{m.push}</button>
          </div>
        </div>}

        {(busy || notice || error) && <footer className={`git-control-menu__feedback${error ? ' is-error' : ''}`} role="status">
          {error || busy || notice}
          {!busy && <button onClick={() => { setError(''); setNotice(''); void refresh() }}>{m.refresh}</button>}
        </footer>}
      </section>
    </div>
    {blocked && <ConfirmDialog title={m.checkoutBlockedTitle}
      body={m.checkoutBlockedBody(blocked.conflictingPaths.length, blocked.targetBranch)}
      confirmLabel={m.writeCommitMessage} onCancel={() => setBlocked(undefined)} onConfirm={() => {
        setPendingCheckout(blocked.targetBranch)
        setBlocked(undefined)
        setView('commit')
      }} />}
  </>

  function removeWorktree(worktree: GitWorktreeSummary) {
    return run(m.removingWorktree, async () => {
      const next = await request<GitRepositoryStatus>('git.worktree-remove', { worktreeId: worktree.worktreeId })
      setNotice(worktree.dirty ? m.worktreeKept : m.worktreeRemoved)
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

function errorText(reason: unknown, m: HudMessages): string {
  return reason instanceof Error ? reason.message : m.gitActionFailed
}

function compactPath(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/').filter(Boolean)
  return parts.length <= 3 ? path : `…/${parts.slice(-3).join('/')}`
}
