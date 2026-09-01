import { describe, expect, it } from 'vitest'

import type { SessionGraphView } from '../hierarchy/hierarchy-types'
import { activeAppSessionCount } from './active-app-sessions'

describe('activeAppSessionCount', () => {
  it('counts unique live starting, running and needs-input sessions across scenes', () => {
    const node = (sessionId: string, workStatus: SessionGraphView['nodes'][number]['workStatus'], archivedAt?: number) => ({
      sessionId, sceneId: 'scene-a', currentMode: 'claude-code' as const, workStatus,
      providerRestoreState: 'none' as const, canFork: true, title: sessionId, cwd: '/tmp',
      activeChildCount: 0, stoppedChildCount: 0, childModeCounts: { shell: 0, claudeCode: 0 },
      latestLines: [], lastUserInteractionSeq: 0, ...(archivedAt === undefined ? {} : { archivedAt })
    })
    const graphs: Record<string, SessionGraphView> = {
      a: { sceneId: 'a', edges: [], nodes: [node('one', 'starting'), node('two', 'running'), node('three', 'needs-input'), node('idle', 'idle')] },
      b: { sceneId: 'b', edges: [], nodes: [node('two', 'running'), node('archived', 'running', 10), node('failed', 'error')] }
    }
    expect(activeAppSessionCount(graphs)).toBe(3)
  })

  it('returns zero when no session graph is available', () => {
    expect(activeAppSessionCount(undefined)).toBe(0)
  })
})
