import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

// Shared CI runners are slower and noisier than developer machines; keep the gate but loosen it there.
const CI_BUDGET_FACTOR = process.env.CI ? 2 : 1

import { SegmentJournal } from './segment-journal'
import {
  iterateSessionFrames,
  readSessionJournalBounds,
  readSessionReplayMetadata
} from './journal-range-reader'

const roots: string[] = []

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('Journal range reader', () => {
  it('records new segment bounds when the writer rotates and closes', async () => {
    const root = await createRoot()
    const journal = await SegmentJournal.open(root, 'writer-indexed', {
      maxSegmentBytes: 360,
      compressSealed: false
    })
    for (let sequence = 1; sequence <= 5; sequence += 1) {
      await journal.appendOutput(sequence, new TextEncoder().encode(`payload-${sequence}`.repeat(20)))
    }
    await journal.close()

    const persisted = JSON.parse(await readFile(
      join(root, 'journal', 'writer-indexed', 'range-index.json'),
      'utf8'
    )) as { segments: Array<{ firstSequence: number; lastSequence: number }> }
    expect(persisted.segments.length).toBeGreaterThan(1)
    expect(persisted.segments[0]).toMatchObject({ firstSequence: 1 })
    expect(persisted.segments.at(-1)).toMatchObject({ lastSequence: 5 })
  })

  it('refreshes the active segment bound when a crash leaves its sidecar behind', async () => {
    const root = await createRoot()
    const journal = await SegmentJournal.open(root, 'growing-active', {
      compressSealed: false
    })
    await journal.appendOutput(1, new TextEncoder().encode('before-sidecar'))
    await expect(readSessionJournalBounds(root, 'growing-active')).resolves.toMatchObject({
      lastSequence: 1
    })

    await journal.appendOutput(2, new TextEncoder().encode('after-sidecar'))

    await expect(readSessionJournalBounds(root, 'growing-active')).resolves.toMatchObject({
      firstSequence: 1,
      lastSequence: 2
    })
    await journal.close()
  })

  it('publishes one valid sidecar under concurrent replay metadata reads', async () => {
    const root = await createRoot()
    const journal = await SegmentJournal.open(root, 'concurrent-index', {
      compressSealed: false
    })
    await journal.appendOutput(1, new TextEncoder().encode('concurrent'))
    await journal.close()
    await rm(join(root, 'journal', 'concurrent-index', 'range-index.json'))

    const results = await Promise.all(Array.from({ length: 12 }, () =>
      readSessionJournalBounds(root, 'concurrent-index')
    ))

    expect(results.every(({ firstSequence, lastSequence }) =>
      firstSequence === 1 && lastSequence === 1
    )).toBe(true)
    expect((await readdir(join(root, 'journal', 'concurrent-index'))).filter(
      (entry) => entry.includes('range-index.json.tmp-')
    )).toEqual([])
  })

  it('keeps Journal replay available when the derived sidecar cannot be published', async () => {
    const root = await createRoot()
    const journal = await SegmentJournal.open(root, 'sidecar-unavailable', {
      compressSealed: false
    })
    await journal.appendOutput(1, new TextEncoder().encode('still-readable'))
    await journal.close()
    const indexPath = join(root, 'journal', 'sidecar-unavailable', 'range-index.json')
    await rm(indexPath)
    await mkdir(indexPath)

    await expect(readSessionJournalBounds(root, 'sidecar-unavailable')).resolves.toMatchObject({
      firstSequence: 1,
      lastSequence: 1
    })
    const frames = []
    for await (const frame of iterateSessionFrames(root, 'sidecar-unavailable', {
      fromSequence: 1
    })) {
      frames.push(frame)
    }
    expect(frames.map(({ sequence }) => sequence)).toEqual([1])
  })

  it('uses persisted segment bounds to skip an older segment outside the requested range', async () => {
    const root = await createRoot()
    const journal = await SegmentJournal.open(root, 'range-session', {
      maxSegmentBytes: 360,
      compressSealed: false
    })
    for (let sequence = 1; sequence <= 6; sequence += 1) {
      await journal.appendOutput(sequence, new TextEncoder().encode(`frame-${sequence}`.repeat(20)))
    }
    await journal.close()

    const bounds = await readSessionJournalBounds(root, 'range-session')
    expect(bounds).toMatchObject({ firstSequence: 1, lastSequence: 6 })
    expect(bounds.segments.length).toBeGreaterThan(1)
    const skipped = bounds.segments.find(({ lastSequence }) => lastSequence < 5)
    expect(skipped).toBeDefined()
    await writeFile(skipped!.path, 'corrupted segment that must stay cold')

    const frames = []
    for await (const frame of iterateSessionFrames(root, 'range-session', { fromSequence: 5 })) {
      frames.push(frame)
    }
    expect(frames.map(({ sequence }) => sequence)).toEqual([5, 6])

    const persisted = JSON.parse(await readFile(
      join(root, 'journal', 'range-session', 'range-index.json'),
      'utf8'
    )) as { segments: unknown[] }
    expect(persisted.segments).toHaveLength(bounds.segments.length)
  })

  it('preserves output, resize, reset, and exit ordering inside the selected range', async () => {
    const root = await createRoot()
    const journal = await SegmentJournal.open(root, 'ordered-range', {
      maxSegmentBytes: 360,
      compressSealed: false
    })
    await journal.appendOutput(1, new TextEncoder().encode('cold'.repeat(80)))
    await journal.appendResize(2, 120, 42)
    await journal.appendReset(3, 7)
    await journal.appendOutput(4, new TextEncoder().encode('visible'))
    await journal.appendExit(5, 9, 15)
    await journal.close()

    const frames = []
    for await (const frame of iterateSessionFrames(root, 'ordered-range', {
      fromSequence: 2,
      throughSequence: 5
    })) {
      frames.push(frame)
    }
    expect(frames).toEqual([
      { kind: 'resize', sequence: 2, cols: 120, rows: 42 },
      { kind: 'reset', sequence: 3, screenEpoch: 7 },
      { kind: 'output', sequence: 4, data: new TextEncoder().encode('visible') },
      { kind: 'exit', sequence: 5, exitCode: 9, signal: 15 }
    ])
  })

  it('loads tail and domain watermarks from the sidecar without decoding cold segments', async () => {
    const root = await createRoot()
    const journal = await SegmentJournal.open(root, 'metadata-session', {
      maxSegmentBytes: 360,
      compressSealed: false
    })
    await journal.appendOutput(1, new TextEncoder().encode('old-line\n'.repeat(20)))
    await journal.appendOutput(2, new TextEncoder().encode('middle-line\n'.repeat(20)))
    await journal.appendDomainCursor(3, 17)
    await journal.appendOutput(4, new TextEncoder().encode('latest-line\n'.repeat(20)))
    await journal.close()
    const bounds = await readSessionJournalBounds(root, 'metadata-session')
    const cold = bounds.segments.find(({ lastSequence }) => lastSequence < 4)
    expect(cold).toBeDefined()
    await writeFile(cold!.path, 'cold corruption is outside the replay metadata path')

    await expect(readSessionReplayMetadata(root, 'metadata-session', 10_000)).resolves.toEqual({
      firstSequence: 1,
      lastSequence: 4,
      tailFromSequence: 1,
      domainEventSequence: 17
    })
  })

  it.each([32, 256])(
    'recovers only the checkpoint suffix of a %d MiB Journal within a 16 MiB RSS budget',
    { timeout: 120_000 },
    async (historyMiB) => {
      const root = await createRoot()
      const sessionId = `memory-${historyMiB}`
      const journal = await SegmentJournal.open(root, sessionId, {
        maxSegmentBytes: 8 * MIB,
        compressSealed: false
      })
      const payload = new Uint8Array(MIB)
      payload.fill(65)
      for (let sequence = 1; sequence <= historyMiB; sequence += 1) {
        await journal.appendOutput(sequence, payload)
      }
      await journal.close()
      const fromSequence = Math.floor(historyMiB * 0.9) + 1
      const durations: number[] = []
      let maximumRssDelta = 0
      const trials = historyMiB === 32 ? 5 : 1
      for (let trial = 0; trial < trials; trial += 1) {
        const baselineRss = process.memoryUsage.rss()
        let peakRss = baselineRss
        let recoveredFrames = 0
        let expectedSequence = fromSequence
        const startedAt = performance.now()
        for await (const frame of iterateSessionFrames(root, sessionId, { fromSequence })) {
          expect(frame.sequence).toBe(expectedSequence)
          expectedSequence += 1
          recoveredFrames += 1
          peakRss = Math.max(peakRss, process.memoryUsage.rss())
        }
        durations.push(performance.now() - startedAt)
        maximumRssDelta = Math.max(maximumRssDelta, peakRss - baselineRss)
        expect(recoveredFrames).toBe(historyMiB - fromSequence + 1)
      }

      expect(maximumRssDelta).toBeLessThanOrEqual(16 * MIB)
      if (historyMiB === 32) {
        durations.sort((left, right) => left - right)
        const p95 = durations[Math.ceil(durations.length * 0.95) - 1]!
        expect(p95).toBeLessThan(100 * CI_BUDGET_FACTOR)
      }
    }
  )
})

const MIB = 1024 * 1024

async function createRoot(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), 'matou-journal-range-'))
  roots.push(root)
  return root
}
