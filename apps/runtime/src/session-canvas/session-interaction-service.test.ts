import { mkdir, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { SessionCanvasService } from './session-canvas-service'
import { SessionInteractionService } from './session-interaction-service'

let database: RuntimeDatabase
let interactions: SessionInteractionService
let canvas: SessionCanvasService
let hierarchy: HierarchyApplicationService
let databasePath: string
let workspaceRoot: string

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-session-interaction-'))
  workspaceRoot = join(root, 'workspace')
  databasePath = join(root, 'data', 'matou.sqlite')
  await mkdir(workspaceRoot)
  database = RuntimeDatabase.open(databasePath)
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  wireServices()
})

afterEach(() => database.close())

describe('SessionInteractionService', () => {
  it('moves only true user interactions to the front with a persistent monotonic order', async () => {
    const initial = bootstrap()
    const second = canvas.createShellSibling(command('second'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 20
    })
    const third = canvas.createShellSibling(command('third'), {
      windowId: 'window-1', sceneId: initial.scene!.id,
      sourceSessionId: initial.session!.id, now: 21
    })

    const secondInteraction = interactions.record(command('interact-second'), {
      sessionId: second.session!.id, interactionKind: 'submit', now: 30
    })
    const thirdInteraction = interactions.record(command('interact-third'), {
      sessionId: third.session!.id, interactionKind: 'control', now: 31
    })

    expect(thirdInteraction.sequence).toBeGreaterThan(secondInteraction.sequence)
    expect(rootOrder(initial.scene!.id)).toEqual([
      third.session!.id,
      second.session!.id,
      initial.session!.id
    ])
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', initial.task!.id
    )).toEqual({ last_opened_at: 31 })

    database.close()
    database = RuntimeDatabase.open(databasePath)
    wireServices()
    expect(rootOrder(initial.scene!.id)).toEqual([
      third.session!.id,
      second.session!.id,
      initial.session!.id
    ])
  })

  it('does not advance sequence when the same command is replayed', () => {
    const initial = bootstrap()
    const request = {
      sessionId: initial.session!.id,
      interactionKind: 'provider-action' as const,
      now: 20
    }

    const first = interactions.record(command('interaction-replay'), request)
    const replay = interactions.record(command('interaction-replay'), request)

    expect(replay).toEqual(first)
    expect(sequenceValue()).toBe(first.sequence)
  })

  it.each(['click', 'output', 'draft'])(
    'rejects %s because it is not a completed user interaction',
    (interactionKind) => {
      const initial = bootstrap()
      expect(() => interactions.record(command(`invalid-${interactionKind}`), {
        sessionId: initial.session!.id,
        interactionKind: interactionKind as 'submit',
        now: 20
      })).toThrow('用户交互类型不参与会话排序')
      expect(sequenceValue()).toBe(0)
    }
  )
})

function wireServices(): void {
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  canvas = new SessionCanvasService(database, transactions)
  interactions = new SessionInteractionService(database, transactions)
}

function bootstrap() {
  return hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'workspace', now: 10
  })
}

function rootOrder(sceneId: string): string[] {
  return interactions.projectSceneGraph(sceneId).nodes
    .filter(({ parentSessionId, archivedAt }) => parentSessionId === undefined && archivedAt === undefined)
    .map(({ sessionId }) => sessionId)
}

function sequenceValue(): number {
  return database.get<{ value: number }>(
    `SELECT value FROM runtime_sequences WHERE name = 'session-user-interaction'`
  )!.value
}

function command(commandId: string) {
  return { commandId, commandType: 'session-interaction', requestHash: `hash-${commandId}` }
}
