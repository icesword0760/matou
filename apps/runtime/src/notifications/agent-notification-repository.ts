import type { DomainCommandMetadata } from '@matou/domain'

import type { ProviderHookNotification } from '../session/provider-hook-server'
import type { RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

export interface AgentNotificationPublishInput extends ProviderHookNotification {
  eventId: string
  now: number
}

interface HierarchyOwner {
  task_id: string
  workspace_id: string
}

export class AgentNotificationRepository {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  publish(command: DomainCommandMetadata, input: AgentNotificationPublishInput): void {
    const owner = this.#database.get<HierarchyOwner>(
      `SELECT sessions.task_id, tasks.workspace_id
       FROM sessions
       JOIN tasks ON tasks.id = sessions.task_id
       WHERE sessions.id = ?`,
      input.sessionId
    )
    this.#transactions.execute(command, ({ emit }) => {
      emit({
        eventId: input.eventId,
        eventType: 'agent.notification',
        aggregateType: 'session',
        aggregateId: input.sessionId,
        ...(owner === undefined ? {} : {
          workspaceId: owner.workspace_id,
          taskId: owner.task_id
        }),
        ...(owner === undefined ? {} : { sessionId: input.sessionId }),
        payload: {
          targetSessionId: input.sessionId,
          runId: input.runId,
          provider: input.provider,
          event: input.event
        },
        schemaVersion: 1,
        occurredAt: input.now
      })
      return { published: true }
    })
  }
}
