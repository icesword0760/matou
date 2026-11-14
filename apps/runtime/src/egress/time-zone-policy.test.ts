import { expect, it } from 'vitest'
import { agentTimeZone, isMainlandTimeZone, nonMainlandUtc8Zone } from './time-zone-policy'

it('identifies mainland China, Hong Kong and Macau time zones', () => {
  for (const zone of [
    'Asia/Shanghai',
    'Asia/Chongqing',
    'Asia/Chungking',
    'Asia/Harbin',
    'Asia/Urumqi',
    'Asia/Kashgar',
    'PRC',
    'Asia/Hong_Kong',
    'Hongkong',
    'Asia/Macau',
    'Asia/Macao'
  ]) {
    expect(isMainlandTimeZone(zone)).toBe(true)
  }

  for (const zone of [
    'Asia/Taipei',
    'Asia/Singapore',
    'Asia/Tokyo',
    'Europe/Berlin',
    'America/Los_Angeles',
    'UTC',
    ''
  ]) {
    expect(isMainlandTimeZone(zone)).toBe(false)
  }
})

it('maps exit country to a non-mainland UTC+8 time zone', () => {
  expect(nonMainlandUtc8Zone('TW')).toBe('Asia/Taipei')
  expect(nonMainlandUtc8Zone('MY')).toBe('Asia/Kuala_Lumpur')
  expect(nonMainlandUtc8Zone('PH')).toBe('Asia/Manila')
  expect(nonMainlandUtc8Zone('US')).toBe('Asia/Singapore')
  expect(nonMainlandUtc8Zone(undefined)).toBe('Asia/Singapore')
})

it('computes effective agent time zone per profile, setting and system zone', () => {
  expect(
    agentTimeZone({
      profile: 'claude-code',
      setting: 'auto',
      systemTimeZone: 'Asia/Shanghai',
      exitCountry: 'US'
    })
  ).toBe('Asia/Singapore')

  expect(
    agentTimeZone({
      profile: 'claude-code',
      setting: 'auto',
      systemTimeZone: 'Asia/Shanghai',
      exitCountry: 'TW'
    })
  ).toBe('Asia/Taipei')

  expect(
    agentTimeZone({
      profile: 'claude-code',
      setting: 'auto',
      systemTimeZone: 'Europe/Berlin',
      exitCountry: 'US'
    })
  ).toBeUndefined()

  expect(
    agentTimeZone({
      profile: 'claude-code',
      setting: 'system',
      systemTimeZone: 'Asia/Shanghai'
    })
  ).toBeUndefined()

  expect(
    agentTimeZone({
      profile: 'claude-code',
      setting: 'America/Los_Angeles',
      systemTimeZone: 'Asia/Shanghai'
    })
  ).toBe('America/Los_Angeles')

  expect(
    agentTimeZone({
      profile: 'codex',
      setting: 'America/New_York',
      systemTimeZone: 'Asia/Shanghai'
    })
  ).toBe('Asia/Singapore')

  expect(
    agentTimeZone({
      profile: 'codex',
      setting: 'system',
      systemTimeZone: 'Asia/Hong_Kong',
      exitCountry: 'MY'
    })
  ).toBe('Asia/Kuala_Lumpur')

  expect(
    agentTimeZone({
      profile: 'codex',
      setting: 'auto',
      systemTimeZone: 'Europe/Berlin',
      exitCountry: 'TW'
    })
  ).toBeUndefined()
})
