import type {
  DomainCommandMetadata,
  SceneSessionGraph,
  SessionWorkStatus
} from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'
import { projectSceneGraphFrom } from './session-graph-repository'

export interface SessionWorkStatusResult {
  sessionId: string
  sceneId: string
  previousStatus: SessionWorkStatus
  workStatus: SessionWorkStatus
  graph: SceneSessionGraph
}

export class SessionWorkStatusService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  set(
    command: DomainCommandMetadata,
    input: { sessionId: string; workStatus: SessionWorkStatus; now: number }
  ): SessionWorkStatusResult {
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const owner = tx.get<{
        scene_id: string
        task_id: string
        workspace_id: string
        work_status: SessionWorkStatus
      }>(
        `SELECT membership.scene_id, sessions.task_id, tasks.workspace_id,
                sessions.work_status
         FROM sessions
         JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
         JOIN tasks ON tasks.id = sessions.task_id
         WHERE sessions.id = ?`,
        input.sessionId
      )
      if (!owner) throw new Error(`Session ${input.sessionId} does not exist in a canvas`)

      if (owner.work_status !== input.workStatus) {
        tx.run(
          `UPDATE sessions
           SET work_status = ?, updated_at = ?, version = version + 1
           WHERE id = ?`,
          input.workStatus, input.now, input.sessionId
        )
      }
      const graph = projectSceneGraphFrom(tx, owner.scene_id)
      const result: SessionWorkStatusResult = {
        sessionId: input.sessionId,
        sceneId: owner.scene_id,
        previousStatus: owner.work_status,
        workStatus: input.workStatus,
        graph
      }
      emit({
        eventId: `${command.commandId}:work-status`,
        eventType: 'session.graph-summary-changed',
        aggregateType: 'session',
        aggregateId: input.sessionId,
        workspaceId: owner.workspace_id,
        taskId: owner.task_id,
        sessionId: input.sessionId,
        payload: result,
        occurredAt: input.now
      })
      return result
    }).result
  }

  get(sessionId: string): SessionWorkStatus | undefined {
    return this.#database.get<{ work_status: SessionWorkStatus }>(
      'SELECT work_status FROM sessions WHERE id = ?', sessionId
    )?.work_status
  }
}
