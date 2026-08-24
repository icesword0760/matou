import type { LayoutNode } from '@matou/domain'
import type { ReactNode } from 'react'

import { SplitDivider } from './SplitDivider'

export function SplitTree({ root, renderMount, ratios = {}, onRatio = () => {} }: {
  root: LayoutNode
  renderMount(mountId: string): ReactNode
  ratios?: Record<string, number>
  onRatio?(nodeId: string, ratio: number): void
}) {
  if (root.kind === 'mount') return <div className="split-mount" data-mount-id={root.mountId}>{renderMount(root.mountId)}</div>
  const ratio = ratios[root.id]
  return <div className={`split-node ${root.direction}`}>
    {root.children.map((child, index) => <div className="split-child" key={child.id}
      data-testid={`split-child-${root.id}-${index}`}
      style={index === 0 && ratio !== undefined ? { flexBasis: `${ratio * 100}%`, flexGrow: 0 } : undefined}>
      <SplitTree root={child} renderMount={renderMount} ratios={ratios} onRatio={onRatio} />
      {index < root.children.length - 1 && <SplitDivider direction={root.direction} onRatio={(ratio) => onRatio(root.id, ratio)} />}
    </div>)}
  </div>
}
