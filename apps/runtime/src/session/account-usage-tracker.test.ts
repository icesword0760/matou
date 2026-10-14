import { describe, expect, it } from 'vitest'
import { AccountUsageTracker } from './account-usage-tracker'

describe('official quota attribution across hot swaps', () => {
  it('requires a new prompt and response after cache expiry, not stale statusLine rerenders', () => {
    let now = 1_000, id = 'a'
    const t = new AccountUsageTracker(() => now, () => id)
    t.spawn('s')
    const prompt = () => t.observe('s', { hook_event_name: 'UserPromptSubmit' })
    const response = (cost: number, pid: string, percent = 24) => t.observe('s', { prompt_id: pid, cost: { total_api_duration_ms: cost } }, [{ label: '5h', percent }])
    prompt(); response(40, 'p1')
    expect(t.snapshot('s')).toMatchObject({ accountId: 'a', observedAt: 1000, windows: [{ percent: 24 }] })
    id = 'b'; t.invalidate(); now += 1
    prompt(); response(80, 'p2')
    expect(t.snapshot('s')).toBeUndefined()
    now += 36_000; response(80, 'p2') // elapsed time alone is insufficient
    expect(t.snapshot('s')).toBeUndefined()
    prompt(); response(80, 'p3') // pre-response rerender, same previous quota
    expect(t.snapshot('s')).toBeUndefined()
    response(120, 'p3', 61)
    expect(t.snapshot('s')).toMatchObject({ accountId: 'b', windows: [{ percent: 61 }] })
    t.invalidate(); now += 36_000; response(160, 'p3') // old long task finishing
    expect(t.snapshot('s')).toBeUndefined()
  })
  it('separates processes, invalidates every old process, allows newly started CLI', () => {
    let now = 1, id = 'a'
    const t = new AccountUsageTracker(() => now, () => id)
    for (const s of ['one', 'two']) t.spawn(s)
    t.invalidate(); id = 'b'; t.spawn('new')
    t.observe('new', { hook_event_name: 'UserPromptSubmit' })
    t.observe('new', { cost: { total_api_duration_ms: 1 } }, [])
    expect(t.snapshot('new')).toMatchObject({ accountId: 'b', windows: [] })
    expect(t.snapshot('one')).toBeUndefined(); expect(t.snapshot('two')).toBeUndefined()
    now += 40_000; id = 'external-login'
    t.observe('new', { cost: { total_api_duration_ms: 2 } }, [])
    expect(t.snapshot('new')).toBeUndefined()
  })
})
