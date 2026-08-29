import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { SessionWorkStatusService } from './session-work-status-service'

let database: RuntimeDatabase
let statuses: SessionWorkStatusService
let sessionId: string

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-work-status-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const hierarchy = new HierarchyApplicationService(database, transactions)
  const initial = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: root,
    defaultName: 'Workspace', now: 1
  })
  sessionId = initial.session!.id
  statuses = new SessionWorkStatusService(database, transactions)
})

afterEach(() => database.close())

describe('SessionWorkStatusService', () => {
  it('publishes the refreshed graph when real work changes state', () => {
    const running = statuses.set(command('running'), {
      sessionId, workStatus: 'running', now: 2
    })
    expect(running).toMatchObject({
      sessionId, previousStatus: 'idle', workStatus: 'running'
    })
    expect(running.graph.nodes.find((node) => node.sessionId === sessionId))
      .toMatchObject({ workStatus: 'running' })

    const idle = statuses.set(command('idle'), {
      sessionId, workStatus: 'idle', now: 3
    })
    expect(idle.graph.nodes.find((node) => node.sessionId === sessionId))
      .toMatchObject({ workStatus: 'idle' })
    expect(database.all<{ event_type: string }>(
      `SELECT event_type FROM domain_events
       WHERE event_type = 'session.graph-summary-changed'`
    )).toHaveLength(2)
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'work-status', requestHash: `hash-${commandId}` }
}
