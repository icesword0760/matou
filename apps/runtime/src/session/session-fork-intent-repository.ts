import type { RuntimeDatabase } from '../storage/database'

interface ForkIntentRow {
  session_id: string
  source_session_id: string
  source_provider_session_id: string
  state: 'pending' | 'starting' | 'succeeded' | 'failed'
  error_message: string | null
}

export type ForkLaunchDecision =
  | {
      kind: 'launch'
      sourceSessionId: string
      sourceProviderSessionId: string
    }
  | { kind: 'failed'; error: string }

export class SessionForkIntentRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  claimForLaunch(sessionId: string, now: number): ForkLaunchDecision | undefined {
    return this.#database.transaction((tx) => {
      const row = tx.get<ForkIntentRow>(
        'SELECT * FROM session_fork_intents WHERE session_id = ?', sessionId
      )
      if (!row || row.state === 'succeeded') return undefined
      if (row.state === 'failed') {
        return { kind: 'failed', error: row.error_message ?? 'Fork 会话启动失败' }
      }
      if (row.state === 'starting') {
        const error = 'Fork 会话启动被中断，请重新分叉'
        tx.run(
          `UPDATE session_fork_intents
           SET state = 'failed', error_message = ?, completed_at = ? WHERE session_id = ?`,
          error, now, sessionId
        )
        return { kind: 'failed', error }
      }
      tx.run(
        `UPDATE session_fork_intents
         SET state = 'starting', started_at = ?, error_message = NULL WHERE session_id = ?`,
        now, sessionId
      )
      return {
        kind: 'launch',
        sourceSessionId: row.source_session_id,
        sourceProviderSessionId: row.source_provider_session_id
      }
    })
  }

  fail(sessionId: string, error: string, now: number): void {
    this.#database.run(
      `UPDATE session_fork_intents
       SET state = 'failed', error_message = ?, completed_at = ?
       WHERE session_id = ? AND state IN ('pending', 'starting')`,
      error, now, sessionId
    )
  }

  state(sessionId: string): ForkIntentRow['state'] | undefined {
    try {
      return this.#database.get<Pick<ForkIntentRow, 'state'>>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', sessionId
      )?.state
    } catch {
      return undefined
    }
  }
}
