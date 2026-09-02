import { mkdtemp, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'

import { afterEach, describe, expect, it } from 'vitest'

import { JournalCompressor } from './journal-compressor'
import { readSegmentFrames, readSessionFrames, SegmentJournal } from './segment-journal'

const roots: string[] = []

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('JournalCompressor', () => {
  it('atomically publishes a durable gzip before deleting the raw segment', async () => {
    const { directory, rawPath } = await sealedRawSegment()
    const expected = await readSegmentFrames(rawPath)
    const phases: string[] = []

    const result = await new JournalCompressor({
      afterPhase: (phase) => { phases.push(phase) }
    }).compress({ sessionId: 'session-1', index: 1, path: rawPath })

    expect(phases).toEqual(['temp-written', 'published', 'raw-deleted'])
    expect(result.path).toBe(`${rawPath}.gz`)
    await expect(readSegmentFrames(result.path)).resolves.toEqual(expected)
    await expect(stat(rawPath)).rejects.toMatchObject({ code: 'ENOENT' })
    expect((await readdir(directory)).some((name) => name.endsWith('.partial'))).toBe(false)
  })

  it.each(['temp-written', 'published'] as const)(
    'recovers an interrupted %s commit without duplicating segment frames',
    async (interruptAfter) => {
      const { root, rawPath } = await sealedRawSegment()
      const expected = await readSessionFrames(root, 'session-1')
      const interrupted = new JournalCompressor({
        afterPhase: (phase) => {
          if (phase === interruptAfter) throw new Error(`interrupt:${phase}`)
        }
      })

      await expect(interrupted.compress({
        sessionId: 'session-1', index: 1, path: rawPath
      })).rejects.toThrow(`interrupt:${interruptAfter}`)

      await new JournalCompressor().compress({ sessionId: 'session-1', index: 1, path: rawPath })
      const reopened = await SegmentJournal.open(root, 'session-1', { compressSealed: false })
      const frames = await reopened.readFrames()
      await reopened.close()
      expect(frames).toEqual(expected)
    }
  )

  it('replaces a corrupt published gzip from the intact raw segment', async () => {
    const { rawPath } = await sealedRawSegment()
    const expected = await readSegmentFrames(rawPath)
    await writeFile(`${rawPath}.gz`, 'not-a-gzip')

    await new JournalCompressor().compress({ sessionId: 'session-1', index: 1, path: rawPath })

    expect(await readSegmentFrames(`${rawPath}.gz`)).toEqual(expected)
  })

  it('does not trust a syntactically valid gzip with invalid Journal contents', async () => {
    const { rawPath } = await sealedRawSegment()
    const expected = await readSegmentFrames(rawPath)
    await writeFile(`${rawPath}.gz`, gzipSync('plain text, not a Journal segment'))

    await new JournalCompressor().compress({ sessionId: 'session-1', index: 1, path: rawPath })

    expect(await readSegmentFrames(`${rawPath}.gz`)).toEqual(expected)
  })

  it('defaults to two concurrent jobs and de-duplicates the same segment', async () => {
    const fixtures = await Promise.all([0, 1, 2, 3].map((index) => sealedRawSegment(`session-${index}`)))
    let active = 0
    let maximum = 0
    const compressor = new JournalCompressor({
      afterPhase: async (phase) => {
        if (phase !== 'temp-written') return
        active += 1
        maximum = Math.max(maximum, active)
        await new Promise((resolve) => setTimeout(resolve, 20))
        active -= 1
      }
    })

    const jobs = fixtures.map(({ rawPath }, index) => compressor.schedule({
      sessionId: `session-${index}`, index: 1, path: rawPath
    }))
    expect(compressor.schedule({
      sessionId: 'session-0', index: 1, path: fixtures[0]!.rawPath
    })).toBe(jobs[0])

    await Promise.all(jobs)
    expect(maximum).toBe(2)
  })

  it('rotates without waiting for gzip work on the PTY write chain', async () => {
    const root = await makeRoot()
    let release!: () => void
    let markStarted!: () => void
    const compressionGate = new Promise<void>((resolve) => { release = resolve })
    const compressionStarted = new Promise<void>((resolve) => { markStarted = resolve })
    const compressor = new JournalCompressor({
      afterPhase: async (phase) => {
        if (phase === 'temp-written') {
          markStarted()
          await compressionGate
        }
      }
    })
    const journal = await SegmentJournal.open(root, 'live-session', {
      maxSegmentBytes: 160,
      rawHotBytes: 160,
      compressor
    })

    await journal.appendOutput(1, new TextEncoder().encode('alpha\n'.repeat(8)))
    await expect(journal.appendOutput(2, new TextEncoder().encode('beta\n'.repeat(8))))
      .resolves.toBeUndefined()
    // The newest sealed segment remains raw by product contract. Rotating a
    // second sealed segment makes the older one eligible for background gzip.
    await expect(journal.appendOutput(3, new TextEncoder().encode('gamma\n'.repeat(8))))
      .resolves.toBeUndefined()
    await compressionStarted
    expect((await readdir(join(root, 'journal', 'live-session'))).some((name) => name.endsWith('.partial')))
      .toBe(true)

    release()
    await compressor.whenIdle()
    await journal.close()
    expect((await journal.readFrames()).map(({ sequence }) => sequence)).toEqual([1, 2, 3])
  })
})

async function sealedRawSegment(sessionId = 'session-1') {
  const root = await makeRoot()
  const journal = await SegmentJournal.open(root, sessionId, {
    maxSegmentBytes: 160,
    compressSealed: false
  })
  await journal.appendOutput(1, new TextEncoder().encode('alpha\n'.repeat(8)))
  await journal.appendOutput(2, new TextEncoder().encode('beta\n'.repeat(8)))
  await journal.close()
  const directory = join(root, 'journal', sessionId)
  return { root, directory, rawPath: join(directory, 'segment-000001.mtj') }
}

async function makeRoot(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'matou-journal-compressor-'))
  roots.push(root)
  return root
}
