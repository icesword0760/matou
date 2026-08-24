import type { LayoutNode } from '@matou/domain'
import type { ReactNode } from 'react'

import { SplitDivider } from './SplitDivider'

export function SplitTree({ root, renderMount, onRatio = () => {} }: {
  root: LayoutNode; renderMount(mountId: string): ReactNode; onRatio?(nodeId: string, ratio: number): void
}) {
  if (root.kind === 'mount') return <div className="split-mount" data-mount-id={root.mountId}>{renderMount(root.mountId)}</div>
  return <div className={`split-node ${root.direction}`}>
    {root.children.map((child, index) => <div className="split-child" key={child.id}>
      <SplitTree root={child} renderMount={renderMount} onRatio={onRatio} />
      {index < root.children.length - 1 && <SplitDivider direction={root.direction} onRatio={(ratio) => onRatio(root.id, ratio)} />}
    </div>)}
  </div>
}
