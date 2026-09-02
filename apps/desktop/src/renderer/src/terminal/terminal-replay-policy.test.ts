import { describe, expect, it } from 'vitest'

import { replayFromSequenceForSpawn, shouldRunReplayProbe } from './terminal-replay-policy'

describe('PRD 04 terminal replay policy', () => {
  it('replays only the current live Runtime run when a cached VT model reconnects', () => {
    expect(replayFromSequenceForSpawn({ reattached: true, replayFromSequence: 41 }, true, 'shell')).toBe(41)
  })

  it('continues a reusable VT model after its own visual watermark', () => {
    expect(replayFromSequenceForSpawn(
      { reattached: true, replayFromSequence: 1 }, true, 'shell', 41
    )).toBe(42)
  })

  it('keeps a fresh Shell model out of raw interrupted history', () => {
    expect(replayFromSequenceForSpawn({ reattached: true, replayFromSequence: 41 }, false, 'shell')).toBe(41)
  })

  it('rebuilds Agent history when a reattached Runtime finds a fresh VT model', () => {
    expect(replayFromSequenceForSpawn({ reattached: true, replayFromSequence: 41 }, false, 'claude-code')).toBe(0)
  })

  it('does not replay durable Shell history after an application restart', () => {
    expect(replayFromSequenceForSpawn({ reattached: false, replayFromSequence: 41 }, false, 'shell')).toBeUndefined()
    expect(replayFromSequenceForSpawn({ reattached: true }, false, 'shell')).toBeUndefined()
  })

  it('keeps the E2E replay probe out of user terminal panels', () => {
    expect(shouldRunReplayProbe('foundation-shell', true)).toBe(true)
    expect(shouldRunReplayProbe('session-user', true)).toBe(false)
    expect(shouldRunReplayProbe('session-user', true, true)).toBe(true)
    expect(shouldRunReplayProbe('foundation-shell', false)).toBe(false)
  })
})
