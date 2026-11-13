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

it('defaults to auto, hides mainland zones for official sessions, and persists preferences across reopen', async () => {
  const originalTZ = process.env.TZ
  const router = new RuntimeRpcRouter(database)
  const mainlandWindow = new ClaudeSettings(database, { systemTimeZone: () => 'Asia/Shanghai' })

  expect(mainlandWindow.snapshot()).toEqual({
    timeZone: 'auto',
    systemTimeZone: 'Asia/Shanghai',
    autoTimeZone: 'Asia/Singapore'
  })
  expect(mainlandWindow.environment('claude-code')).toEqual({ TZ: 'Asia/Singapore' })
  expect(mainlandWindow.environment('codex', { exitCountry: 'TW', official: true })).toEqual({ TZ: 'Asia/Taipei' })
  expect(mainlandWindow.environment('shell')).toEqual({})
  expect(mainlandWindow.environment('claude-code', { official: false })).toEqual({})
  expect(mainlandWindow.environment('codex', { official: false })).toEqual({})

  const berlinWindow = new ClaudeSettings(database, { systemTimeZone: () => 'Europe/Berlin' })
  expect(berlinWindow.snapshot()).toEqual({
    timeZone: 'auto',
    systemTimeZone: 'Europe/Berlin'
  })
  expect(berlinWindow.snapshot().autoTimeZone).toBeUndefined()
  expect(berlinWindow.environment('claude-code')).toEqual({})
  expect(berlinWindow.environment('codex', { exitCountry: 'TW', official: true })).toEqual({})

  await router.handle('claude-settings.set', { timeZone: 'America/Los_Angeles' })
  expect(mainlandWindow.environment('claude-code')).toEqual({ TZ: 'America/Los_Angeles' })
  expect(mainlandWindow.environment('claude-code', { official: false })).toEqual({ TZ: 'America/Los_Angeles' })
  expect(mainlandWindow.environment('codex')).toEqual({ TZ: 'Asia/Singapore' })
  expect(mainlandWindow.environment('shell')).toEqual({})
  expect(process.env.TZ).toBe(originalTZ)

  database.close()
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  const reopened = new RuntimeRpcRouter(database)
  await expect(reopened.handle('claude-settings.get', {})).resolves.toMatchObject({ timeZone: 'America/Los_Angeles' })

  await reopened.handle('claude-settings.set', { timeZone: 'system' })
  const afterSystem = new ClaudeSettings(database, { systemTimeZone: () => 'Asia/Shanghai' })
  expect(afterSystem.environment('claude-code')).toEqual({})
  expect(afterSystem.environment('codex')).toEqual({ TZ: 'Asia/Singapore' })

  await reopened.handle('claude-settings.set', { timeZone: 'auto' })
  await expect(reopened.handle('claude-settings.get', {})).resolves.toMatchObject({ timeZone: 'auto' })
  expect(new ClaudeSettings(database, { systemTimeZone: () => 'Asia/Shanghai' }).environment('claude-code')).toEqual({
    TZ: 'Asia/Singapore'
  })
})

it('rejects malformed zones without overwriting the last saved preference', async () => {
  const router = new RuntimeRpcRouter(database)
  await router.handle('claude-settings.set', { timeZone: 'UTC' })
  for (const timeZone of [null, 8, '', 'Mars/Olympus', '+08:00', ' UTC ', '../../etc/localtime']) {
    await expect(router.handle('claude-settings.set', { timeZone })).rejects.toMatchObject({ code: 'INVALID_REQUEST' })
  }
  await expect(router.handle('claude-settings.get', {})).resolves.toMatchObject({ timeZone: 'UTC' })
})

it('reads the host process TZ when systemTimeZone is not injected', () => {
  const originalTZ = process.env.TZ
  try {
    const settings = new ClaudeSettings(database)
    process.env.TZ = 'Asia/Shanghai'
    expect(settings.snapshot()).toEqual({
      timeZone: 'auto',
      systemTimeZone: 'Asia/Shanghai',
      autoTimeZone: 'Asia/Singapore'
    })
    expect(settings.environment('claude-code')).toEqual({ TZ: 'Asia/Singapore' })
    expect(settings.environment('codex')).toEqual({ TZ: 'Asia/Singapore' })

    process.env.TZ = 'Europe/Berlin'
    expect(settings.snapshot()).toEqual({
      timeZone: 'auto',
      systemTimeZone: 'Europe/Berlin'
    })
    expect(settings.environment('claude-code')).toEqual({})
    expect(settings.environment('codex')).toEqual({})
  } finally {
    if (originalTZ === undefined) delete process.env.TZ
    else process.env.TZ = originalTZ
  }
})

it('allows reading in storage recovery mode but blocks saving', async () => {
  const router = new RuntimeRpcRouter(database, new NotificationProjection(), { accessPolicy: new RuntimeAccessPolicy('read-only') })
  await expect(router.handle('claude-settings.get', {})).resolves.toMatchObject({ timeZone: 'auto' })
  await expect(router.handle('claude-settings.set', { timeZone: 'UTC' })).rejects.toThrow()
})
