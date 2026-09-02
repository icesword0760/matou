import { mkdtemp, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { JournalCompressor } from './journal-compressor'
import { JournalHistoryReader } from './journal-history-reader'
import { SegmentJournal } from './segment-journal'

const roots: string[] = []

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('JournalHistoryReader', () => {
  it('paginates compressed and raw segments without duplicate or skipped lines', async () => {
    const { root, sessionId, directory } = await historyFixture(14)
    await new JournalCompressor().compress({
      sessionId, index: 1, path: join(directory, 'segment-000001.mtj')
    })
    const reader = new JournalHistoryReader(root)

    const newest = await reader.page({ sessionId, lineLimit: 5 })
    expect(newest.lines.map(({ text }) => text)).toEqual([
      'line-10', 'line-11', 'line-12', 'line-13', 'line-14'
    ])
    const older = await reader.page({
      sessionId, before: newest.lines[0]!.cursor, lineLimit: 5
    })
    expect(older.lines.map(({ text }) => text)).toEqual([
      'line-05', 'line-06', 'line-07', 'line-08', 'line-09'
    ])
    expect(new Set([...newest.lines, ...older.lines].map(({ cursor }) => JSON.stringify(cursor))).size)
      .toBe(10)
  })

  it('keeps cursor pagination exact when many lines share one output frame', async () => {
    const root = await makeRoot()
    const journal = await SegmentJournal.open(root, 'shared-frame', { compressSealed: false })
    await journal.appendOutput(1, new TextEncoder().encode('one\ntwo\nthree\nfour\nfive\n'))
    await journal.close()
    const reader = new JournalHistoryReader(root)

    const newest = await reader.page({ sessionId: 'shared-frame', lineLimit: 2 })
    const middle = await reader.page({ sessionId: 'shared-frame', before: newest.lines[0]!.cursor, lineLimit: 2 })
    const oldest = await reader.page({ sessionId: 'shared-frame', before: middle.lines[0]!.cursor, lineLimit: 2 })

    expect([...oldest.lines, ...middle.lines, ...newest.lines].map(({ text }) => text))
      .toEqual(['one', 'two', 'three', 'four', 'five'])
  })

  it('decodes UTF-8 characters split across output frames and segment boundaries', async () => {
    const root = await makeRoot()
    const sessionId = 'unicode-session'
    const journal = await SegmentJournal.open(root, sessionId, {
      maxSegmentBytes: 150,
      compressSealed: false
    })
    const bytes = new TextEncoder().encode('prefix-😀-target\n')
    await journal.appendOutput(1, bytes.subarray(0, 9))
    await journal.appendOutput(2, bytes.subarray(9, 12))
    await journal.appendOutput(3, bytes.subarray(12))
    await journal.close()

    const result = await new JournalHistoryReader(root).search({
      sessionId, query: '😀-target', limit: 10,
      options: { caseSensitive: true, regex: false, wholeWord: false }
    })

    expect(result.matches.map(({ text }) => text)).toEqual(['prefix-😀-target'])
  })

  it('enforces a hard 1000-line ceiling', async () => {
    const { root, sessionId } = await historyFixture(1_205, true)
    const page = await new JournalHistoryReader(root).page({ sessionId, lineLimit: 5_000 })

    expect(page.lines).toHaveLength(1_000)
    expect(page.lines[0]!.text).toBe('line-206')
    expect(page.lines.at(-1)!.text).toBe('line-1205')
    expect(page.hasMore).toBe(true)
  })

  it('isolates one damaged segment as a gap and keeps later history readable', async () => {
    const { root, sessionId, directory } = await historyFixture(30)
    await writeFile(join(directory, 'segment-000001.mtj'), 'damaged-segment')

    const result = await new JournalHistoryReader(root).page({ sessionId, lineLimit: 100 })

    expect(result.gaps).toEqual([
      expect.objectContaining({ segmentIndex: 1, code: 'CORRUPT_SEGMENT' })
    ])
    expect(result.lines.length).toBeGreaterThan(0)
    expect(result.lines.at(-1)!.text).toBe('line-30')
  })

  it('supports literal, case-sensitive, whole-word, and regular-expression search', async () => {
    const root = await makeRoot()
    const journal = await SegmentJournal.open(root, 'search-options', { compressSealed: false })
    await journal.appendOutput(1, new TextEncoder().encode('Token tokenized\nTOKEN token\n'))
    await journal.close()
    const reader = new JournalHistoryReader(root)

    const literal = await reader.search({
      sessionId: 'search-options', query: 'token', limit: 10,
      options: { caseSensitive: false, regex: false, wholeWord: true }
    })
    const regex = await reader.search({
      sessionId: 'search-options', query: '^TOKEN\\s+token$', limit: 10,
      options: { caseSensitive: true, regex: true, wholeWord: false }
    })

    expect(literal.matches.map(({ text }) => text)).toEqual(['TOKEN token', 'Token tokenized'])
    expect(regex.matches.map(({ text }) => text)).toEqual(['TOKEN token'])
  })

  it('keeps paging bounded while searching a real multi-megabyte terminal log', async () => {
    const root = await makeRoot()
    const sessionId = 'large-history'
    const journal = await SegmentJournal.open(root, sessionId, {
      maxSegmentBytes: 1024 * 1024,
      compressSealed: false
    })
    const encoder = new TextEncoder()
    let sequence = 0
    for (let batch = 0; batch < 24; batch += 1) {
      const lines = Array.from({ length: 5_000 }, (_, line) => {
        const ordinal = batch * 5_000 + line
        const marker = ordinal === 73_421 ? ' LARGE_ARCHIVE_MARKER' : ''
        return `${String(ordinal).padStart(6, '0')} realistic terminal output payload ${'x'.repeat(32)}${marker}\n`
      }).join('')
      await journal.appendOutput(++sequence, encoder.encode(lines))
    }
    await journal.close()
    const directory = join(root, 'journal', sessionId)
    const bytes = (await Promise.all((await readdir(directory))
      .filter((name) => name.endsWith('.mtj'))
      .map((name) => stat(join(directory, name))))).reduce((total, value) => total + value.size, 0)
    expect(bytes).toBeGreaterThan(5 * 1024 * 1024)

    const reader = new JournalHistoryReader(root)
    const page = await reader.page({ sessionId, lineLimit: 10_000 })
    const search = await reader.search({
      sessionId, query: 'LARGE_ARCHIVE_MARKER', limit: 10,
      options: { caseSensitive: true, regex: false, wholeWord: false }
    })

    expect(page.lines).toHaveLength(1_000)
    expect(page.lines.at(-1)?.text).toContain('119999')
    expect(search.matches).toEqual([
      expect.objectContaining({ text: expect.stringContaining('073421') })
    ])
  }, 30_000)
})

async function historyFixture(lineCount: number, singleFrame = false) {
  const root = await makeRoot()
  const sessionId = 'history-session'
  const journal = await SegmentJournal.open(root, sessionId, {
    maxSegmentBytes: singleFrame ? 1024 * 1024 : 180,
    compressSealed: false
  })
  if (singleFrame) {
    const text = Array.from({ length: lineCount }, (_, index) =>
      `line-${String(index + 1).padStart(2, '0')}\n`).join('')
    await journal.appendOutput(1, new TextEncoder().encode(text))
  } else {
    for (let index = 1; index <= lineCount; index += 1) {
      await journal.appendOutput(index, new TextEncoder().encode(`line-${String(index).padStart(2, '0')}\n`))
    }
  }
  await journal.close()
  return { root, sessionId, directory: join(root, 'journal', sessionId) }
}

async function makeRoot(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'matou-journal-history-'))
  roots.push(root)
  return root
}
