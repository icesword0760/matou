import { useState } from 'react'

export function RenameDialog(props: {
  label: string; initialValue: string; error?: (value: string) => string | undefined
  onConfirm(value: string): void; onCancel(): void
}) {
  const [value, setValue] = useState(props.initialValue)
  const error = props.error?.(value.trim())
  return <div role="dialog" aria-modal="true">
    <label>{props.label}<input aria-label={props.label} value={value} onChange={(event) => setValue(event.target.value)} /></label>
    {error && <p role="alert">{error}</p>}
    <button onClick={() => props.onConfirm(value.trim())} disabled={!value.trim() || Boolean(error)}>确认</button>
    <button onClick={props.onCancel}>取消</button>
  </div>
}
