import { readdir } from 'node:fs/promises'
import { join } from 'node:path'

import { CheckpointManager } from '../checkpoints/checkpoint-manager'
import { JournalCorruptionError, SegmentJournal } from '../journal/segment-journal'
import type { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { JournalEventCoordinator } from './journal-event-alignment'
import { SessionRepository } from '../domain/session-repository'

export interface RecoveredSession {
  sessionId: string
  terminalSequence: number
  domainEventSequence: number
  repairedAlignment: boolean
  removedCheckpointOrphans: number
}

export interface FailedSessionRecovery {
  sessionId: string
  code: 'JOURNAL_CORRUPT' | 'RECOVERY_FAILED'
  message: string
}

export interface RuntimeRecoveryReport {
  interruptedRuns: string[]
  recovered: RecoveredSession[]
  failed: FailedSessionRecovery[]
}

/**
 * Reconciles durable terminal and domain state before the Runtime accepts clients.
 * Every Session is an independent recovery unit so one damaged journal never
 * prevents other workspaces from opening.
 */
export class RuntimeRecoveryService {
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase

  constructor(dataRoot: string, database: RuntimeDatabase) {
    this.#dataRoot = dataRoot
    this.#database = database
  }

  async recoverAll(): Promise<RuntimeRecoveryReport> {
    const report: RuntimeRecoveryReport = { interruptedRuns: [], recovered: [], failed: [] }
    const transactions = new DomainTransactionManager(this.#database)
    const sessions = new SessionRepository(this.#database, transactions)
    for (const run of this.#database.all<{ id: string }>(
      `SELECT id FROM session_runs
       WHERE status IN ('starting', 'running') AND runtime_generation <> ?
       ORDER BY started_at`,
      this.#database.runtimeGeneration
    )) {
      sessions.interruptRun(
        {
          commandId: `runtime-recovery-interrupt-${run.id}`,
          commandType: 'session.run-interrupt',
          requestHash: `interrupt:${run.id}`
        },
        run.id,
        Date.now()
      )
      report.interruptedRuns.push(run.id)
    }
    const persistedSessions = new Set(
      this.#database.all<{ id: string }>('SELECT id FROM sessions').map(({ id }) => id)
    )
    const journalRoot = join(this.#dataRoot, 'journal')
    let entries: string[]
    try {
      entries = await readdir(journalRoot)
    } catch (error) {
      if (isMissingFile(error)) return report
      throw error
    }

    const coordinator = new JournalEventCoordinator(
      this.#database,
      transactions
    )
    const checkpoints = new CheckpointManager(this.#dataRoot, this.#database)
    for (const sessionId of entries.sort()) {
      if (!persistedSessions.has(sessionId)) continue
      let journal: SegmentJournal | undefined
      try {
        journal = await SegmentJournal.open(this.#dataRoot, sessionId)
        const watermark = await coordinator.recover(sessionId, journal)
        const removedCheckpointOrphans = await checkpoints.removeOrphans(sessionId)
        report.recovered.push({
          sessionId,
          terminalSequence: watermark.terminalSequence,
          domainEventSequence: watermark.domainEventSequence,
          repairedAlignment: watermark.repaired,
          removedCheckpointOrphans
        })
      } catch (error) {
        report.failed.push({
          sessionId,
          code: error instanceof JournalCorruptionError ? 'JOURNAL_CORRUPT' : 'RECOVERY_FAILED',
          message: errorMessage(error)
        })
      } finally {
        await journal?.close().catch(() => undefined)
      }
    }
    return report
  }
}

function isMissingFile(error: unknown): boolean {
  return error instanceof Error && 'code' in error && error.code === 'ENOENT'
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
