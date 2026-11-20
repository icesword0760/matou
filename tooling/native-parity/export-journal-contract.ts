// Frozen journal contracts from actual source, on disposable paths only.
import { mkdtemp, readFile, writeFile, readdir, mkdir, copyFile } from 'node:fs/promises'
import { basename } from 'node:path'
import { crc32, gunzipSync } from 'node:zlib'
import { defaultJournalCompressor } from '../../apps/runtime/src/journal/journal-compressor'
import { JournalHistoryReader } from '../../apps/runtime/src/journal/journal-history-reader'
import { iterateSessionFrames } from '../../apps/runtime/src/journal/journal-range-reader'
import { iterateSegmentFrames, type DecodedJournalFrame } from '../../apps/runtime/src/journal/segment-journal'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'
import { SegmentJournal, readSegmentFrames, repairSegmentTail, JournalCorruptionError } from '../../apps/runtime/src/journal/segment-journal'
import { JournalTailIndex } from '../../apps/runtime/src/journal/journal-tail-index'
import { selectCompressionCandidates } from '../../apps/runtime/src/journal/journal-policy'

if (process.env.MATOU_JOURNAL_GATE_RUN) {
  await exportGateCases(process.env.MATOU_JOURNAL_GATE_RUN)
  process.exit(0)
}

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

interface GateRecord { timestamp: number; frame: DecodedJournalFrame }
interface GateCodec { id: string; name: string; base64: string; repair: boolean }

function gateError(error: unknown) {
  if (!(error instanceof Error)) throw error
  return {
    type: error.name,
    message: error.message,
    ...(error instanceof JournalCorruptionError
      ? { offset: error.offset, quarantined: !!error.quarantinePath }
      : {}),
    ...('code' in error ? { code: error.code } : {})
  }
}

async function gateFrames(frames: AsyncIterable<DecodedJournalFrame>) {
  const readable = []
  let error = null
  try {
    for await (const frame of frames) {
      readable.push(frame.kind === 'output' ? { ...frame, data: [...frame.data] } : frame)
    }
  } catch (cause) {
    error = gateError(cause)
  }
  return { frames: readable, error }
}

async function gateWriter(root: string, id: string, records: GateRecord[], compressSealed = false) {
  const writer = await SegmentJournal.open(root, id, {
    maxSegmentBytes: 512, rawHotBytes: 1, compressSealed
  })
  for (const { timestamp, frame } of records) {
    Date.now = () => timestamp
    switch (frame.kind) {
      case 'output': await writer.appendOutput(frame.sequence, frame.data); break
      case 'resize': await writer.appendResize(frame.sequence, frame.cols, frame.rows); break
      case 'reset': await writer.appendReset(frame.sequence, frame.screenEpoch); break
      case 'encoding': await writer.appendEncoding(frame.sequence, frame.encoding); break
      case 'domain-cursor': await writer.appendDomainCursor(frame.sequence, frame.domainEventSequence); break
      case 'exit': await writer.appendExit(frame.sequence, frame.exitCode, frame.signal); break
    }
  }
  await writer.close()
  await defaultJournalCompressor.whenIdle()
  return writer.directory
}

async function exportGateCases(stage: string) {
  const clock = Date.now
  const now = 1_794_000_000_000
  const root = join(stage, 'frozen-data')
  const output = (sequence: number, data: Uint8Array): DecodedJournalFrame => ({ kind: 'output', sequence, data })
  const text = (value: string) => new TextEncoder().encode(value)
  const records = ([
    output(1, text('first 中文\r\n')),
    { kind: 'resize', sequence: 2, cols: 80, rows: 24 },
    { kind: 'reset', sequence: 3, screenEpoch: 2 },
    { kind: 'encoding', sequence: 4, encoding: 'utf-8' },
    { kind: 'domain-cursor', sequence: 5, domainEventSequence: 8 },
    output(6, Uint8Array.from([0xf0, 0x9f])),
    output(7, Uint8Array.from([0x99, 0x82, 13, 10])),
    { kind: 'exit', sequence: 8, exitCode: 7, signal: 15 }
  ] satisfies DecodedJournalFrame[]).map((frame, i) => ({ timestamp: now + i + 1, frame }))
  const jsonFile = (path: string, value: unknown) => writeFile(path, JSON.stringify(value) + '\n')
  const index = (directory: string, name: string) => readFile(join(directory, name))
    .then(bytes => {
      try { return JSON.parse(bytes.toString('utf8')) }
      catch (error) {
        if (!(error instanceof SyntaxError)) throw error
        return { invalidBase64: bytes.toString('base64') }
      }
    }, error => {
      if (error.code !== 'ENOENT') throw error
      return null
    })
  const firstName = 'segment-000001.mtj'
  const secondName = 'segment-000002.mtj'
  try {
    const rawDirectory = await gateWriter(root, 'raw', records)
    const rawNames = (await readdir(rawDirectory)).filter(name => name.endsWith('.mtj')).sort()
    const raw = await Promise.all(rawNames.map(async name => ({
      name, base64: (await readFile(join(rawDirectory, name))).toString('base64')
    })))
    if (rawNames.join(',') !== [firstName, secondName].join(',')) throw new Error('unexpected fixed segment split')
    const first = Buffer.from(raw[0].base64, 'base64')
    const second = Buffer.from(raw[1].base64, 'base64')
    const magic = first.subarray(0, 8)
    const gzip = gzipSync(first)
    const firstEnd = 16 + first.readUInt32BE(8)
    const domainEnd = 16 + second.readUInt32BE(8)
    const utf8Split = domainEnd + 8 + second.readUInt32BE(domainEnd)
    const change = (source: Buffer, offset: number) => {
      const bytes = Buffer.from(source)
      bytes[offset] ^= 1
      return bytes
    }
    const payloadMismatch = Buffer.from(first)
    const dataLength = payloadMismatch.indexOf('"dataLength":14')
    if (dataLength < 0 || dataLength >= firstEnd) throw new Error('unexpected first payload header')
    payloadMismatch[dataLength + '"dataLength":1'.length] = '3'.charCodeAt(0)
    payloadMismatch.writeUInt32BE(crc32(payloadMismatch.subarray(16, firstEnd)), 12)
    const lengthMismatch = Buffer.from(first.subarray(0, firstEnd))
    lengthMismatch.writeUInt32BE(lengthMismatch.readUInt32BE(8) + 1, 8)
    const zeroCrc = Buffer.from(first.subarray(0, firstEnd))
    zeroCrc.writeUInt32BE(0, 12)
    const middleCrc = change(first, 12)
    const finalCrc = change(first, first.length - 1)
    const tornBody = first.subarray(0, first.length - 3)
    const codec: GateCodec[] = []
    const add = (id: string, bytes: Buffer, compressed = false) => codec.push({
      id, name: id + (compressed ? '.mtj.gz' : '.mtj'), base64: bytes.toString('base64'), repair: !compressed
    })
    add('raw-valid-crc', first)
    add('gzip-valid', gzip, true)
    add('crc-wrong-final', finalCrc)
    add('crc-wrong-middle', middleCrc)
    add('crc-zero', zeroCrc)
    add('crc-missing-at-eof', Buffer.concat([first, first.subarray(8, 12)]))
    add('crc-field-omitted', Buffer.concat([first.subarray(0, 12), first.subarray(16, firstEnd)]))
    add('record-torn-prefix', Buffer.concat([first, Buffer.from([0, 0, 1])]))
    add('record-torn-body', tornBody)
    add('body-length-too-large-for-file', lengthMismatch)
    add('payload-length-mismatch-valid-crc', payloadMismatch)
    add('invalid-magic', change(first, 0))
    add('zero-byte-file', Buffer.alloc(0))
    add('magic-only-file', magic)
    add('gzip-invalid-header', change(gzip, 0), true)
    add('gzip-truncated-header', gzip.subarray(0, 5), true)
    add('gzip-truncated-trailer', gzip.subarray(0, gzip.length - 3), true)
    add('gzip-valid-with-torn-record', gzipSync(tornBody), true)
    const codecOut = []
    const codecDir = join(root, 'codec')
    await mkdir(codecDir, { recursive: true })
    for (const item of codec) {
      const path = join(codecDir, item.name)
      const before = Buffer.from(item.base64, 'base64')
      await writeFile(path, before)
      const strict = await gateFrames(iterateSegmentFrames(path))
      if (!(await readFile(path)).equals(before)) throw new Error('strict read mutated input: ' + item.id)
      let repair = null
      if (item.repair) {
        Date.now = () => now
        try {
          const report = await repairSegmentTail(path)
          repair = { report, bytes: (await readFile(path)).toString('base64'), after: await gateFrames(iterateSegmentFrames(path)) }
        } catch (error) {
          const quarantine = error instanceof JournalCorruptionError ? error.quarantinePath : undefined
          repair = { error: gateError(error), ...(quarantine ? {
            quarantine: basename(quarantine),
            quarantinedBytes: (await readFile(quarantine)).toString('base64'),
            originalPresent: (await readdir(codecDir)).includes(item.name)
          } : {}) }
        }
      }
      codecOut.push({ id: item.id, strict, repair })
    }

    const denseDir = await gateWriter(root, 'dense', [{ timestamp: now, frame: output(1, text('line\n'.repeat(1002))) }])
    const lateDir = await gateWriter(root, 'late', [{ timestamp: now + 9, frame: output(9, text('late\n')) }])
    const rangeIndex = await index(rawDirectory, 'range-index.json')
    const tailIndex = await index(rawDirectory, 'tail-index.json')
    const sidecars = (range: string, tail: string) => ({ 'range-index.json': text(range), 'tail-index.json': text(tail) })
    // Object order deliberately creates segment 2 before segment 1.
    const normal = { [secondName]: second, [firstName]: first }
    const gzipFirst = { [secondName]: second, [firstName + '.gz']: gzip }
    const layouts = {
      normal,
      empty: { [firstName]: magic },
      zero: { [firstName]: Buffer.alloc(0) },
      'empty-between': { 'segment-000003.mtj': second, [secondName]: magic, [firstName]: first },
      'gzip-first': gzipFirst,
      'utf8-split-between-segments': {
        'segment-000003.mtj.gz': gzipSync(Buffer.concat([magic, second.subarray(utf8Split)])),
        [secondName]: second.subarray(0, utf8Split),
        [firstName]: first
      },
      'raw-gzip-duplicate': { ...normal, [firstName + '.gz']: gzip },
      'raw-gzip-fallback': { ...gzipFirst, [firstName]: finalCrc },
      'cold-gap': { 'segment-000003.mtj': second, [firstName]: middleCrc },
      'cold-gap-more-lines': { 'segment-000004.mtj': await readFile(join(lateDir, firstName)),
        'segment-000003.mtj': second, [firstName]: middleCrc },
      'overlap': { [secondName]: first, [firstName]: first },
      'index-truncated': { ...gzipFirst, ...sidecars(
        JSON.stringify(rangeIndex).slice(0, -9), JSON.stringify(tailIndex).slice(0, -9)
      ) },
      'index-invalid-fields': { ...normal, ...sidecars(
        JSON.stringify({ ...rangeIndex, segments: [...rangeIndex.segments].reverse() }),
        JSON.stringify({ ...tailIndex, maxTrackedLines: 0 })
      ) },
      'index-invalid-cold': { [secondName]: second, [firstName]: middleCrc, ...sidecars('{', '{') },
      dense: { [firstName]: await readFile(join(denseDir, firstName)) }
    }
    const layoutFiles = Object.fromEntries(Object.entries(layouts).map(([layout, files]) => [
      layout, Object.entries(files).map(([name, bytes]) => ({ name, base64: Buffer.from(bytes).toString('base64') }))
    ]))
    const ranges = [
      { id: 'segment-order', layout: 'normal', fromSequence: 0 },
      { id: 'empty-segment', layout: 'empty', fromSequence: 0 },
      { id: 'zero-byte-segment', layout: 'zero', fromSequence: 0 },
      { id: 'empty-between-segments', layout: 'empty-between', fromSequence: 1 },
      { id: 'gzip-first-cross-segment', layout: 'gzip-first', fromSequence: 4, throughSequence: 6 },
      { id: 'raw-gzip-no-duplicates', layout: 'raw-gzip-duplicate', fromSequence: 0 },
      { id: 'damaged-raw-gzip-fallback', layout: 'raw-gzip-fallback', fromSequence: 0 },
      { id: 'offset-after-end', layout: 'normal', fromSequence: 9 },
      { id: 'through-before-start', layout: 'normal', fromSequence: 0, throughSequence: 0 },
      { id: 'reversed-range', layout: 'normal', fromSequence: 7, throughSequence: 3 },
      { id: 'offset-unsafe-integer', layout: 'normal', fromSequence: Number.MAX_SAFE_INTEGER + 1 },
      { id: 'through-unsafe-integer', layout: 'normal', fromSequence: 0, throughSequence: Number.MAX_SAFE_INTEGER + 1 },
      { id: 'segment-overlap', layout: 'overlap', fromSequence: 0 },
      { id: 'range-index-truncated', layout: 'index-truncated', fromSequence: 4, throughSequence: 6 },
      { id: 'range-index-invalid-fields', layout: 'index-invalid-fields', fromSequence: 0 },
      { id: 'range-index-invalid-and-cold-corrupt', layout: 'index-invalid-cold', fromSequence: 5 }
    ]
    const tails = [
      { id: 'tail-index-truncated', layout: 'index-truncated', limit: 1 },
      { id: 'tail-index-invalid-fields', layout: 'index-invalid-fields', limit: 1 },
      { id: 'tail-index-rebuild-from-healthy-suffix', layout: 'index-invalid-cold', limit: 1 },
      { id: 'tail-limit-zero', layout: 'normal', limit: 0 },
      { id: 'tail-limit-too-large', layout: 'normal', limit: 10001 }
    ]
    const historyPages = [
      { id: 'history-cross-segment', layout: 'gzip-first', input: { lineLimit: 10 } },
      { id: 'history-split-utf8-across-raw-and-gzip', layout: 'utf8-split-between-segments', input: {} },
      { id: 'history-empty-segment', layout: 'empty', input: {} },
      { id: 'history-zero-byte-segment', layout: 'zero', input: {} },
      { id: 'history-before-first', layout: 'normal', input: { before: { sequence: 0, lineIndex: 0 } } },
      { id: 'history-before-after-end', layout: 'normal', input: { before: { sequence: 99, lineIndex: 99 } } },
      { id: 'history-anchor-after-end', layout: 'normal', input: { around: { sequence: 99, lineIndex: 0 } } },
      { id: 'history-limit-zero', layout: 'normal', input: { lineLimit: 0 } },
      { id: 'history-limit-too-large', layout: 'dense', input: { lineLimit: 10001 } },
      { id: 'history-missing-and-corrupt-gaps', layout: 'cold-gap', input: {} },
      { id: 'history-early-stop-with-gaps', layout: 'cold-gap-more-lines', input: {
        around: { sequence: 7, lineIndex: 0 }, beforeLines: 0, afterLines: 0
      } },
      { id: 'history-gzip-fallback', layout: 'raw-gzip-fallback', input: {} }
    ]
    // Layouts are recreated per operation because the production readers may write indexes.
    async function install(rootPath: string, id: string, layout: keyof typeof layouts) {
      const directory = join(rootPath, 'journal', id)
      await mkdir(directory, { recursive: true })
      for (const { name, base64 } of layoutFiles[layout]) {
        await writeFile(join(directory, name), Buffer.from(base64, 'base64'))
      }
      return directory
    }
    const rangesOut = []
    for (const { id, layout, fromSequence, throughSequence } of ranges) {
      const directory = await install(root, id, layout as keyof typeof layouts)
      const strict = await gateFrames(iterateSessionFrames(root, id, { fromSequence, throughSequence }))
      rangesOut.push({ id, strict, rangeIndex: await index(directory, 'range-index.json') })
    }
    const tailsOut = []
    for (const { id, layout, limit } of tails) {
      const directory = await install(root, id, layout as keyof typeof layouts)
      const writer = await SegmentJournal.open(root, id, { maxSegmentBytes: 512, compressSealed: false })
      let replay = null, error = null
      try {
        replay = writer.replayMetadata(limit)
      } catch (cause) {
        error = gateError(cause)
      } finally {
        await writer.close()
      }
      tailsOut.push({ id, replay, error, tailIndex: await index(directory, 'tail-index.json') })
    }
    const historyOut = []
    const historyRoot = join(root, 'history')
    const history = new JournalHistoryReader(historyRoot)
    for (const { id, layout, input } of historyPages) {
      await install(historyRoot, id, layout as keyof typeof layouts)
      await install(join(stage, 'history-inputs'), id, layout as keyof typeof layouts)
      historyOut.push({ id, page: await history.page({ sessionId: id, ...input }) })
    }
    const compressedDirectory = await gateWriter(root, 'compressed', records, true)
    const compressedNames = (await readdir(compressedDirectory)).filter(name => /\.mtj(\.gz)?$/.test(name)).sort()
    const compressed = []
    for (const name of compressedNames) {
      const bytes = await readFile(join(compressedDirectory, name))
      compressed.push({
        name, base64: (name.endsWith('.gz') ? gunzipSync(bytes) : bytes).toString('base64'),
        gzipBase64: name.endsWith('.gz') ? bytes.toString('base64') : null
      })
    }
    const encode = {
      id: 'raw-encoding-and-rollover', segments: raw, tailIndex, rangeIndex
    }
    const compression = {
      id: 'gzip-publication', segments: compressed,
      strict: await gateFrames(iterateSessionFrames(root, 'compressed', { fromSequence: 0 }))
    }
    const fixedRecords = records.map(({ timestamp, frame }) => ({
      timestamp, frame: frame.kind === 'output' ? { ...frame, data: [...frame.data] } : frame
    }))
    await jsonFile(join(stage, 'inputs.json'), { now, records: fixedRecords, codec, layouts: layoutFiles, ranges, tails, historyPages })
    await jsonFile(join(stage, 'frozen.json'), {
      codec: codecOut, segments: [encode, compression, ...rangesOut, ...tailsOut], history: historyOut
    })
  } finally {
    Date.now = clock
  }
}
