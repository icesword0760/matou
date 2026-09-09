import { useEffect, useRef, useState } from 'react'

import { useMessages } from '../i18n/LocaleProvider'

export interface BranchDialogSubmit {
  name: string
  worktreeMode: 'current' | 'new'
  submissionKey: string
}

export function BranchDialog(props: {
  relationMode: 'child' | 'sibling' | 'peer'
  sourceTitle: string
  gitAvailable: boolean
  onCancel(): void
  onConfirm(input: BranchDialogSubmit): Promise<unknown> | unknown
}) {
  const messages = useMessages()
  const m = messages.sessionCanvas.branchDialog
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
  const title = m.title[relationMode]

  useEffect(() => { inputRef.current?.focus() }, [])

  const submit = async () => {
    if (submittingRef.current) return
    const displayName = name.trim()
    if (!displayName) {
      setError(m.nameRequired)
      inputRef.current?.focus()
      return
    }
    if ([...displayName].length > 64) {
      setError(m.nameTooLong)
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
          <p>{relationMode === 'peer'
            ? m.peerDescription(sourceTitle)
            : m.branchDescription(sourceTitle)}</p>
        </div>
        <button type="button" aria-label={m.close} disabled={submitting} onClick={onCancel}>×</button>
      </header>

      <label className="branch-dialog__field">
        <span>{m.name}</span>
        <input ref={inputRef} aria-label={m.name} value={name} maxLength={128}
          placeholder={m.namePlaceholder} disabled={submitting}
          onChange={(event) => { setName(event.target.value); setError('') }}
          onKeyDown={(event) => {
            if (event.key === 'Enter') { event.preventDefault(); void submit() }
            if (event.key === 'Escape' && !submitting) onCancel()
          }} />
        <small>{[...name.trim()].length}/64</small>
      </label>

      <fieldset className="branch-dialog__worktrees">
        <legend>{m.worktreeSection}</legend>
        <label className={`branch-worktree-card${worktreeMode === 'current' ? ' is-selected' : ''}`}>
          <input type="radio" name="worktree-mode" checked={worktreeMode === 'current'}
            disabled={submitting} onChange={() => setWorktreeMode('current')} />
          <span><strong>{m.useCurrentWorktree}</strong><small>{relationMode === 'peer'
            ? m.currentWorktreeHint.peer
            : m.currentWorktreeHint.branch}</small></span>
        </label>
        <label className={`branch-worktree-card${worktreeMode === 'new' ? ' is-selected' : ''}${gitAvailable ? '' : ' is-disabled'}`}>
          <input type="radio" name="worktree-mode" checked={worktreeMode === 'new'}
            disabled={submitting || !gitAvailable} onChange={() => setWorktreeMode('new')} />
          <span><strong>{m.useNewWorktree}</strong><small>{m.newWorktreeHint}</small>
            {!gitAvailable && <em>{m.needsGitRepository}</em>}</span>
        </label>
      </fieldset>

      {worktreeMode === 'new' && <p className="branch-dialog__notice">
        {m.newWorktreeNotice}
      </p>}
      {error && <p className="branch-dialog__error" role="alert">{error}</p>}
      {submitting && <p className="branch-dialog__progress" role="status">{m.creating}</p>}

      <footer className="branch-dialog__footer">
        <button type="button" disabled={submitting} onClick={onCancel}>{messages.common.cancel}</button>
        <button type="button" className="primary" disabled={submitting} onClick={() => void submit()}>
          {m.create}
        </button>
      </footer>
    </section>
  </div>
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
