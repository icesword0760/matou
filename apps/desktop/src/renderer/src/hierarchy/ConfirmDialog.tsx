export function ConfirmDialog(props: { title: string; body: string; confirmLabel: string; onConfirm(): void; onCancel(): void }) {
  return <div role="alertdialog" aria-modal="true" aria-label={props.title}>
    <h2>{props.title}</h2><p>{props.body}</p>
    <button onClick={props.onConfirm}>{props.confirmLabel}</button><button onClick={props.onCancel}>取消</button>
  </div>
}
