import { describe, expect, it } from 'vitest'

import { CONTROL_KEY_SEQUENCES, TerminalInputQueue } from './terminal-input-queue'

describe('TerminalInputQueue', () => {
  it('keeps whole actions ordered for one session', async () => {
    const queue = new TerminalInputQueue()
    const output: string[] = []
    let release: () => void = () => {}
    const gate = new Promise<void>((resolve) => { release = resolve })

    const first = queue.enqueue('session-1', async () => {
      await gate
      output.push('first\r')
    })
    const second = queue.enqueue('session-1', () => { output.push('second\r') })
    await Promise.resolve()
    expect(output).toEqual([])
    release()
    await Promise.all([first, second])
    expect(output).toEqual(['first\r', 'second\r'])
  })

  it('lets different sessions progress independently', async () => {
    const queue = new TerminalInputQueue()
    let release: () => void = () => {}
    const gate = new Promise<void>((resolve) => { release = resolve })
    const output: string[] = []
    const blocked = queue.enqueue('session-1', () => gate)
    await queue.enqueue('session-2', () => { output.push('session-2') })
    expect(output).toEqual(['session-2'])
    release()
    await blocked
  })

  it('invalidates actions waiting behind a SessionRun that ended', async () => {
    const queue = new TerminalInputQueue()
    let release: () => void = () => {}
    const gate = new Promise<void>((resolve) => { release = resolve })
    const first = queue.enqueue('session-1', () => gate)
    await Promise.resolve()
    const waiting = queue.enqueue('session-1', () => undefined)
    queue.clear('session-1')
    release()
    await first
    await expect(waiting).rejects.toThrow('SessionRun ended before queued input was sent')
  })

  it('matches the reference product control-key allowlist exactly', () => {
    expect(CONTROL_KEY_SEQUENCES).toEqual({
      Enter: '\r', Tab: '\t', Escape: '\u001b', Backspace: '\u007f', Delete: '\u001b[3~',
      ArrowUp: '\u001b[A', ArrowDown: '\u001b[B', ArrowLeft: '\u001b[D', ArrowRight: '\u001b[C',
      Home: '\u001b[H', End: '\u001b[F', PageUp: '\u001b[5~', PageDown: '\u001b[6~',
      CtrlC: '\u0003', CtrlD: '\u0004', CtrlL: '\u000c', CtrlU: '\u0015', CtrlZ: '\u001a'
    })
  })
})
