import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import type { LayoutNode } from '@matou/domain'

import { HierarchyApplicationService } from './hierarchy-application-service'
import { SceneLayoutService } from './scene-layout-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let layouts: SceneLayoutService

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-scene-layout-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  layouts = new SceneLayoutService(database, transactions)
})

afterEach(() => database.close())

describe('SceneLayoutService', () => {
  it('replaces the complete Scene tree at one revision and rejects stale edits', () => {
    const initial = bootstrap()
    const root: LayoutNode = {
      id: 'layout-mount-1', kind: 'mount', mountId: initial.mount!.id
    }

    const result = layouts.replaceLayout(command('layout-2'), {
      sceneId: initial.scene!.id, expectedRevision: 1, root, now: 50
    })

    expect(result.layoutRevision).toBe(2)
    expect(layouts.getLayout(initial.scene!.id)).toEqual(root)
    expect(() => layouts.replaceLayout(command('layout-stale'), {
      sceneId: initial.scene!.id, expectedRevision: 1, root, now: 51
    })).toThrow(/revision/i)
  })

  it('requires every Scene mount exactly once in the replacement tree', () => {
    const initial = bootstrap()

    expect(() => layouts.replaceLayout(command('layout-missing'), {
      sceneId: initial.scene!.id,
      expectedRevision: 1,
      root: { id: 'missing-node', kind: 'mount', mountId: 'missing-mount' },
      now: 50
    })).toThrow('layout mounts must exactly match the Scene mounts')
  })
})

function bootstrap() {
  return hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: '/tmp/layout-workspace',
    defaultName: 'layout-workspace', now: 1
  })
}

function command(commandId: string) {
  return { commandId, commandType: 'test', requestHash: `hash-${commandId}` }
}
