export function SplitDivider({ direction, onRatio }: {
  direction: 'horizontal' | 'vertical'; onRatio(ratio: number): void
}) {
  return <div role="separator" aria-orientation={direction === 'horizontal' ? 'vertical' : 'horizontal'}
    className={`split-divider ${direction}`}
    onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)}
    onPointerMove={(event) => {
      if (event.buttons !== 1) return
      const parent = event.currentTarget.closest('.split-node')?.getBoundingClientRect()
      if (!parent) return
      const raw = direction === 'horizontal'
        ? (event.clientX - parent.left) / parent.width
        : (event.clientY - parent.top) / parent.height
      onRatio(Math.max(0.1, Math.min(0.9, raw)))
    }} />
}
