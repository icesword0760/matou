// Frozen journal contracts from actual source, on disposable paths only.
import { mkdtemp, readFile, writeFile, readdir, mkdir, copyFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'
import { SegmentJournal, readSegmentFrames, repairSegmentTail, JournalCorruptionError } from '../../apps/runtime/src/journal/segment-journal'
import { JournalTailIndex } from '../../apps/runtime/src/journal/journal-tail-index'
import { selectCompressionCandidates } from '../../apps/runtime/src/journal/journal-policy'

const root = await mkdtemp(join(tmpdir(), 'matou-journal-contract-'))
const journal = await SegmentJournal.open(root, 'frames', { maxSegmentBytes: 512, compressSealed: false })
await journal.appendOutput(1, new TextEncoder().encode('first 中文\r\n'))
await journal.appendResize(2, 80, 24)
await journal.appendReset(3, 2)
await journal.appendEncoding(4, 'utf-8')
await journal.appendDomainCursor(5, 8)
await journal.appendOutput(6, Uint8Array.from([0xf0, 0x9f]))
await journal.appendOutput(7, Uint8Array.from([0x99, 0x82, 13, 10]))
await journal.appendExit(8, 7, 15)
await journal.close()
const dir = join(root, 'journal/frames')
const normalize = (frame: any) => frame.kind === 'output' ? { ...frame, data: [...frame.data] } : frame
const segments = []
const records = []
for (const name of (await readdir(dir)).filter(n => n.endsWith('.mtj')).sort()) {
  const bytes = await readFile(join(dir, name))
  const frames = (await readSegmentFrames(join(dir, name))).map(normalize)
  let offset = 8, frameIndex = 0
  while (offset < bytes.length) {
    const bodySize = bytes.readUInt32BE(offset), headerSize = bytes.readUInt32BE(offset + 8)
    const header = JSON.parse(bytes.subarray(offset + 12, offset + 12 + headerSize).toString())
    records.push({ timestamp: header.timestamp, frame: frames[frameIndex++] })
    offset += 8 + bodySize
  }
  const gz = gzipSync(bytes)
  const gzipPath = join(dir, `${name}.gz`); await writeFile(gzipPath, gz)
  segments.push({ name, base64: bytes.toString('base64'), gzipBase64: gz.toString('base64'), frames,
    gzipFrames: (await readSegmentFrames(gzipPath)).map(normalize) })
}
const first = Buffer.from(segments[0].base64, 'base64')
const corruptions = []
for (const kind of ['torn-prefix', 'torn-body', 'final-checksum', 'middle-checksum', 'invalid-magic']) {
  let bytes = Buffer.from(first)
  if (kind === 'torn-prefix') bytes = Buffer.concat([bytes, Buffer.from([0, 0, 1])])
  if (kind === 'torn-body') bytes = bytes.subarray(0, bytes.length - 3)
  if (kind === 'final-checksum') bytes[bytes.length - 1] ^= 1
  if (kind === 'middle-checksum') bytes[20] ^= 1
  if (kind === 'invalid-magic') bytes[0] ^= 1
  const path = join(root, `${kind}.mtj`); await writeFile(path, bytes)
  let strictError: any
  try { await readSegmentFrames(path) } catch (e) { const error = e as JournalCorruptionError; strictError = { message: error.message, offset: error.offset } }
  let repair: any, repairError: any, repairedBase64: string | undefined
  try { repair = await repairSegmentTail(path); repairedBase64 = (await readFile(path)).toString('base64') }
  catch (e) { const error = e as JournalCorruptionError; repairError = { message: error.message, offset: error.offset, quarantined: !!error.quarantinePath } }
  corruptions.push({ kind, base64: bytes.toString('base64'), strictError, repair, repairError, repairedBase64 })
}
const tailCases = []
const series = [
  { name: 'control-and-split-alternate', max: 3, inputs: [
    { sequence: 1, text: '', domainEventSequence: 3 },
    { sequence: 2, text: 'primary1\n\x1b[?10' }, { sequence: 3, text: '49hfull\nscreen\n' },
    { sequence: 4, text: '\x1b[?1049lprimary2\n\x1b]0;ignored\n\x07' },
    { sequence: 5, text: '\x1bPignored\n\x1b\\primary3\nprimary4\n', domainEventSequence: 9 },
  ] },
  { name: 'sparse-checkpoints', max: 10000, inputs: Array.from({ length: 513 }, (_, i) => ({ sequence: i + 1, text: 'x\n' })) },
  { name: 'bounded-tail', max: 10000, inputs: [{ sequence: 1, text: 'one\n'.repeat(10000) }, { sequence: 2, text: 'last\n' }, { sequence: 3, text: 'next' }] },
]
for (const seriesCase of series) {
  let index = new JournalTailIndex(seriesCase.max)
  const inputs = []
  for (const [i, input] of seriesCase.inputs.entries()) {
    const domain = 'domainEventSequence' in input ? input.domainEventSequence : undefined
    const data = [...new TextEncoder().encode(input.text)]
    index.record(input.sequence, Uint8Array.from(data), domain)
    if (i === 1) index = JournalTailIndex.fromSnapshot(index.snapshot(2))
    inputs.push({ sequence: input.sequence, data, ...(domain === undefined ? {} : { domainEventSequence: domain }) })
  }
  tailCases.push({ name: seriesCase.name, maximum: seriesCase.max, inputs, snapshot: index.snapshot(2),
    tailStart: index.tailStart(seriesCase.max), domainAtFirst: index.domainEventSequenceAtOrBefore(1), domainAtLast: index.domainEventSequenceAtOrBefore(999999) })
}
const candidates = [
  { index: 1, path: 'segment-000001.mtj', bytes: 100, state: 'sealed-raw' as const },
  { index: 2, path: 'segment-000002.mtj', bytes: 100, state: 'sealed-raw' as const, checkpointProtected: true },
  { index: 3, path: 'segment-000003.mtj', bytes: 100, state: 'sealed-raw' as const },
  { index: 4, path: 'segment-000004.mtj', bytes: 80, state: 'active' as const },
]
const fixture = { source: 'Frozen SegmentJournal/JournalTailIndex source on captured Electron; captured timestamps for byte-exact replay; gzip compressed bytes are test inputs, not encoder byte-equality expectations.',
  records, segments, corruptions, tailCases, compressionPolicy: { candidates, rawHotBytes: 180, expected: selectCompressionCandidates(candidates, 180) },
  tailSnapshot: JSON.parse(await readFile(join(dir, 'tail-index.json'), 'utf8')),
  rangeIndex: JSON.parse(await readFile(join(dir, 'range-index.json'), 'utf8')) }
await writeFile('apps/native/core/tests/fixtures/journal-reference.json', JSON.stringify(fixture) + '\n')
console.log(JSON.stringify({ root, records: records.length, segments: segments.length, corruptions: corruptions.length, tailCases: tailCases.length }))
