import { useEffect, useRef, useState } from 'react'

export interface BranchDialogSubmit {
  name: string
  worktreeMode: 'current' | 'new'
  submissionKey: string
}

export function BranchDialog(props: {
  relationMode: 'child' | 'sibling'
  sourceTitle: string
  gitAvailable: boolean
  onCancel(): void
  onConfirm(input: BranchDialogSubmit): Promise<unknown> | unknown
}) {
  const { relationMode, sourceTitle, gitAvailable, onCancel, onConfirm } = props
  const [name, setName] = useState('')
  const [worktreeMode, setWorktreeMode] = useState<'current' | 'new'>('current')
  const [error, setError] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const submittingRef = useRef(false)
  const submissionKeyRef = useRef<string | null>(null)
  const submissionKey = submissionKeyRef.current ?? crypto.randomUUID()
  submissionKeyRef.current = submissionKey
  const inputRef = useRef<HTMLInputElement>(null)
  const title = relationMode === 'child' ? '创建子会话分支' : '创建同级分支'

  useEffect(() => { inputRef.current?.focus() }, [])

  const submit = async () => {
    if (submittingRef.current) return
    const displayName = name.trim()
    if (!displayName) {
      setError('请输入分支名称')
      inputRef.current?.focus()
      return
    }
    if ([...displayName].length > 64) {
      setError('分支名称最多 64 个字符')
      inputRef.current?.focus()
      return
    }
    submittingRef.current = true
    setSubmitting(true)
    setError('')
    try {
      await onConfirm({
        name: displayName,
        worktreeMode,
        submissionKey
      })
    } catch (cause) {
      setError(errorMessage(cause))
      submittingRef.current = false
      setSubmitting(false)
      inputRef.current?.focus()
    }
  }

  return <div className="branch-dialog-overlay" onMouseDown={(event) => {
    if (event.target === event.currentTarget && !submitting) onCancel()
  }}>
    <section className="branch-dialog" role="dialog" aria-modal="true" aria-labelledby="branch-dialog-title">
      <header className="branch-dialog__header">
        <div>
          <h2 id="branch-dialog-title">{title}</h2>
          <p>从“{sourceTitle}”继续一条独立工作路径</p>
        </div>
        <button type="button" aria-label="关闭创建分支" disabled={submitting} onClick={onCancel}>×</button>
      </header>

      <label className="branch-dialog__field">
        <span>分支名称</span>
        <input ref={inputRef} aria-label="分支名称" value={name} maxLength={128}
          placeholder="例如：修复登录流程" disabled={submitting}
          onChange={(event) => { setName(event.target.value); setError('') }}
          onKeyDown={(event) => {
            if (event.key === 'Enter') { event.preventDefault(); void submit() }
            if (event.key === 'Escape' && !submitting) onCancel()
          }} />
        <small>{[...name.trim()].length}/64</small>
      </label>

      <fieldset className="branch-dialog__worktrees">
        <legend>工作目录</legend>
        <label className={`branch-worktree-card${worktreeMode === 'current' ? ' is-selected' : ''}`}>
          <input type="radio" name="worktree-mode" checked={worktreeMode === 'current'}
            disabled={submitting} onChange={() => setWorktreeMode('current')} />
          <span><strong>使用当前工作树</strong><small>和父会话使用同一目录，适合连续处理同一份改动</small></span>
        </label>
        <label className={`branch-worktree-card${worktreeMode === 'new' ? ' is-selected' : ''}${gitAvailable ? '' : ' is-disabled'}`}>
          <input type="radio" name="worktree-mode" checked={worktreeMode === 'new'}
            disabled={submitting || !gitAvailable} onChange={() => setWorktreeMode('new')} />
          <span><strong>从新工作树创建</strong><small>创建隔离的 Git worktree，适合多个功能并行开发</small>
            {!gitAvailable && <em>需要 Git 仓库</em>}</span>
        </label>
      </fieldset>

      {worktreeMode === 'new' && <p className="branch-dialog__notice">
        原目录中的未提交修改会保留在原处；新工作树从当前 HEAD 创建。
      </p>}
      {error && <p className="branch-dialog__error" role="alert">{error}</p>}
      {submitting && <p className="branch-dialog__progress" role="status">正在创建分支…</p>}

      <footer className="branch-dialog__footer">
        <button type="button" disabled={submitting} onClick={onCancel}>取消</button>
        <button type="button" className="primary" disabled={submitting} onClick={() => void submit()}>
          创建分支
        </button>
      </footer>
    </section>
  </div>
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
