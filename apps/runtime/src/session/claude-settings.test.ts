import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, expect, it } from 'vitest'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { RuntimeRpcRouter } from '../rpc/runtime-rpc-router'
import { RuntimeAccessPolicy } from '../storage/runtime-access-policy'
import { NotificationProjection } from '../product/experience-foundation'
import { ClaudeSettings } from './claude-settings'

let root: string, database: RuntimeDatabase
beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-time-zone-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
})
afterEach(async () => { database.close(); await rm(root, { recursive: true, force: true }) })

it('persists through RPC and database reopen, isolates Claude, and resets to inherited defaults', async () => {
  const originalTZ = process.env.TZ
  const router = new RuntimeRpcRouter(database)
  const otherWindow = new ClaudeSettings(database)
  expect(otherWindow.environment('claude-code')).toEqual({})
  await router.handle('claude-settings.set', { timeZone: 'America/Los_Angeles' })
  expect(otherWindow.environment('claude-code')).toEqual({ TZ: 'America/Los_Angeles' })
  expect(otherWindow.environment('codex')).toEqual({})
  expect(otherWindow.environment('shell')).toEqual({})
  expect(process.env.TZ).toBe(originalTZ)
  database.close()
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  const reopened = new RuntimeRpcRouter(database)
  await expect(reopened.handle('claude-settings.get', {})).resolves.toMatchObject({ timeZone: 'America/Los_Angeles' })
  await reopened.handle('claude-settings.set', { timeZone: 'system' })
  expect(new ClaudeSettings(database).environment('claude-code')).toEqual({})
})

it('rejects malformed zones without overwriting the last saved preference', async () => {
  const router = new RuntimeRpcRouter(database)
  await router.handle('claude-settings.set', { timeZone: 'UTC' })
  for (const timeZone of [null, 8, '', 'Mars/Olympus', '+08:00', ' UTC ', '../../etc/localtime']) {
    await expect(router.handle('claude-settings.set', { timeZone })).rejects.toMatchObject({ code: 'INVALID_REQUEST' })
  }
  await expect(router.handle('claude-settings.get', {})).resolves.toMatchObject({ timeZone: 'UTC' })
})

it('allows reading in storage recovery mode but blocks saving', async () => {
  const router = new RuntimeRpcRouter(database, new NotificationProjection(), { accessPolicy: new RuntimeAccessPolicy('read-only') })
  await expect(router.handle('claude-settings.get', {})).resolves.toMatchObject({ timeZone: 'system' })
  await expect(router.handle('claude-settings.set', { timeZone: 'UTC' })).rejects.toThrow()
})
