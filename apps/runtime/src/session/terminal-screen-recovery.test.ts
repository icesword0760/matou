import { Terminal } from '@xterm/headless'
import { expect, it } from 'vitest'

import { terminalRunBoundary } from './terminal-screen-recovery'

it('keeps the old viewport while returning input and drawing modes to the new-process baseline', async () => {
  const terminal = new Terminal({ cols: 20, rows: 4, allowProposedApi: true })
  const write = (data: string) => new Promise<void>(resolve => terminal.write(data, resolve))
  try {
    await write('OLD FIRST\r\nOLD LAST')
    await write('\x1b[?1000;1004;2004;1h\x1b[?7;25l')
    expect(terminal.modes.mouseTrackingMode).toBe('vt200')
    await write(terminalRunBoundary(4))
    await write('\x1b[2J\x1b[HNEW PROCESS')
    expect(terminal.modes).toMatchObject({
      mouseTrackingMode: 'none', bracketedPasteMode: false,
      applicationCursorKeysMode: false, sendFocusMode: false,
      wraparoundMode: true, originMode: false
    })
    const buffer = terminal.buffer.normal
    const history = Array.from({ length: buffer.length }, (_, row) => buffer.getLine(row)?.translateToString(true)).join('\n')
    expect(history).toContain('OLD FIRST')
    expect(history).toContain('OLD LAST')
    expect(history).toContain('NEW PROCESS')
    expect(buffer.baseY).toBeGreaterThan(0)
  } finally { terminal.dispose() }
})
