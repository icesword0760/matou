import { describe, expect, it } from 'vitest'

import { TerminalScreenProjector } from './terminal-screen-projector'

describe('TerminalScreenProjector', () => {
  it('restores cursor and wrapping before applying later incremental output', async () => {
    const original = new TerminalScreenProjector(12, 4)
    const restored = new TerminalScreenProjector(12, 4)
    await original.write('中文内容12345\r\nprogress: 1')
    const snapshot = await original.serialize()
    await restored.write(snapshot.snapshot)
    const update = '\u001b[1A\r\u001b[2Kupdated\u001b[1B\rprogress: 2'
    await Promise.all([original.write(update), restored.write(update)])
    expect(await restored.snapshot()).toEqual(await original.snapshot())
    original.dispose()
    restored.dispose()
  })

  it('captures a precise queued boundary without consuming subsequent output', async () => {
    const screen = new TerminalScreenProjector(20, 4)
    void screen.write('before')
    const capture = screen.serialize()
    void screen.write('-after')
    const restored = new TerminalScreenProjector(20, 4)
    await restored.write((await capture).snapshot)
    expect((await restored.snapshot()).text).toBe('before')
    expect((await screen.snapshot()).text).toBe('before-after')
    screen.dispose()
    restored.dispose()
  })

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
