import { useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'

import type { ConfirmStep } from './terminal-close-flow'

export function ConfirmDialog(props: {
  title: string
  body: ReactNode
  confirmLabel: string
  confirmTone?: 'default' | 'danger'
  cancelLabel?: string
  showCancel?: boolean
  scope?: 'viewport' | 'session'
  onConfirm(): void
  onCancel(): void
}) {
  const dialogRef = useRef<HTMLDivElement>(null)
  const [composing, setComposing] = useState(false)

  useEffect(() => {
    const dialog = dialogRef.current
    const firstControl = dialog?.querySelector<HTMLElement>('button, input')
    firstControl?.focus()
  }, [])

  const overlay = <div className={`dialog-overlay${props.scope === 'session' ? ' is-session-scoped' : ''}`} onPointerDown={(event) => {
    if (event.currentTarget === event.target) props.onCancel()
  }}><div ref={dialogRef} role="alertdialog" aria-modal="true" aria-label={props.title}
    onCompositionStart={() => setComposing(true)} onCompositionEnd={() => setComposing(false)}
    onKeyDown={(event) => {
      if (event.key === 'Escape' && !composing) props.onCancel()
      if (event.key !== 'Tab') return
      const controls = [...(dialogRef.current?.querySelectorAll<HTMLElement>('button, input') ?? [])]
      if (controls.length === 0) return
      const index = controls.indexOf(document.activeElement as HTMLElement)
      const next = event.shiftKey
        ? controls[(index - 1 + controls.length) % controls.length]
        : controls[(index + 1) % controls.length]
      event.preventDefault()
      next?.focus()
    }}>
    <header><h2>{props.title}</h2><button className="dialog-close" aria-label="关闭" onClick={props.onCancel}>×</button></header>
    {typeof props.body === 'string' ? <p>{props.body}</p> : <div className="dialog-content">{props.body}</div>}
    <footer>
      {props.showCancel !== false && <button onClick={props.onCancel}>{props.cancelLabel ?? '取消'}</button>}
      <button className={`dialog-primary${props.confirmTone === 'danger' ? ' is-danger' : ''}`}
        onClick={() => !composing && props.onConfirm()}>{props.confirmLabel}</button>
    </footer>
  </div></div>
  if (props.scope !== 'session') return overlay
  const sessionCanvas = document.querySelector<HTMLElement>('.scene-stage:not([hidden]) .session-canvas') ??
    document.querySelector<HTMLElement>('.session-canvas')
  return sessionCanvas ? createPortal(overlay, sessionCanvas) : overlay
}

export function ConfirmationSequence(props: {
  steps: ConfirmStep[]
  onComplete(): void
  onCancel(): void
}) {
  const [index, setIndex] = useState(0)
  const step = props.steps[index]
  if (!step) return null
  return <ConfirmDialog {...step} onCancel={props.onCancel} onConfirm={() => {
    if (index + 1 < props.steps.length) setIndex(index + 1)
    else props.onComplete()
  }} />
}
