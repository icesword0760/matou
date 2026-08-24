import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { DetachedSessionService } from './detached-session-service'
import { HierarchyApplicationService } from './hierarchy-application-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let detached: DetachedSessionService

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-detached-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  detached = new DetachedSessionService(database, transactions)
})

afterEach(() => database.close())

describe('DetachedSessionService', () => {
  it('returns the same Session and mount to its original Scene', () => {
    const initial = bootstrap()
    const moved = detached.detach(command('detach'), {
      mainWindowId: 'window-1', sceneWindowId: 'detached-1',
      sceneId: initial.scene!.id, mountId: initial.mount!.id,
      sessionId: initial.session!.id, nativeWindowKey: 'native-1', now: 10
    })
    expect(moved).toMatchObject({
      sessionId: initial.session!.id, mountId: initial.mount!.id,
      sceneId: initial.scene!.id, state: 'detached'
    })

    const returned = detached.returnSession(command('return'), {
      sceneWindowId: 'detached-1', mainWindowId: 'window-1', now: 11
    })
    expect(returned).toMatchObject({
      sessionId: initial.session!.id, mountId: initial.mount!.id,
      sceneId: initial.scene!.id, state: 'attached'
    })
    expect(database.get<{ scene_window_id: string | null }>(
      'SELECT scene_window_id FROM session_mounts WHERE id = ?', initial.mount!.id
    )?.scene_window_id).toBeNull()
  })

  it('normalizes detached mounts back to attached on restart', () => {
    const initial = bootstrap()
    detached.detach(command('detach'), {
      mainWindowId: 'window-1', sceneWindowId: 'detached-1',
      sceneId: initial.scene!.id, mountId: initial.mount!.id,
      sessionId: initial.session!.id, nativeWindowKey: 'native-1', now: 10
    })

    expect(detached.normalizeOnStartup(20)).toEqual([initial.session!.id])
    expect(database.get<{ scene_window_id: string | null }>(
      'SELECT scene_window_id FROM session_mounts WHERE id = ?', initial.mount!.id
    )?.scene_window_id).toBeNull()
  })
})

function bootstrap() {
  return hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: '/tmp/detached-workspace',
    defaultName: 'detached-workspace', now: 1
  })
}
function command(commandId: string) {
  return { commandId, commandType: 'test', requestHash: `hash-${commandId}` }
}
