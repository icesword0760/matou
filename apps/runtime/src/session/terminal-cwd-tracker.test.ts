import { describe, expect, it } from 'vitest'

import { TerminalCwdTracker } from './terminal-cwd-tracker'

describe('TerminalCwdTracker', () => {
  it('reads the latest OSC 7 working directory', () => {
    const tracker = new TerminalCwdTracker()
    expect(tracker.ingest('\u001b]7;file://host/tmp/first\u001b\\')).toBe('/tmp/first')
    expect(tracker.ingest('\u001b]7;file://host/tmp/space%20name\u0007')).toBe('/tmp/space name')
  })

  it('recognizes an OSC 7 sequence split across PTY chunks', () => {
    const tracker = new TerminalCwdTracker()
    expect(tracker.ingest('prefix\u001b]7;file://host/tmp/spl')).toBeUndefined()
    expect(tracker.ingest('it\u001b\\prompt')).toBe('/tmp/split')
  })

  it('ignores malformed and non-file OSC values', () => {
    const tracker = new TerminalCwdTracker()
    expect(tracker.ingest('\u001b]7;https://host/tmp\u0007')).toBeUndefined()
    expect(tracker.ingest('\u001b]7;broken\u0007')).toBeUndefined()
  })
})
