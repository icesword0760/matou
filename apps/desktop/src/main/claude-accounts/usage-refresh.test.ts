import { mkdtemp, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { AccountPreferenceStore, ClaudeAccountsManager, type AccountPreferences } from './manager'
import { expiredUsageWindow } from '../../shared/account-usage'
import type { ClaudeAccountSession } from '@matou/contracts'

const account = { id: 'a', name: 'Work', email: 'wo•••@example.com', plan: 'Claude', needsLogin: false }
async function setup() {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-10-14T04:00:00Z'))
  let prefs: AccountPreferences = { names: {}, rememberScope: false }
  let currentId = 'a', sessions: ClaudeAccountSession[] = []
  const run = vi.fn(async (v: Record<string, unknown>) => {
    if (v.action === 'switch') currentId = String(v.id)
    return { accounts: [account, { ...account, id: 'b' }], currentId }
  })
  const store = { load: async () => prefs, save: vi.fn(async (v: AccountPreferences) => { prefs = v }) }
  const runtime = { request: vi.fn(async (action: string) => { if (action === 'usage-reset') sessions = []; return sessions }) }
  const manager = new ClaudeAccountsManager({ run }, store, runtime, vi.fn())
  await manager.command({ type: 'inspect' })
  const observe = (id = 'a', used = 38, observedAt = Date.now()) => {
    sessions = [{ sessionId: 's', runId: 'r', busy: true, resumable: true, defaultLogin: true,
      usage: { accountId: id, observedAt, windows: [{ label: '5h', percent: used, resetsAt: Date.now() + 20_000 }] } }]
  }
  return { manager, run, observe, runtime, store, prefs: () => prefs }
}
afterEach(() => vi.useRealTimers())
describe('local official usage collection', () => {
  it('records official responses without bridge refresh requests, even with hidden windows', async () => {
    const f = await setup(); f.observe(); await vi.advanceTimersByTimeAsync(5_000)
    expect(f.manager.state().accounts[0]?.usage).toMatchObject({ fiveHour: { used: 38 }, stale: false })
    f.manager.setUsageVisible(1, false)
    await vi.advanceTimersByTimeAsync(360_000)
    expect(f.run.mock.calls.every(([v]) => v.action !== 'refresh')).toBe(true)
    expect(f.store.save).toHaveBeenCalledTimes(1)
    expect(f.manager.state().accounts[0]?.needsLogin).toBe(false)
    await f.manager.dispose()
  })
  it('saves outgoing quota before switching and rejects cross-account or older data', async () => {
    const f = await setup(); f.observe()
    await f.manager.command({ type: 'switch', id: 'b', rememberScope: true })
    expect(f.prefs().usage?.a?.fiveHour?.used).toBe(38)
    expect(f.manager.state().accounts[0]?.usage?.stale).toBe(true)
    f.observe('a', 99); await vi.advanceTimersByTimeAsync(5_000)
    expect(f.prefs().usage?.b).toBeUndefined()
    f.observe('b', 61); await vi.advanceTimersByTimeAsync(5_000)
    expect(f.prefs().usage?.b?.fiveHour?.used).toBe(61)
    f.observe('b', 1, Date.now() - 60_000); await vi.advanceTimersByTimeAsync(5_000)
    expect(f.prefs().usage?.b?.fiveHour?.used).toBe(61)
    await f.manager.dispose()
  })
  it('keeps authoritative reset date and last percentage after reset, never invents a new window', async () => {
    const f = await setup(); f.observe(); await vi.advanceTimersByTimeAsync(5_000)
    const before = f.prefs().usage?.a
    await vi.advanceTimersByTimeAsync(30_000)
    expect(f.prefs().usage?.a).toEqual(before)
    expect(expiredUsageWindow(before?.fiveHour)).toBe(true)
    expect(f.manager.state().accounts[0]?.usage).toMatchObject({ stale: true, fiveHour: { used: 38 } })
    await f.manager.dispose()
  })
  it('manual refresh reads local state only; shutdown stops collection', async () => {
    const f = await setup(); f.observe(); await f.manager.command({ type: 'refresh-usage' })
    expect(f.run.mock.calls.at(-1)?.[0]).toEqual({ action: 'snapshot' })
    expect(f.prefs().usage?.a?.fiveHour?.used).toBe(38)
    await f.manager.dispose(); const n = f.runtime.request.mock.calls.length
    await vi.advanceTimersByTimeAsync(60_000); expect(f.runtime.request).toHaveBeenCalledTimes(n)
  })
  it('date calculation handles UTC offsets, year rollover and invalid values', () => {
    expect(expiredUsageWindow({ resetsAt: '2027-01-01T00:00:00+08:00' }, Date.parse('2026-12-31T16:00:00Z'))).toBe(true)
    expect(expiredUsageWindow({ resetsAt: '2027-01-01T00:00:00+08:00' }, Date.parse('2026-12-31T15:59:59Z'))).toBe(false)
    expect(expiredUsageWindow({ resetsAt: '' })).toBe(false)
  })
})

describe('non-secret quota persistence', () => {
  it('survives store recreation and discards a damaged cache without losing saved names', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-usage-prefs-'))
    const path = join(root, 'preferences.json')
    const value: AccountPreferences = { names: { a: 'Work' }, rememberScope: true,
      usage: { a: { checkedAt: 1791950400000, stale: false, fiveHour: { used: 81, resetsAt: '2026-10-15T01:00:00Z' } } } }
    try {
      await new AccountPreferenceStore(path).save(value)
      expect(await new AccountPreferenceStore(path).load()).toEqual(value)
      expect((await stat(path)).mode & 0o777).toBe(0o600)
      await writeFile(path, JSON.stringify({ ...value, usage: { a: { checkedAt: 'broken' } } }))
      expect(await new AccountPreferenceStore(path).load()).toEqual({ ...value, usage: {} })
    } finally { await rm(root, { recursive: true, force: true }) }
  })
})
