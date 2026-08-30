import { describe, expect, it } from 'vitest'

import { TerminalWorkStatusTracker } from './terminal-work-status-tracker'

describe('TerminalWorkStatusTracker', () => {
  it('maps real shell command boundaries to user-visible work states', () => {
    const tracker = new TerminalWorkStatusTracker()

    expect(tracker.ingest('\u001b]133;C\u0007')).toEqual(['running'])
    expect(tracker.ingest('output\r\n\u001b]133;D;0\u0007')).toEqual(['idle'])
    expect(tracker.ingest('\u001b]133;C\u0007\u001b]133;D;2\u0007')).toEqual(['running', 'error'])
    expect(tracker.ingest('\u001b]133;D;130\u0007')).toEqual(['interrupted'])
  })

  it('reassembles split OSC 133 frames without replaying an earlier state', () => {
    const tracker = new TerminalWorkStatusTracker()

    expect(tracker.ingest('text\u001b]133;D;')).toEqual([])
    expect(tracker.ingest('0\u001b\\prompt')).toEqual(['idle'])
    expect(tracker.ingest('more prompt')).toEqual([])
  })

  it('recognizes an explicit blocking Shell prompt without guessing from generic input calls', () => {
    const tracker = new TerminalWorkStatusTracker()

    expect(tracker.ingest('\u001b]133;C\u0007')).toEqual(['running'])
    expect(tracker.ingest("printf 'enter value: '; read -r value\r\n")).toEqual([])
    expect(tracker.ingest('enter ')).toEqual([])
    expect(tracker.ingest('value: ')).toEqual(['needs-input'])

    const generic = new TerminalWorkStatusTracker()
    expect(generic.ingest('\u001b]133;C\u0007')).toEqual(['running'])
    expect(generic.ingest("python3 -c 'input()'\r\n")).toEqual([])
  })

  it('recognizes common confirmation and secret prompts only at the live line boundary', () => {
    const tracker = new TerminalWorkStatusTracker()

    expect(tracker.ingest('Password: ')).toEqual(['needs-input'])
    expect(tracker.ingest('continue? [y/N] ')).toEqual(['needs-input'])
    expect(tracker.ingest('Password: accepted\r\n')).toEqual([])
  })
})
