import { performance } from 'node:perf_hooks'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { DomainEventStore } from '../events/domain-event-store'
import { CreditWindow } from '../flow-control/credit-window'
import { SegmentJournal, readSessionFrames } from '../journal/segment-journal'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

const roots: string[] = []
const databases: RuntimeDatabase[] = []
afterEach(async () => {
  for (const database of databases.splice(0)) database.close()
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('infrastructure load envelope', () => {
  it('keeps credit accounting independent across 16 busy terminal streams', () => {
    const windows = Array.from({ length: 16 }, () => new CreditWindow({
      highWatermarkBytes: 1024 * 1024,
      lowWatermarkBytes: 512 * 1024
    }))
    const startedAt = performance.now()
    for (let sequence = 1; sequence <= 1025; sequence += 1) {
      for (const window of windows) window.recordSent(sequence, 1024)
    }
    expect(windows.every(({ isPaused }) => isPaused)).toBe(true)
    windows[0]!.acknowledge(1025)
    expect(windows[0]!.isPaused).toBe(false)
    expect(windows.slice(1).every(({ isPaused }) => isPaused)).toBe(true)
    expect(performance.now() - startedAt).toBeLessThan(5_000)
  })

  it('appends and replays eight Session journals concurrently without cross-session loss', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-load-journal-'))
    roots.push(root)
    const startedAt = performance.now()
    await Promise.all(Array.from({ length: 8 }, async (_, sessionIndex) => {
      const sessionId = `session-${sessionIndex}`
      const journal = await SegmentJournal.open(root, sessionId, { compressSealed: false })
      for (let sequence = 1; sequence <= 256; sequence += 1) {
        await journal.appendOutput(sequence, Uint8Array.from({ length: 256 }, () => sessionIndex))
      }
      await journal.close()
      const frames = await readSessionFrames(root, sessionId)
      expect(frames).toHaveLength(256)
      expect(frames.at(-1)?.sequence).toBe(256)
    }))
    expect(performance.now() - startedAt).toBeLessThan(10_000)
  })

  it('keeps synchronous transaction-plus-outbox latency bounded under a 500-command burst', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-load-outbox-'))
    roots.push(root)
    const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
    databases.push(database)
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    const transactions = new DomainTransactionManager(database)
    const latencies: number[] = []
    for (let index = 1; index <= 500; index += 1) {
      const startedAt = performance.now()
      transactions.execute(
        { commandId: `load-${index}`, commandType: 'load.event', requestHash: `hash-${index}` },
        ({ emit }) => {
          emit({
            eventId: `load-event-${index}`, eventType: 'load.event', aggregateType: 'load',
            aggregateId: `load-${index}`, payload: { index }, occurredAt: index
          })
          return index
        }
      )
      latencies.push(performance.now() - startedAt)
    }
    latencies.sort((left, right) => left - right)
    const p95 = latencies[Math.floor(latencies.length * 0.95)]!
    expect(new DomainEventStore(database).readAfter(0, 1000)).toHaveLength(500)
    expect(p95).toBeLessThan(100)
  })
})
