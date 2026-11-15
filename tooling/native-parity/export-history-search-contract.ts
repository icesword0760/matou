// Frozen archive reader outputs, recorded from disposable journal files.
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'
import { JournalHistoryReader } from '../../apps/runtime/src/journal/journal-history-reader'
import { SegmentJournal } from '../../apps/runtime/src/journal/segment-journal'

const root = await mkdtemp(join(tmpdir(), 'matou-history-contract-'))
const results: unknown[] = []
try {
  for (const variant of ['raw', 'compressed', 'gap']) {
    const sessionId = `history-${variant}`
    const journal = await SegmentJournal.open(root, sessionId, { maxSegmentBytes: 150, compressSealed: false })
    const frames = [
      Buffer.from('\ufefffirst\r\ncat42 cat42\nuntest test\n[test]\n中文测试\n中文 测试\n'),
      Buffer.from('\x1b[31mred\x1b[0m\r\n\x1b]0;title\x07visible\npartial-'),
      Buffer.from([0xf0, 0x9f]),
      Buffer.from([0x99, 0x82, 10]),
      Buffer.from('line-a\nline-b\nline-c\nlast-no-newline'),
    ]
    for (const [index, bytes] of frames.entries()) await journal.appendOutput(index + 1, bytes)
    await journal.close()
    const directory = join(root, 'journal', sessionId)
    const names = (await readdir(directory)).filter(name => name.endsWith('.mtj')).sort()
    if (variant === 'compressed') {
      for (const name of names) {
        await writeFile(join(directory, name + '.gz'), gzipSync(await readFile(join(directory, name))))
        await rm(join(directory, name))
      }
    }
    if (variant === 'gap') await rm(join(directory, names[1]!))
    const reader = new JournalHistoryReader(root)
    const probes: unknown[] = []
    const pageInputs = [
      {}, { lineLimit: 3 }, { lineLimit: 0 },
      { before: { sequence: 5, lineIndex: 1 }, lineLimit: 2 },
      { around: { sequence: 5, lineIndex: 1 }, beforeLines: 1, afterLines: 1 },
      { around: { sequence: 99, lineIndex: 0 } },
    ]
    for (const input of pageInputs) probes.push({ op: 'page', input, expected: await reader.page({ sessionId, ...input }) })
    const searchInputs = [
      { query: '' }, { query: 'line', limit: 2 },
      { query: 'LINE', before: { sequence: 5, lineIndex: 2 } },
      ...['test', '测试', 'cat', '(?<=cat)\\d+', '^', '['].map(query => ({ query, options: { caseSensitive: false, regex: query !== 'test' && query !== '测试', wholeWord: true } })),
      { query: '🙂' }, { query: 'red', options: { caseSensitive: true, regex: false, wholeWord: false } },
    ]
    for (const input of searchInputs) {
      try { probes.push({ op: 'search', input, expected: await reader.search({ sessionId, ...input }) }) }
      catch (error) { probes.push({ op: 'search', input, expected: { error: (error as Error).message } }) }
    }
    const segments = []
    for (const name of (await readdir(directory)).filter(name => /\.(mtj|bin)(\.gz)?$/.test(name)).sort()) {
      segments.push({ name, base64: (await readFile(join(directory, name))).toString('base64') })
    }
    results.push({ variant, sessionId, segments, probes })
  }
  const path = 'apps/native/runtime/tests/fixtures/history-search-reference.json'
  await mkdir('apps/native/runtime/tests/fixtures', { recursive: true })
  await writeFile(path, JSON.stringify({ source: 'Frozen JournalHistoryReader', cases: results }, null, 2) + '\n')
  console.log(JSON.stringify({ path, cases: results.length }))
} finally {
  await rm(root, { recursive: true, force: true })
}
