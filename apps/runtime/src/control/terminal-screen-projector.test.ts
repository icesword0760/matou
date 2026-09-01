import { describe, expect, it } from 'vitest'

import { TerminalScreenProjector } from './terminal-screen-projector'

describe('TerminalScreenProjector', () => {
  it('returns the latest terminal viewport after cursor movement and clearing', async () => {
    const screen = new TerminalScreenProjector(12, 3)
    await screen.write('first\r\nsecond')
    await screen.write('\rREPLACED')
    expect((await screen.snapshot()).text).toContain('REPLACED')
    expect((await screen.snapshot()).text).not.toContain('second')

    await screen.write('\u001b[2J\u001b[Hclean')
    expect(await screen.snapshot()).toMatchObject({ text: 'clean', cols: 12, rows: 3 })
  })

  it('tracks resize, scrollback and wide characters without ANSI decoration', async () => {
    const screen = new TerminalScreenProjector(8, 2)
    await screen.write('\u001b[31m红色\u001b[0m\r\nline2\r\nline3')
    expect((await screen.snapshot()).text).toBe('line2\nline3')

    await screen.resize(12, 3)
    await screen.write('\r\n最后一行')
    expect(await screen.snapshot()).toMatchObject({
      text: expect.stringContaining('最后一行'), cols: 12, rows: 3
    })
  })

  it('serializes concurrent writes in arrival order', async () => {
    const screen = new TerminalScreenProjector(20, 2)
    await Promise.all([screen.write('one'), screen.write('-two'), screen.write('-three')])
    expect((await screen.snapshot()).text).toBe('one-two-three')
  })
})
