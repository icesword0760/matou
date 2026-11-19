import { afterEach, expect, it, vi } from 'vitest'
const ledger = vi.hoisted(() => ({ rows: [] as unknown[] }))
vi.mock('node:fs', () => ({ statSync: () => ({ size: 1, isFile: () => true }),
  readFileSync: () => JSON.stringify(ledger.rows) }))
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); vi.unstubAllEnvs() })

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


it('limits the actual bridge shell identity and instructions to its live foreground agent', async () => {
  vi.useFakeTimers()
  vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true })))
  for (const [key, value] of Object.entries({
    MATOU_CONTROL_PROTOCOL: '1', MATOU_CONTROL_CALLER_RUN: 'managed-run', MATOU_CONTROL_CALLER_SESSION: 'card',
    MATOU_CONTROL_ENDPOINT: '/fixture/control.sock', MATOU_CONTROL_TOKEN: 'fixture-private-control', MATOU_CONTROL_NODE_EXECUTABLE: '/node'
  })) vi.stubEnv(key, value)
  const bridge = await import(new URL('../../control-assets/providers/dsh/bridge.mjs', import.meta.url).href)
  const sections = new Map<object, any>()
  const createAgent = (id: string, extra = {}) => {
    const agent = { session: { id, header: { cwd: '/fixture', version: 4 } }, status: 'idle',
      ctx: { get: () => ({ section: (spec: unknown) => { sections.set(agent, spec); return () => sections.delete(agent) } }) }, ...extra }
    return agent
  }
  const main = createAgent('main'), parked = createAgent('parked'), child = createAgent('child', { parentAgent: main })
  const handlers = new Map<string, (...args: any[]) => unknown>(), disposers: Array<() => void> = []
  let shellSpec: any
  const effect = (factory: () => () => void) => { disposers.push(factory()) }
  ledger.rows = [{ pid: process.pid, sessionId: 'main' }]
  bridge.apply({
    on: (name: string, handler: (...args: any[]) => unknown) => { handlers.set(name, handler); return () => handlers.delete(name) },
    get: () => ({ list: () => [main, parked, child] }), provide: () => () => {}, effect,
    inject: (_services: string[], apply: (ctx: unknown) => void) => apply({ effect, get: () => ({
      register: (spec: unknown) => { shellSpec = spec; return () => {} }
    }) })
  }, { hookUrl: 'http://127.0.0.1:1/hooks/fixture', runId: 'managed-run' })
  try {
    const ownsIdentity = (agent: unknown) => Object.keys(shellSpec.resolve({ agent })).includes('DSH_MATOU_CONTROL_TOKEN')
    expect(ownsIdentity(main)).toBe(true)
    expect(ownsIdentity(parked)).toBe(false)
    expect(ownsIdentity(child)).toBe(false)
    expect(ownsIdentity({ ...main })).toBe(false)
    expect(sections.has(child)).toBe(false)
    expect(sections.get(main).text({ scope: main })).not.toBe('')
    expect(sections.get(main).text({ scope: child })).toBe('')
    ledger.rows = []
    expect(ownsIdentity(main)).toBe(false)
    expect(sections.get(main).text({ scope: main })).toBe('')
    ledger.rows = [{ pid: process.pid, sessionId: 'parked' }]
    expect(ownsIdentity(main)).toBe(false)
    expect(ownsIdentity(parked)).toBe(true)
    expect(sections.get(parked).text({ scope: parked })).not.toBe('')
    vi.stubEnv('MATOU_CONTROL_CALLER_RUN', 'old-run')
    expect(ownsIdentity(parked)).toBe(false)
    vi.stubEnv('MATOU_CONTROL_CALLER_RUN', 'managed-run')
    handlers.get('agent/disposed')!({ agent: parked })
    expect(ownsIdentity(parked)).toBe(false)
    ledger.rows = [{ pid: process.pid, sessionId: 'main' }]
    for (const dispose of [...disposers].reverse()) dispose()
    expect(ownsIdentity(main)).toBe(false)
    expect(sections.size).toBe(0)
  } finally { for (const dispose of [...disposers].reverse()) dispose() }
})
