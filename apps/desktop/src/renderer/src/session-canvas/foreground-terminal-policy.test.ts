import { describe, expect, it } from 'vitest'

import type { SessionGraphNodeView } from '../hierarchy/hierarchy-types'
import { foregroundSiblingSessionIds } from './foreground-terminal-policy'

describe('foregroundSiblingSessionIds', () => {
  it('keeps every running sibling in the selected horizontal level, including offscreen siblings', () => {
    const nodes = [
      node('root-a'),
      node('root-b'),
      node('child-a', 'root-a'),
      node('child-b', 'root-a'),
      node('other-child', 'root-b')
    ]

    expect(foregroundSiblingSessionIds(nodes, 'root-a')).toEqual(['child-a', 'child-b'])
  })

  it('excludes stopped and summary-only nodes from foreground terminal ownership', () => {
    const stopped = node('stopped')
    stopped.archivedAt = 10
    const teamMember = node('team-member')
    teamMember.currentMode = 'agent-team-member'

    expect(foregroundSiblingSessionIds([node('shell'), stopped, teamMember], undefined)).toEqual(['shell'])
  })
})

function node(sessionId: string, parentSessionId?: string): SessionGraphNodeView {
  return {
    sessionId, sceneId: 'scene-1',
    ...(parentSessionId ? { parentSessionId } : {}),
    currentMode: 'shell', workStatus: 'idle', providerRestoreState: 'none', canFork: false,
    title: sessionId, cwd: '/tmp', activeChildCount: 0, stoppedChildCount: 0,
    childModeCounts: { shell: 0, claudeCode: 0 }, latestLines: [], lastUserInteractionSeq: 0
  }
}
