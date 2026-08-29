import type {
  DomainCommandMetadata,
  SceneSessionGraph
} from '@matou/domain'

import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'
import { projectSceneGraphFrom } from './session-graph-repository'

export type SessionInteractionKind = 'submit' | 'control' | 'provider-action'

export interface RecordSessionInteractionInput {
  sessionId: string
  interactionKind: SessionInteractionKind
  now: number
}

export interface RecordedSessionInteraction {
  sessionId: string
  sceneId: string
  taskId: string
  workspaceId: string
  interactionKind: SessionInteractionKind
  sequence: number
  graph: SceneSessionGraph
}

const ORDERING_INTERACTIONS = new Set<SessionInteractionKind>([
  'submit', 'control', 'provider-action'
])

export class SessionInteractionService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  record(
    command: DomainCommandMetadata,
    input: RecordSessionInteractionInput
  ): RecordedSessionInteraction {
    if (!ORDERING_INTERACTIONS.has(input.interactionKind)) {
      throw new Error('用户交互类型不参与会话排序')
    }
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const owner = tx.get<{
        scene_id: string
        task_id: string
        workspace_id: string
      }>(
        `SELECT membership.scene_id, sessions.task_id, tasks.workspace_id
         FROM session_canvas_memberships AS membership
         JOIN sessions ON sessions.id = membership.session_id
         JOIN tasks ON tasks.id = sessions.task_id
         WHERE membership.session_id = ?
           AND sessions.archived_at IS NULL
           AND tasks.archived_at IS NULL`,
        input.sessionId
      )
      if (!owner) throw new Error(`Session ${input.sessionId} does not exist in an active canvas`)

      tx.run(
        `UPDATE runtime_sequences SET value = value + 1
         WHERE name = 'session-user-interaction'`
      )
      const sequence = tx.get<{ value: number }>(
        `SELECT value FROM runtime_sequences
         WHERE name = 'session-user-interaction'`
      )?.value
      if (sequence === undefined) throw new Error('session-user-interaction sequence does not exist')

      tx.run(
        `UPDATE session_canvas_memberships
         SET last_user_interaction_seq = ?, updated_at = ?
         WHERE session_id = ?`,
        sequence, input.now, input.sessionId
      )
      tx.run(
        `UPDATE sessions
         SET last_activity_at = ?, updated_at = ?, version = version + 1
         WHERE id = ?`,
        input.now, input.now, input.sessionId
      )
      tx.run('UPDATE tasks SET last_opened_at = ? WHERE id = ?', input.now, owner.task_id)
      tx.run(
        'UPDATE workspaces SET last_opened_at = ? WHERE id = ?',
        input.now, owner.workspace_id
      )

      const graph = projectSceneGraphFrom(tx, owner.scene_id)
      const result: RecordedSessionInteraction = {
        sessionId: input.sessionId,
        sceneId: owner.scene_id,
        taskId: owner.task_id,
        workspaceId: owner.workspace_id,
        interactionKind: input.interactionKind,
        sequence,
        graph
      }
      emit({
        eventId: `${command.commandId}:session-user-interacted`,
        eventType: 'session.user-interacted',
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

  projectSceneGraph(sceneId: string, windowId?: string): SceneSessionGraph {
    return projectSceneGraphFrom(this.#database, sceneId, windowId)
  }
}
