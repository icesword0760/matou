import { describe, expect, it } from 'vitest'

import type { HostImpactSummary } from './host-action-types'
import {
  HostActionConfirmationError,
  HostActionConfirmationService
} from './host-action-confirmation-service'
import type { HostCallerIdentity } from './host-control-types'

const caller: HostCallerIdentity = { runId: 'run-parent', sessionId: 'session-parent' }
const otherCaller: HostCallerIdentity = { runId: 'run-other', sessionId: 'session-other' }

const impact: HostImpactSummary = {
  target: {
    window: { ref: 'window:window-1', title: 'Main' },
    workspace: { ref: 'workspace:workspace-1', title: 'Workspace', path: '/workspace' },
    task: { ref: 'task:task-1', title: 'Task' },
    canvas: { ref: 'canvas:scene-1', title: 'Canvas' },
    session: { ref: 'session:session-1', title: 'Shell' }
  },
  scope: 'subtree',
  tasks: 1,
  canvases: 1,
  sessions: 2,
  descendants: 1,
  liveRuns: 1,
  terminalProcesses: 1,
  preservesProjectFiles: true,
  preservesBranches: true,
  preservesWorktrees: true
}

function issueInput(overrides: Partial<{
  caller: HostCallerIdentity
  action: 'remove' | 'canvas-close'
  targetRef: string
  scope: 'node' | 'subtree'
  projectionRevision: string
  impact: HostImpactSummary
  now: number
}> = {}) {
  return {
    caller,
    action: 'remove' as const,
    targetRef: 'session:session-parent',
    scope: 'subtree' as const,
    projectionRevision: 'r1',
    impact,
    now: 1_000,
    ...overrides
  }
}

describe('HostActionConfirmationService', () => {
  it('generates 24-byte base64url references by default', () => {
    const service = new HostActionConfirmationService()
    const ref = service.issue(issueInput())

    expect(ref).toMatch(/^[A-Za-z0-9_-]{32}$/)
    expect(Buffer.from(ref, 'base64url')).toHaveLength(24)
  })

  it('binds a confirmation to caller, action, target, revision and impact hash', () => {
    const service = new HostActionConfirmationService({ randomRef: () => 'confirmation-ref' })
    const ref = service.issue(issueInput())

    expect(() => service.consume({
      ...issueInput({ caller: otherCaller, now: 1_100 }), ref
    })).toThrowError(expect.objectContaining({ code: 'CONFIRMATION_REQUIRED' }))

    expect(service.consume({ ...issueInput({ now: 1_100 }), ref })).toMatchObject({
      caller,
      action: 'remove',
      targetRef: 'session:session-parent',
      scope: 'subtree',
      projectionRevision: 'r1',
      impact,
      impactHash: expect.stringMatching(/^[a-f0-9]{64}$/),
      expiresAt: 121_000
    })

    expect(() => service.consume({ ...issueInput({ now: 1_200 }), ref }))
      .toThrowError(expect.objectContaining({ code: 'CONFIRMATION_REQUIRED' }))
  })

  it('expires at exactly the 120 second TTL', () => {
    const service = new HostActionConfirmationService({ randomRef: () => 'confirmation-ref' })
    const ref = service.issue(issueInput({ now: 5_000 }))

    expect(() => service.consume({ ...issueInput({ now: 124_999 }), ref }))
      .not.toThrow()

    const serviceAtBoundary = new HostActionConfirmationService({ randomRef: () => 'confirmation-ref' })
    const ref2 = serviceAtBoundary.issue(issueInput({ now: 10_000 }))
    expect(() => serviceAtBoundary.consume({ ...issueInput({ now: 130_000 }), ref: ref2 }))
      .toThrowError(expect.objectContaining({ code: 'CONFIRMATION_EXPIRED' }))
  })

  it('rejects changed revision and changed impact as stale', () => {
    const service = new HostActionConfirmationService({ randomRef: () => 'confirmation-ref' })
    const ref = service.issue(issueInput())

    expect(() => service.consume({ ...issueInput({ projectionRevision: 'r2', now: 1_100 }), ref }))
      .toThrowError(expect.objectContaining({ code: 'CONFIRMATION_STALE' }))

    expect(() => service.consume({
      ...issueInput({ impact: { ...impact, sessions: 99 }, now: 1_100 }), ref
    })).toThrowError(expect.objectContaining({ code: 'CONFIRMATION_STALE' }))
  })

  it('canonicalizes impact objects independent of key insertion order', () => {
    const service = new HostActionConfirmationService({ randomRef: () => 'confirmation-ref' })
    const reorderedImpact = {
      preservesWorktrees: true,
      preservesBranches: true,
      preservesProjectFiles: true,
      terminalProcesses: 1,
      liveRuns: 1,
      descendants: 1,
      sessions: 2,
      canvases: 1,
      tasks: 1,
      scope: 'subtree' as const,
      target: {
        session: { title: 'Shell', ref: 'session:session-1' },
        canvas: { title: 'Canvas', ref: 'canvas:scene-1' },
        task: { title: 'Task', ref: 'task:task-1' },
        workspace: { path: '/workspace', title: 'Workspace', ref: 'workspace:workspace-1' },
        window: { title: 'Main', ref: 'window:window-1' }
      }
    } satisfies HostImpactSummary
    const first = service.issue(issueInput())
    expect(first).toBe('confirmation-ref')
    const firstRecord = service.consume({ ...issueInput(), ref: first })
    const second = new HostActionConfirmationService({ randomRef: () => 'confirmation-ref-2' })
    const ref = second.issue(issueInput({ impact: reorderedImpact }))
    const record = second.consume({ ...issueInput(), ref })
    expect(record.impactHash).toBe(firstRecord.impactHash)
  })

  it('revokes all confirmations for a run', () => {
    const service = new HostActionConfirmationService({
      randomRef: (() => {
        let n = 0
        return () => `confirmation-${++n}`
      })()
    })
    const first = service.issue(issueInput())
    const second = service.issue(issueInput({ action: 'canvas-close' }))
    service.issue(issueInput({ caller: otherCaller }))

    service.revokeRun(caller.runId)
    expect(() => service.consume({ ...issueInput(), ref: first }))
      .toThrowError(expect.objectContaining({ code: 'CONFIRMATION_REQUIRED' }))
    expect(() => service.consume({ ...issueInput({ action: 'canvas-close' }), ref: second }))
      .toThrowError(HostActionConfirmationError)
    expect(service.consume({ ...issueInput({ caller: otherCaller }), ref: 'confirmation-3' })).toBeDefined()
  })
})
