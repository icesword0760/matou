import { useState } from 'react'
import { createPortal } from 'react-dom'

export function RenameDialog(props: {
  label: string; initialValue: string; error?: (value: string) => string | undefined
  title?: string; placeholder?: string; emptyError?: string
  scope?: 'viewport' | 'session'
  onConfirm(value: string): void; onCancel(): void
}) {
  const [value, setValue] = useState(props.initialValue)
  const [emptySubmitted, setEmptySubmitted] = useState(false)
  const [composing, setComposing] = useState(false)
  const trimmed = value.trim()
  const error = emptySubmitted && !trimmed ? props.emptyError : props.error?.(trimmed)
  const confirm = () => {
    if (composing) return
    if (!trimmed) { setEmptySubmitted(true); return }
    if (!error) props.onConfirm(trimmed)
  }
  const overlay = <div className={`dialog-overlay${props.scope === 'session' ? ' is-session-scoped' : ''}`} onPointerDown={(event) => {
    if (event.currentTarget === event.target) props.onCancel()
  }}><div className="rename-dialog" role="dialog" aria-modal="true">
    <header><h2>{props.title ?? '重命名'}</h2><button className="dialog-close" aria-label="关闭" onClick={props.onCancel}>×</button></header>
    <div className="rename-dialog-body"><label>
      <span className="visually-hidden">{props.label}</span>
      <span className="rename-input-wrap"><input autoFocus maxLength={20} aria-label={props.label}
        placeholder={props.placeholder ?? `请输入${props.label}`}
        value={value} onChange={(event) => { setValue(event.target.value); setEmptySubmitted(false) }}
        onCompositionStart={() => setComposing(true)} onCompositionEnd={() => setComposing(false)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') props.onCancel()
          if (event.key === 'Enter') confirm()
        }} /><span>{value.length}/20</span></span>
    </label>{error && <p role="alert">{error}</p>}</div>
    <footer><button onClick={props.onCancel}>取消</button>
      <button className="dialog-primary" onClick={confirm}
        disabled={Boolean(error) && Boolean(trimmed)}>确定</button></footer>
  </div></div>
  if (props.scope !== 'session') return createPortal(overlay, document.body)
  const sessionCanvas = document.querySelector<HTMLElement>('.scene-stage:not([hidden]) .session-canvas') ??
    document.querySelector<HTMLElement>('.session-canvas')
  return sessionCanvas ? createPortal(overlay, sessionCanvas) : overlay
}
