import { describe, expect, it } from 'vitest'

import { ClaudePermissionModeTracker } from './claude-permission-mode-tracker'

describe('ClaudePermissionModeTracker', () => {
  it('follows the latest visible Claude permission footer across ANSI redraws and split chunks', () => {
    const tracker = new ClaudePermissionModeTracker()

    expect(tracker.ingest('\u001b[2K▶▶ auto mode on (shift+tab to cycle)')).toBe('auto')
    expect(tracker.ingest('\u001b[2K▶▶ bypass permis')).toBeUndefined()
    expect(tracker.ingest('sions on (shift+tab to cycle) · ← for agents')).toBe('bypassPermissions')
  })

  it('does not treat ordinary conversation text as a permission footer', () => {
    const tracker = new ClaudePermissionModeTracker()

    expect(tracker.ingest('Auto mode lets Claude handle permission prompts automatically.')).toBeUndefined()
    expect(tracker.ingest('Please document bypass permissions for the team.')).toBeUndefined()
  })
})
