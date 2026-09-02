import { chmod, mkdtemp, readFile, readdir, rename, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'

import { afterEach, describe, expect, it } from 'vitest'

import {
  JournalCorruptionError,
  SegmentJournal,
  readSegmentFrames,
  readSessionFrames,
  repairSegmentTail
} from './segment-journal'
import { loadJournalTailIndex, writeJournalTailIndex } from './journal-tail-index'
import { JournalCompressor } from './journal-compressor'

const temporaryDirectories: string[] = []

afterEach(async () => {
  await Promise.all(
    temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true }))
  )
})

describe('SegmentJournal', () => {
  it('replays output and exit frames in append order', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-1')

    await journal.appendOutput(1, Uint8Array.from([65, 66, 67]))
    await journal.appendExit(2, 0)
    await journal.close()

    await expect(readSegmentFrames(journal.path)).resolves.toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65, 66, 67]) },
      { kind: 'exit', sequence: 2, exitCode: 0 }
    ])
  })

  it('stores every V2 control frame with a monotonic sequence', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-v2')

    await journal.appendResize(1, 120, 40)
    await journal.appendEncoding(2, 'utf-8')
    await journal.appendReset(3, 2)
    await journal.appendOutput(4, Uint8Array.from([65]))
    await journal.appendDomainCursor(5, 19)
    await journal.appendExit(6, 1, 15)
    await journal.close()

    await expect(readSegmentFrames(journal.path)).resolves.toEqual([
      { kind: 'resize', sequence: 1, cols: 120, rows: 40 },
      { kind: 'encoding', sequence: 2, encoding: 'utf-8' },
      { kind: 'reset', sequence: 3, screenEpoch: 2 },
      { kind: 'output', sequence: 4, data: Uint8Array.from([65]) },
      { kind: 'domain-cursor', sequence: 5, domainEventSequence: 19 },
      { kind: 'exit', sequence: 6, exitCode: 1, signal: 15 }
    ])
  })

  it('rejects a duplicate or decreasing sequence', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-sequence')
    await journal.appendOutput(2, Uint8Array.from([65]))

    await expect(journal.appendResize(2, 80, 24)).rejects.toThrow(
      'journal sequence must increase monotonically'
    )
    await journal.close()
  })

  it('detects checksum corruption in a complete frame', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-corrupt')
    await journal.appendOutput(1, Uint8Array.from([65, 66, 67]))
    await journal.appendOutput(2, Uint8Array.from([68, 69, 70]))
    await journal.close()
    const bytes = await readFile(journal.path)
    bytes[20] = bytes[20]! ^ 0xff
    await writeFile(journal.path, bytes)

    await expect(readSegmentFrames(journal.path)).rejects.toBeInstanceOf(JournalCorruptionError)
  })

  it('repairs a torn tail but preserves complete frames', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-tail')
    await journal.appendOutput(1, Uint8Array.from([65]))
    await journal.appendOutput(2, Uint8Array.from([66, 67]))
    await journal.close()
    const complete = await readFile(journal.path)
    await writeFile(journal.path, complete.subarray(0, complete.byteLength - 3))

    const repair = await repairSegmentTail(journal.path)

    expect(repair.truncatedBytes).toBeGreaterThan(0)
    await expect(readSegmentFrames(journal.path)).resolves.toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65]) }
    ])
  })

  it('recovers the last complete frame after a partial ENOSPC write', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    let writes = 0
    const journal = await SegmentJournal.open(directory, 'session-enospc', {
      writeFrame: async (handle, encoded) => {
        writes += 1
        if (writes === 2) {
          await handle.write(encoded.subarray(0, 11))
          const error = new Error('simulated disk full') as NodeJS.ErrnoException
          error.code = 'ENOSPC'
          throw error
        }
        await handle.write(encoded)
      }
    })
    await journal.appendOutput(1, Uint8Array.from([65]))
    await expect(journal.appendOutput(2, Uint8Array.from([66]))).rejects.toMatchObject({ code: 'ENOSPC' })
    await journal.close()

    const reopened = await SegmentJournal.open(directory, 'session-enospc')
    expect(reopened.lastSequence).toBe(1)
    await expect(reopened.readFrames()).resolves.toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65]) }
    ])
    await reopened.close()
  })

  it('rolls back a partial frame so the same sequence can retry on the open journal', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    let failOnce = true
    const journal = await SegmentJournal.open(directory, 'session-live-retry', {
      writeFrame: async (handle, encoded) => {
        if (failOnce) {
          failOnce = false
          await handle.write(encoded.subarray(0, 13))
          throw Object.assign(new Error('simulated disk full'), { code: 'ENOSPC' })
        }
        await handle.write(encoded)
      }
    })

    await expect(journal.appendOutput(1, Uint8Array.from([65, 66]))).rejects.toMatchObject({
      code: 'ENOSPC'
    })
    expect(journal.lastSequence).toBe(0)
    await journal.appendOutput(1, Uint8Array.from([65, 66]))
    await journal.appendExit(2, 0)

    await expect(journal.readFrames()).resolves.toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65, 66]) },
      { kind: 'exit', sequence: 2, exitCode: 0 }
    ])
    await journal.close()
  })

  it('keeps the live journal retryable when opening the next segment fails', async () => {
    if (process.platform === 'win32') return
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-rotate-retry', {
      maxSegmentBytes: 160,
      compressSealed: false
    })
    const sessionDirectory = join(directory, 'journal', 'session-rotate-retry')
    await journal.appendOutput(1, new TextEncoder().encode('first-line'.repeat(10)))

    await chmod(sessionDirectory, 0o500)
    try {
      await expect(journal.appendOutput(2, new TextEncoder().encode('second-line')))
        .rejects.toMatchObject({ code: expect.stringMatching(/EACCES|EPERM/) })
    } finally {
      await chmod(sessionDirectory, 0o700)
    }

    await expect(journal.appendOutput(2, new TextEncoder().encode('second-line')))
      .resolves.toBeUndefined()
    await expect(journal.readFrames()).resolves.toMatchObject([
      { kind: 'output', sequence: 1 },
      { kind: 'output', sequence: 2 }
    ])
    await journal.close()
  })

  it('surfaces a read-only data directory without damaging another Session', async () => {
    if (process.platform === 'win32') return
    const writable = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    const readOnly = await mkdtemp(join(tmpdir(), 'matou-journal-readonly-'))
    temporaryDirectories.push(writable, readOnly)
    const healthy = await SegmentJournal.open(writable, 'healthy-session')
    await healthy.appendOutput(1, Uint8Array.from([65]))
    await healthy.close()

    await chmod(readOnly, 0o500)
    try {
      await expect(SegmentJournal.open(readOnly, 'blocked-session')).rejects.toMatchObject({
        code: expect.stringMatching(/EACCES|EPERM/)
      })
    } finally {
      await chmod(readOnly, 0o700)
    }
    await expect(readSessionFrames(writable, 'healthy-session')).resolves.toHaveLength(1)
  })

  it('rotates without blocking the PTY write path and keeps the newest raw hot window', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const compressor = new JournalCompressor()
    const journal = await SegmentJournal.open(directory, 'session-rotate', {
      maxSegmentBytes: 150,
      rawHotBytes: 160,
      compressSealed: true,
      compressor
    })
    for (let sequence = 1; sequence <= 12; sequence += 1) {
      await journal.appendOutput(sequence, Uint8Array.from({ length: 32 }, () => sequence))
    }
    await journal.close()
    await compressor.whenIdle()

    const files = await readdir(join(directory, 'journal', 'session-rotate'))
    expect(files.filter((file) => file.endsWith('.mtj')).length).toBeGreaterThanOrEqual(2)
    expect(files.filter((file) => file.endsWith('.gz')).length).toBeGreaterThan(0)
    expect((await readSessionFrames(directory, 'session-rotate')).map(({ sequence }) => sequence)).toEqual([
      1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12
    ])
  })

  it('keeps a live append boundary when cold history and its tail sidecar are damaged', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const compressor = new JournalCompressor()
    const options = {
      maxSegmentBytes: 150,
      rawHotBytes: 160,
      compressSealed: true,
      compressor
    }
    const journal = await SegmentJournal.open(directory, 'session-cold-gap', options)
    for (let sequence = 1; sequence <= 18; sequence += 1) {
      await journal.appendOutput(sequence, new TextEncoder().encode(`line-${sequence}\n`.repeat(4)))
    }
    await journal.close()
    await compressor.whenIdle()

    const sessionDirectory = join(directory, 'journal', 'session-cold-gap')
    const coldArchive = (await readdir(sessionDirectory)).filter((file) => file.endsWith('.gz')).sort()[0]
    expect(coldArchive).toBeDefined()
    await writeFile(join(sessionDirectory, coldArchive!), 'damaged cold archive')
    await writeFile(join(sessionDirectory, 'tail-index.json'), '{damaged')

    const reopened = await SegmentJournal.open(directory, 'session-cold-gap', options)
    expect(reopened.lastSequence).toBe(18)
    await reopened.appendOutput(19, new TextEncoder().encode('live-after-gap\n'))
    expect(reopened.lastSequence).toBe(19)
    await reopened.close()
  })

  it('reads a segment index only once while raw and gzip copies overlap during compression commit', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-overlap', {
      maxSegmentBytes: 150
    })
    for (let sequence = 1; sequence <= 4; sequence += 1) {
      await journal.appendOutput(sequence, Uint8Array.from({ length: 32 }, () => sequence))
    }
    await journal.close()
    const sessionDirectory = join(directory, 'journal', 'session-overlap')
    const firstRaw = (await readdir(sessionDirectory)).filter((file) => file.endsWith('.mtj')).sort()[0]!
    await writeFile(
      join(sessionDirectory, `${firstRaw}.gz`),
      gzipSync(await readFile(join(sessionDirectory, firstRaw)))
    )

    expect((await readSessionFrames(directory, 'session-overlap')).map(({ sequence }) => sequence)).toEqual([
      1, 2, 3, 4
    ])
  })

  it('reopens and appends to a legacy raw bin segment without losing its sequence', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const original = await SegmentJournal.open(directory, 'session-legacy')
    await original.appendOutput(1, Uint8Array.from([65]))
    const modernPath = original.path
    await original.close()
    const legacyPath = modernPath.replace(/\.mtj$/, '.bin')
    await rename(modernPath, legacyPath)

    const reopened = await SegmentJournal.open(directory, 'session-legacy')
    expect(reopened.lastSequence).toBe(1)
    await reopened.appendOutput(2, Uint8Array.from([66]))
    await reopened.close()

    await expect(readSessionFrames(directory, 'session-legacy')).resolves.toEqual([
      { kind: 'output', sequence: 1, data: Uint8Array.from([65]) },
      { kind: 'output', sequence: 2, data: Uint8Array.from([66]) }
    ])
  })

  it('persists the 10,000-line tail boundary and rebuilds a corrupt sidecar', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-tail-index')
    for (let sequence = 1; sequence <= 10_001; sequence += 1) {
      await journal.appendOutput(sequence, new TextEncoder().encode(`line-${sequence}\n`))
    }
    expect(journal.tailStart()).toBe(2)
    await journal.close()

    const sidecar = join(directory, 'journal', 'session-tail-index', 'tail-index.json')
    expect((await loadJournalTailIndex(sidecar)).lastSequence).toBe(10_001)
    const reopened = await SegmentJournal.open(directory, 'session-tail-index')
    expect(reopened.tailStart()).toBe(2)
    await reopened.close()

    await writeFile(sidecar, '{truncated')
    const rebuilt = await SegmentJournal.open(directory, 'session-tail-index')
    expect(rebuilt.tailStart()).toBe(2)
    expect(rebuilt.tailIndexSnapshot().completedLineCount).toBe(10_001)
    await rebuilt.close()
  }, 15_000)

  it('catches a valid but stale sidecar up from newer Journal output', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const first = await SegmentJournal.open(directory, 'session-stale-index')
    await first.appendOutput(1, new TextEncoder().encode('one\n'))
    await first.close()
    const sidecar = join(directory, 'journal', 'session-stale-index', 'tail-index.json')
    const stale = await loadJournalTailIndex(sidecar)

    const second = await SegmentJournal.open(directory, 'session-stale-index')
    await second.appendOutput(2, new TextEncoder().encode('two\n'))
    await second.close()
    await writeJournalTailIndex(sidecar, stale)

    const recovered = await SegmentJournal.open(directory, 'session-stale-index')
    expect(recovered.tailIndexSnapshot()).toMatchObject({
      lastSequence: 2,
      completedLineCount: 2
    })
    await recovered.close()
  })

  it('catches a sidecar up across every sealed segment without skipping lines or domain cursors', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const options = { maxSegmentBytes: 128 }
    const first = await SegmentJournal.open(directory, 'session-multi-stale-index', options)
    await first.appendOutput(1, new TextEncoder().encode('one\n'.repeat(20)))
    await first.close()
    const sidecar = join(directory, 'journal', 'session-multi-stale-index', 'tail-index.json')
    const stale = await loadJournalTailIndex(sidecar)

    const writer = await SegmentJournal.open(directory, 'session-multi-stale-index', options)
    await writer.appendOutput(2, new TextEncoder().encode('two\n'.repeat(20)))
    await writer.appendDomainCursor(3, 19)
    await writer.appendOutput(4, new TextEncoder().encode('three\n'.repeat(20)))
    await writer.close()
    expect((await loadJournalTailIndex(sidecar)).activeSegmentIndex).toBeGreaterThan(
      stale.activeSegmentIndex + 1
    )
    await writeJournalTailIndex(sidecar, stale)

    const recovered = await SegmentJournal.open(directory, 'session-multi-stale-index', options)
    expect(recovered.tailIndexSnapshot()).toMatchObject({
      lastSequence: 4, completedLineCount: 60
    })
    expect(recovered.domainEventSequenceAtOrBefore(4)).toBe(19)
    await recovered.close()
  })
})
