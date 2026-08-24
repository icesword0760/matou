import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { NavigationRepository } from './navigation-repository'
import { HierarchyApplicationService } from './hierarchy-application-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let navigation: NavigationRepository

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-navigation-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  hierarchy = new HierarchyApplicationService(database, new DomainTransactionManager(database))
  navigation = new NavigationRepository(database)
})

afterEach(() => database.close())

describe('NavigationRepository', () => {
  it('projects independent per-window focus and one placement for each Task', () => {
    const initial = hierarchy.bootstrapWindow(command('bootstrap'), {
      windowId: 'window-1', defaultRootDirectory: '/tmp/matou-nav',
      defaultName: 'matou-nav', now: 1
    })
    hierarchy.activateWorkspace({
      windowId: 'window-2', workspaceId: initial.workspace!.id, now: 2
    })

    expect(navigation.get('window-1')).toMatchObject({
      windowId: 'window-1', activeWorkspaceId: initial.workspace!.id
    })
    expect(navigation.get('window-2')).toMatchObject({
      windowId: 'window-2', activeWorkspaceId: initial.workspace!.id
    })
    expect(navigation.listTaskPlacements()).toEqual([
      expect.objectContaining({ windowId: 'window-1', taskId: initial.task!.id, ordinal: 0 })
    ])
  })
})

function command(commandId: string) {
  return { commandId, commandType: 'test', requestHash: `hash-${commandId}` }
}
