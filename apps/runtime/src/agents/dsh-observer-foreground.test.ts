import { afterEach, expect, it, vi } from 'vitest'
const ledger = vi.hoisted(() => ({ rows: [] as unknown[] }))
vi.mock('node:fs', () => ({ statSync: () => ({ size: 1, isFile: () => true }),
  readFileSync: () => JSON.stringify(ledger.rows) }))
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals() })

it('revokes background ownership before the next poll and never forwards the previous session after a switch', async () => {
  vi.useFakeTimers()
  const fetch = vi.fn(async () => ({ ok: true }))
  vi.stubGlobal('fetch', fetch)
  const bridge = await import(new URL('../../control-assets/providers/dsh/bridge.mjs', import.meta.url).href)
  const main = { session: { id: 'main', header: { cwd: '/fixture', version: 4 } }, status: 'running' }
  const other = { session: { id: 'other', header: { cwd: '/fixture', version: 4 } }, status: 'idle' }
  ledger.rows = [{ pid: process.pid, sessionId: 'main' }]
  const handlers = new Map<string, (...args: any[]) => unknown>()
  let dispose = () => {}
  bridge.apply({
    on: (name: string, handler: (...args: any[]) => unknown) => { handlers.set(name, handler); return () => handlers.delete(name) },
    get: () => ({ list: () => [main] }), provide: () => () => {},
    effect: (factory: () => () => void) => { dispose = factory() }
  }, { hookUrl: 'http://127.0.0.1:1/hooks/fixture', runId: 'run' })
  try {
    await vi.advanceTimersByTimeAsync(1)
    expect(fetch).toHaveBeenCalledTimes(1)
    // Native /bg closes input ownership; an event arriving before the 100 ms
    // ledger poll must not be attributed to the old foreground terminal.
    ledger.rows = []
    handlers.get('session/event')!(main.session, { type: 'turn/end', data: { turn: 1, reason: { kind: 'completed' } } })
    expect(fetch).toHaveBeenCalledTimes(1)
    handlers.get('agent/created')!({ agent: other })
    ledger.rows = [{ pid: process.pid, sessionId: 'other' }]
    await vi.advanceTimersByTimeAsync(100)
    expect(fetch).toHaveBeenCalledTimes(2)
    handlers.get('session/event')!(main.session, { type: 'turn/end', data: { turn: 2, reason: { kind: 'completed' } } })
    expect(fetch).toHaveBeenCalledTimes(2)
    const snapshots = fetch.mock.calls.map(call => JSON.parse((call as unknown as [unknown, { body: string }])[1].body))
    expect(snapshots.map(row => [row.session_id, row.generation])).toEqual([['main', 1], ['other', 2]])
  } finally { dispose() }
})
