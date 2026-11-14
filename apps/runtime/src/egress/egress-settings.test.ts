import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { EgressSettings } from './egress-settings'

let root: string
let database: RuntimeDatabase

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-egress-settings-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
})

afterEach(async () => {
  database.close()
  await rm(root, { recursive: true, force: true })
})

describe('EgressSettings', () => {
  it('defaults to protectionEnabled true and country mode with empty baselines', () => {
    const settings = new EgressSettings(database)

    expect(settings.protectionEnabled()).toBe(true)
    expect(settings.rule()).toEqual({ mode: 'country', baselines: {} })
  })

  it('persists protection, rule mode, and provider baselines across database reopen', () => {
    const settings = new EgressSettings(database)
    settings.setProtection(false)
    settings.setMode('ip')
    settings.setBaseline('anthropic', { ip: '169.128.1.1', country: 'US' })
    settings.setBaseline('openai', { ip: '203.0.113.10', country: 'JP' })

    expect(settings.protectionEnabled()).toBe(false)
    expect(settings.rule()).toEqual({
      mode: 'ip',
      baselines: {
        anthropic: { ip: '169.128.1.1', country: 'US' },
        openai: { ip: '203.0.113.10', country: 'JP' }
      }
    })

    database.close()
    database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
    const reopened = new EgressSettings(database)

    expect(reopened.protectionEnabled()).toBe(false)
    expect(reopened.rule()).toEqual({
      mode: 'ip',
      baselines: {
        anthropic: { ip: '169.128.1.1', country: 'US' },
        openai: { ip: '203.0.113.10', country: 'JP' }
      }
    })
  })

  it('falls back to defaults when stored preference JSON is corrupted or invalid', () => {
    database.run(
      'INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)',
      'egress.protection',
      '{not-json',
      Date.now()
    )
    database.run(
      'INSERT INTO preferences (key, value_json, updated_at) VALUES (?, ?, ?)',
      'egress.rule',
      '{corrupt',
      Date.now()
    )

    const settings = new EgressSettings(database)
    expect(settings.protectionEnabled()).toBe(true)
    expect(settings.rule()).toEqual({ mode: 'country', baselines: {} })

    database.run(
      'UPDATE preferences SET value_json = ? WHERE key = ?',
      JSON.stringify('not-a-boolean'),
      'egress.protection'
    )
    database.run(
      'UPDATE preferences SET value_json = ? WHERE key = ?',
      JSON.stringify({
        mode: 'invalid-mode',
        baselines: {
          anthropic: { ip: 123, country: 'US' },
          openai: { ip: '1.2.3.4', country: 'US' },
          other: { ip: '5.6.7.8', country: 'JP' }
        }
      }),
      'egress.rule'
    )

    expect(settings.protectionEnabled()).toBe(true)
    expect(settings.rule()).toEqual({
      mode: 'country',
      baselines: {
        openai: { ip: '1.2.3.4', country: 'US' }
      }
    })
  })
})
