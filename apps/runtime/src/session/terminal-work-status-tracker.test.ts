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

  it('recognizes a real zsh read prompt without depending on Bash read syntax', () => {
    const tracker = new TerminalWorkStatusTracker()

    expect(tracker.ingest('\u001b]133;C\u0007')).toEqual(['running'])
    expect(tracker.ingest('STA008_WAIT> ')).toEqual(['needs-input'])
  })

  it('keeps a blocking prompt when one PTY chunk also closes the previous command', () => {
    const tracker = new TerminalWorkStatusTracker()

    expect(tracker.ingest(
      '\u001b]133;D;0\u0007\u001b]133;A\u0007\u001b]133;C\u0007STA008_WAIT> '
    )).toEqual(['idle', 'running', 'needs-input'])
  })

  it('leaves Claude failure text to the user instead of classifying the task', () => {
    const tracker = new TerminalWorkStatusTracker({ provider: 'claude-code' })

    expect(tracker.ingest('Retrying in 2s · attempt 9/10')).toEqual([])
    expect(tracker.ingest('\r\n✻ Connection refused — a firewall or proxy may be blocking it ')).toEqual([])
    expect(tracker.ingest('(ConnectionRefused) · Retrying in 34s · attempt 10/10')).toEqual([])
    expect(tracker.ingest('\r\nAPI Error: 529 Overloaded')).toEqual([])
    expect(tracker.ingest('\r\nauthentication failed')).toEqual([])
    expect(tracker.ingest('\r\nrate limit reached · final attempt')).toEqual([])
    expect(tracker.ingest('\r\n────────────────────\r\n❯ ')).toEqual([])

    const shell = new TerminalWorkStatusTracker()
    expect(shell.ingest('echo "Connection refused · attempt 10/10"')).toEqual([])
  })

  it('does not derive task failure from Claude full-screen repaint deltas', () => {
    const tracker = new TerminalWorkStatusTracker({ provider: 'claude-code' })

    expect(tracker.ingest(
      '\u001b[2D\u001b[3B\r\u001b[6A⏺\u001b[3GAPI Error:\u001b[14GConnection refused —' +
      '\u001b[35Ga firewall or proxy may be blocking it (ConnectionRefused)\u001b[K\r' +
      '\u001b[2B✻ Baked for 2m 50s · done 6:02 PM\u001b[K\r\u001b[2B❯ '
    )).toEqual([])
    expect(tracker.ingest('\u001b]133;D;0\u0007')).toEqual([])
  })
})
