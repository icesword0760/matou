import { describe, expect, it } from 'vitest'

import { ProviderResumeMonitor } from './provider-resume-monitor'

describe('ProviderResumeMonitor', () => {
  it('detects a missing provider session even when the failure spans output chunks', () => {
    const monitor = new ProviderResumeMonitor()

    expect(monitor.ingest('Error: no session fo')).toBeUndefined()
    expect(monitor.ingest('und for id provider-42\r\n')).toBe('provider session not found')
  })

  it('recognizes the failure wording used by supported AI CLIs', () => {
    for (const output of [
      'Session not found',
      'No conversation found for that session',
      'Invalid session identifier',
      'Error: failed to resume session'
    ]) {
      expect(new ProviderResumeMonitor().ingest(output)).toBe('provider session not found')
    }
  })

  it('recognizes current Claude Code failure text split by cursor-position control sequences', () => {
    const monitor = new ProviderResumeMonitor()

    expect(monitor.ingest(
      '\u001b[?25lNo\u001b[4Gconversation\u001b[17Gfound\u001b[23Gwith\u001b[28Gsession\u001b[36GID:' +
      '\u001b[40G00000000-0000-4000-8000-000000000000\r\n'
    )).toBe('provider session not found')
  })

  it('does not degrade a session for ordinary provider output', () => {
    const monitor = new ProviderResumeMonitor()

    expect(monitor.ingest('Reading session context...\r\nReady.')).toBeUndefined()
  })

  it('reports a resume failure when the provider remains unresponsive until the deadline', () => {
    const monitor = new ProviderResumeMonitor()
    const timeout = (monitor as unknown as { timeout?: () => string }).timeout

    expect(timeout?.call(monitor)).toBe('provider resume timed out')
    expect(monitor.isMonitoring).toBe(false)
  })

  it('stops treating later conversation text as startup failure after substantial resume output', () => {
    const monitor = new ProviderResumeMonitor()

    expect(monitor.ingest('a'.repeat(2_001))).toBeUndefined()
    expect(monitor.isMonitoring).toBe(false)
    expect(monitor.timeout()).toBeUndefined()
    expect(monitor.ingest('The user wrote: session not found in a log file')).toBeUndefined()
  })

  it('does not let a large startup frame hide a failure contained in that same frame', () => {
    const monitor = new ProviderResumeMonitor()

    expect(monitor.ingest(`${'a'.repeat(2_001)} No session found with session ID`))
      .toBe('provider session not found')
    expect(monitor.isSettled).toBe(false)
    expect(monitor.isMonitoring).toBe(false)
  })
})
