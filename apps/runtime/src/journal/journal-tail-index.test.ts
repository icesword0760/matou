import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import {
  JournalTailIndex,
  loadJournalTailIndex,
  writeJournalTailIndex
} from './journal-tail-index'

const roots: string[] = []
const bytes = (value: string): Uint8Array => new TextEncoder().encode(value)

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('JournalTailIndex', () => {
  it('returns the journal start while fewer than the requested lines exist', () => {
    const index = new JournalTailIndex()
    index.record(5, bytes('first\nsecond'))

    expect(index.tailStart(10_000)).toBe(5)
  })

  it('points 10,001 one-frame lines at the start of the latest 10,000', () => {
    const index = new JournalTailIndex()
    for (let sequence = 1; sequence <= 10_001; sequence += 1) {
      index.record(sequence, bytes(`line-${sequence}\n`))
    }

    expect(index.tailStart()).toBe(2)
  })

  it('keeps dense output indexing below one Runtime long-task budget', () => {
    const index = new JournalTailIndex()
    const denseLines = bytes('x\n'.repeat(1_000))
    const startedAt = performance.now()

    for (let sequence = 1; sequence <= 200; sequence += 1) {
      index.record(sequence, denseLines)
    }

    expect(index.snapshot().completedLineCount).toBe(200_000)
    expect(index.tailStart()).toBe(191)
    expect(performance.now() - startedAt).toBeLessThan(50)
  })

  it('counts LF and split CRLF once while preserving Unicode split across frames', () => {
    const index = new JournalTailIndex(4)
    const emoji = bytes('😀')
    index.record(1, Uint8Array.from([...emoji.subarray(0, 2), 13]))
    index.record(2, Uint8Array.from([...emoji.subarray(2), 10]))
    index.record(3, bytes('二\n三\n'))
    index.record(4, bytes('四\n五\n'))

    expect(index.tailStart(4)).toBe(3)
    expect(index.snapshot().completedLineCount).toBe(5)
  })

  it('keeps a very long line as one line', () => {
    const index = new JournalTailIndex()
    index.record(1, new Uint8Array(20 * 1024 * 1024).fill(65))
    index.record(2, bytes('\nnext'))

    expect(index.tailStart()).toBe(1)
    expect(index.snapshot().completedLineCount).toBe(1)
  })

  it('ignores alternate-screen newlines even when control sequences cross frames', () => {
    const index = new JournalTailIndex(2)
    index.record(1, bytes('primary-1\n\u001b[?10'))
    index.record(2, bytes('49hfull\nscreen\n'))
    index.record(3, bytes('\u001b[?1049'))
    index.record(4, bytes('lprimary-2\nprimary-3\n'))

    expect(index.snapshot().completedLineCount).toBe(3)
    expect(index.tailStart(2)).toBe(4)
  })

  it('ignores DEC 47 alternate screen and DCS payload newlines', () => {
    const index = new JournalTailIndex(2)
    index.record(1, bytes('primary-1\n\u001b[?47halternate\nrow\n\u001b[?47l'))
    index.record(2, bytes('\u001bP$qpayload\nrow\u001b\\primary-2\nprimary-3\n'))

    expect(index.snapshot().completedLineCount).toBe(3)
    expect(index.tailStart(2)).toBe(2)
  })

  it('preserves a non-output Journal prefix as the tail start', () => {
    const index = new JournalTailIndex()
    index.record(1, new Uint8Array())
    index.record(2, new Uint8Array())
    index.record(3, bytes('first visible output'))

    expect(index.tailStart()).toBe(1)
    expect(index.snapshot()).toMatchObject({ firstSequence: 1, lastSequence: 3 })
  })

  it('emits sparse cumulative checkpoints every 256 recorded frames', () => {
    const index = new JournalTailIndex()
    for (let sequence = 1; sequence <= 513; sequence += 1) {
      index.record(sequence, bytes('x\n'))
    }

    expect(index.snapshot().sparse).toEqual([
      { sequence: 256, completedLineCount: 256 },
      { sequence: 512, completedLineCount: 512 }
    ])
  })

  it('writes an atomic sidecar and rejects a corrupt sidecar', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-tail-index-'))
    roots.push(root)
    const path = join(root, 'tail-index.json')
    const index = new JournalTailIndex()
    index.record(1, bytes('one\ntwo'))

    await writeJournalTailIndex(path, index.snapshot())
    await expect(loadJournalTailIndex(path)).resolves.toMatchObject({
      firstSequence: 1,
      lastSequence: 1,
      completedLineCount: 1
    })
    expect((await readFile(path, 'utf8')).includes('completedLineCount')).toBe(true)

    await writeFile(path, '{broken')
    await expect(loadJournalTailIndex(path)).rejects.toThrow('invalid Journal tail index')

    await writeFile(path, JSON.stringify({
      ...index.snapshot(),
      lastSequence: 2,
      lineStartSequences: [1, 999]
    }))
    await expect(loadJournalTailIndex(path)).rejects.toThrow('invalid Journal tail index')
  })

  it('rejects a decreasing output sequence', () => {
    const index = new JournalTailIndex()
    index.record(2, bytes('two'))
    expect(() => index.record(2, bytes('duplicate'))).toThrow('increase monotonically')
  })
})
