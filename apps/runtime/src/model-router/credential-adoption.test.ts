import { afterEach, describe, expect, it } from 'vitest'
import { createServer } from 'node:http'
import { mkdir, mkdtemp, readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { ModelRouterManager } from './manager'
import { ROUTER_VERSION } from './config'
import type { RouterProcessEvent } from './process-owner'

const output = resolve(import.meta.dirname, '../../../../output/model-router-stage2-rework')
const managers: ModelRouterManager[] = []
afterEach(async () => { await Promise.all(managers.splice(0).map(manager => manager.close())) })
async function fixture(entries: Array<Array<Record<string, unknown>>>, onPoll?: () => void) {
  await mkdir(output, { recursive: true })
  const dataRoot = await mkdtemp(join(output, 'adoption-'))
  const listeners = new Set<(event: RouterProcessEvent) => void>()
  let polls = 0
  const server = createServer((request, response) => {
    response.setHeader('x-cpa-version', ROUTER_VERSION)
    response.setHeader('Content-Type', 'application/json')
    const named = new URL(request.url!, 'http://localhost').searchParams.has('name')
    const files = named ? entries[Math.min(polls++, entries.length - 1)] : []
    response.end(JSON.stringify({ files }))
    if (named) onPoll?.()
  })
  const manager = new ModelRouterManager({
    dataRoot, resolveProxy: async () => ({ source: 'direct' }),
    control: {
      start: async () => {
        const config = JSON.parse(await readFile(join(dataRoot, 'model-router/config.json'), 'utf8')) as { server: { port: number } }
        await new Promise<void>(done => server.listen(config.server.port, '127.0.0.1', done))
        for (const listener of listeners) listener({ state: 'spawned', pid: process.pid, restarts: 0 })
      },
      stop: async () => { if (server.listening) await new Promise<void>(done => server.close(() => done())) },
      onEvent: listener => { listeners.add(listener); return () => listeners.delete(listener) }
    }
  })
  managers.push(manager)
  return { manager, polls: () => polls }
}
const active = { name: 'new-account.json', provider: 'codex', auth_index: 'new-index', status: 'active', disabled: false }
describe('credential adoption readiness', () => {
  it('waits for the exact credential and active state instead of merely HTTP readiness', async () => {
    const { manager, polls } = await fixture([
      [], [{ ...active, status: 'pending' }], [{ ...active, provider: 'antigravity' }],
      [{ ...active, auth_index: 'old-index' }], [active]
    ])
    await expect(manager.waitForCredentialAdoption({ name: active.name, provider: 'codex', authIndex: 'new-index' })).resolves.toEqual({ name: active.name, provider: 'codex', authIndex: 'new-index', status: 'active' })
    expect(polls()).toBe(5)
    expect(manager.status.leases).toBe(0)
  })
  it.each([{ ...active, disabled: true }, { ...active, status: 'error' }, { ...active, requires_sign_in: true }])(
    'does not declare a disabled, failed or sign-in-required credential ready', async entry => {
      const { manager } = await fixture([[entry]])
      await expect(manager.waitForCredentialAdoption({ name: active.name, provider: 'codex' }, { timeoutMs: 100 })).rejects.toThrow('not adopted before the deadline')
      expect(manager.status.leases).toBe(0)
    }
  )
  it('releases its temporary lease on cancellation', async () => {
    const abort = new AbortController()
    const { manager } = await fixture([[]], () => abort.abort(new Error('fixture cancelled')))
    await expect(manager.waitForCredentialAdoption({ name: active.name, provider: 'codex' }, { signal: abort.signal })).rejects.toThrow('fixture cancelled')
    expect(manager.status.leases).toBe(0)
  })
  it('cancels an adoption wait when the runtime manager closes', async () => {
    let close: () => void = () => {}
    const { manager } = await fixture([[]], () => close())
    close = () => { void manager.close() }
    await expect(manager.waitForCredentialAdoption({ name: active.name, provider: 'codex' })).rejects.toThrow('closed')
    expect(manager.status.leases).toBe(0)
  })
})
