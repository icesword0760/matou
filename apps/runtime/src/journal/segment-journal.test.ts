import { chmod, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import {
  JournalCorruptionError,
  SegmentJournal,
  readSegmentFrames,
  readSessionFrames,
  repairSegmentTail
} from './segment-journal'

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

  it('rotates and compresses sealed segments while replaying the whole session', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'matou-journal-'))
    temporaryDirectories.push(directory)
    const journal = await SegmentJournal.open(directory, 'session-rotate', {
      maxSegmentBytes: 150,
      compressSealed: true
    })
    for (let sequence = 1; sequence <= 6; sequence += 1) {
      await journal.appendOutput(sequence, Uint8Array.from({ length: 32 }, () => sequence))
    }
    await journal.close()

    const files = await readdir(join(directory, 'journal', 'session-rotate'))
    expect(files.some((file) => file.endsWith('.bin.gz'))).toBe(true)
    expect((await readSessionFrames(directory, 'session-rotate')).map(({ sequence }) => sequence)).toEqual([
      1, 2, 3, 4, 5, 6
    ])
  })
})
