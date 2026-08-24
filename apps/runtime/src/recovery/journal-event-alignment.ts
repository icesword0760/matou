import type { DomainCommandMetadata, DomainCommit } from '@matou/domain'

import type { SegmentJournal } from '../journal/segment-journal'
import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type {
  DomainMutationContext,
  DomainTransactionManager
} from '../storage/domain-transaction'

export type AlignmentFaultPhase =
  | 'after-domain-commit'
  | 'after-marker-append'
  | 'after-marker-flush'

export interface RecoveryWatermark {
  terminalSequence: number
  domainEventSequence: number
  repaired: boolean
  pendingDomainEventSequence: number | undefined
}

interface StoredRequiredSequence {
  seq: number
  required_terminal_sequence: number | null
}

export class JournalEventCoordinator {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  async execute<T>(
    sessionId: string,
    journal: SegmentJournal,
    command: DomainCommandMetadata,
    mutate: (context: DomainMutationContext) => T,
    injectFault?: (phase: AlignmentFaultPhase) => void
  ): Promise<DomainCommit<T>> {
    const commit = this.#transactions.execute(command, mutate)
    injectFault?.('after-domain-commit')

    if (commit.lastEventSequence !== undefined) {
      const existingCursor = highestDomainCursor(await journal.readFrames())
      if (existingCursor < commit.lastEventSequence) {
        await journal.appendDomainCursor(journal.lastSequence + 1, commit.lastEventSequence)
        injectFault?.('after-marker-append')
        await journal.flush()
        injectFault?.('after-marker-flush')
      }
    }
    return commit
  }

  async recover(sessionId: string, journal: SegmentJournal): Promise<RecoveryWatermark> {
    const frames = await journal.readFrames()
    const committedMaximum =
      this.#database.get<{ maximum: number }>(
        'SELECT COALESCE(MAX(seq), 0) AS maximum FROM domain_events'
      )?.maximum ?? 0
    const cursor = highestDomainCursor(frames)
    if (cursor > committedMaximum) {
      throw new Error(
        `journal domain cursor ${cursor} is ahead of committed sequence ${committedMaximum}`
      )
    }

    const terminalSequenceBeforeRepair = journal.lastSequence
    const pending = this.#database.all<StoredRequiredSequence>(
      `SELECT seq, required_terminal_sequence
       FROM domain_events
       WHERE session_id = ? AND seq > ?
       ORDER BY seq`,
      sessionId,
      cursor
    )

    let alignThrough = cursor
    let pendingDomainEventSequence: number | undefined
    for (const event of pending) {
      if (
        event.required_terminal_sequence !== null &&
        event.required_terminal_sequence > terminalSequenceBeforeRepair
      ) {
        pendingDomainEventSequence = event.seq
        break
      }
      alignThrough = event.seq
    }

    let repaired = false
    if (alignThrough > cursor) {
      await journal.appendDomainCursor(journal.lastSequence + 1, alignThrough)
      await journal.flush()
      repaired = true
    }

    return {
      terminalSequence: journal.lastSequence,
      domainEventSequence: alignThrough,
      repaired,
      pendingDomainEventSequence
    }
  }
}

function highestDomainCursor(
  frames: Awaited<ReturnType<SegmentJournal['readFrames']>>
): number {
  let maximum = 0
  for (const frame of frames) {
    if (frame.kind === 'domain-cursor') {
      maximum = Math.max(maximum, frame.domainEventSequence)
    }
  }
  return maximum
}

// Keeps the transaction-only dependency explicit for boundary checks and repository signatures.
export type AlignedMutation = (context: {
  tx: DatabaseTransaction
  emit: DomainMutationContext['emit']
}) => unknown
