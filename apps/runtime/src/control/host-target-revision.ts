import { createHash } from 'node:crypto'

import type { HostTarget } from './host-control-types'

/** Hashes every stable field that can change a position-based target selector. */
export function hostTargetRevision(targets: readonly HostTarget[]): string {
  return createHash('sha256')
    .update(JSON.stringify(targets.map((target) => ({
      ref: target.ref,
      workspaceId: target.workspaceId,
      taskId: target.taskId,
      sessionId: target.sessionId,
      mountId: target.mountId,
      parentRef: target.dag.parentRef ?? null,
      childRefs: target.dag.childRefs
    }))))
    .digest('hex')
}
