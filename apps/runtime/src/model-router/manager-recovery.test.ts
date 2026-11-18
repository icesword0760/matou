import { createServer } from 'node:http'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { startClaudeRequestGateway } from '../session/claude-request-gateway'
import { ROUTER_VERSION } from './config'
import { ModelRouterManager } from './manager'
import type { RouterProcessEvent } from './process-owner'
import { SubscriptionAccounts } from './subscription-accounts'

const output = resolve(import.meta.dirname, '../../../../output/model-router-recovery')
const disposals: Array<() => Promise<void>> = []
afterEach(async () => { while (disposals.length) await disposals.pop()!() })

async function fixture() {
  await mkdir(output, { recursive: true })
  const dataRoot = await mkdtemp(join(output, 'case-'))
  disposals.push(() => rm(dataRoot, { recursive: true, force: true }))
  const ports: number[] = []
  const requests = vi.fn()
  const server = createServer((request, response) => {
    request.resume()
    if (request.url === '/v8/management/credentials') response.setHeader('x-cpa-version', ROUTER_VERSION)
    else requests()
    response.end('ready')
  })
  const listen = (port: number) => new Promise<void>((done, reject) => {
    const error = (err: Error) => reject(err)
    server.once('error', error)
    server.listen(port, '127.0.0.1', () => { server.off('error', error); done() })
  })
  const stop = async () => {
    if (!server.listening) return
    await new Promise<void>(done => { server.close(() => done()); server.closeAllConnections() })
  }
  let emit: (event: RouterProcessEvent) => void
  const manager = new ModelRouterManager({
    dataRoot, resolveProxy: async () => ({ source: 'direct' }),
    control: {
      start: async () => {
        const config = JSON.parse(await readFile(join(dataRoot, 'model-router/config.json'), 'utf8')) as { server: { port: number } }
        ports.push(config.server.port)
        await listen(config.server.port)
        emit({ state: 'spawned', pid: process.pid, restarts: 0 })
      },
      stop,
      onEvent: listener => { emit = listener; return () => {} }
    }
  })
  disposals.push(() => manager.close())
  const accounts = new SubscriptionAccounts({
    dataRoot, manager, store: new ProviderConfigStore(dataRoot), platform: 'linux', openAuthorizationUrl: vi.fn()
  })
  disposals.push(() => accounts.close())
  const crash = async () => {
    await stop()
    emit({ state: 'failed', reason: 'supervisor terminated by SIGKILL' })
  }
  const logPath = join(dataRoot, 'model-router/logs/lifecycle.log')
  const events = async () => (await readFile(logPath, 'utf8')).trim().split('\n').map(line => JSON.parse(line))
  return { accounts, manager, stop, listen, crash, ports, requests, events, logPath, emit: (event: RouterProcessEvent) => emit(event) }
}

it('restarts a failed manager on the previous configured port when acquired again', async () => {
  const { manager, crash, ports } = await fixture()
  const first = await manager.acquire('session')
  await crash()
  expect(manager.status.state).toBe('failed')
  const next = await manager.acquire('second-session')
  expect(next.connection.baseUrl).toBe(first.connection.baseUrl)
  const port = Number(new URL(first.connection.baseUrl).port)
  expect(ports).toEqual([port, port])
  first.release()
  next.release()
})

it('uses one startup for concurrent requests from an existing gateway and releases the recovery leases', async () => {
  const { accounts, manager, crash, ports, requests, events, logPath } = await fixture()
  const session = await manager.acquire('session')
  const port = Number(new URL(session.connection.baseUrl).port)
  let releaseRequests!: () => void
  const requestsFailed = new Promise<void>(resolve => { releaseRequests = resolve })
  const recoverRouter = vi.fn(async () => {
    if (recoverRouter.mock.calls.length === 3) releaseRequests()
    await requestsFailed
    await accounts.ensureRouterRunning()
  })
  const gateway = await startClaudeRequestGateway({
    upstreamUrl: session.connection.baseUrl, env: {}, resolveOverride: () => ({ routerTarget: true }), recoverRouter
  })
  disposals.push(gateway.close)
  const post = () => fetch(gateway.baseUrl + '/v1/messages', { method: 'POST', body: '{"model":"matou-role/haiku"}' })
  expect(await (await post()).text()).toBe('ready')
  await crash()
  const responses = await Promise.all([post(), post(), post()])
  expect(await Promise.all(responses.map(response => response.text()))).toEqual(['ready', 'ready', 'ready'])
  expect(requests).toHaveBeenCalledTimes(4)
  expect(recoverRouter).toHaveBeenCalledTimes(3)
  expect(ports).toEqual([port, port])
  expect(manager.status).toMatchObject({ state: 'running', leases: 1 })
  await vi.waitFor(async () => {
    const rows = await events()
    expect(rows.map(row => row.event)).toEqual(['started', 'failed', 'recovery-started', 'started', 'recovery-succeeded'])
    expect(rows[0]).toMatchObject({ timestamp: expect.any(String), port, pid: process.pid, restarts: 0 })
    expect(rows[1]).toMatchObject({ reason: 'supervisor terminated by SIGKILL' })
  })
  expect((await stat(logPath)).mode & 0o777).toBe(0o600)
  session.release()
})

it('waits through the supervisor backoff on the original port', async () => {
  const { accounts, manager, stop, listen, emit, ports, events } = await fixture()
  const session = await manager.acquire('session')
  await stop()
  const ready = vi.fn()
  const recover = accounts.ensureRouterRunning().then(ready)
  await vi.waitFor(async () => expect(await events()).toContainEqual(expect.objectContaining({ event: 'recovery-started' })))
  expect(ready).not.toHaveBeenCalled()
  await listen(ports[0]!)
  emit({ state: 'spawned', pid: process.pid, restarts: 1, reason: 'router terminated by SIGKILL' })
  await recover
  expect(ports).toHaveLength(1)
  expect(manager.status.leases).toBe(1)
  await vi.waitFor(async () => expect(await events()).toContainEqual(expect.objectContaining({
    event: 'restart', restarts: 1, reason: 'router terminated by SIGKILL'
  })))
  session.release()
})

it('falls back to a new port if another listener owns the old port and reports the new router URL', async () => {
  const { accounts, manager, crash, ports, events } = await fixture()
  const session = await manager.acquire('session')
  await crash()
  const foreign = createServer()
  await new Promise<void>((done, reject) => foreign.once('error', reject).listen(ports[0], '127.0.0.1', done))
  disposals.push(() => new Promise<void>(done => foreign.close(() => done())))
  const recovered = await accounts.ensureRouterRunning()
  expect(ports).toHaveLength(2)
  expect(ports[1]).not.toBe(ports[0])
  expect(recovered).toBe(`http://127.0.0.1:${ports[1]}`)
  expect(manager.status).toMatchObject({ state: 'running', leases: 1 })
  await vi.waitFor(async () => expect(await events()).toContainEqual(expect.objectContaining({
    event: 'recovery-succeeded', port: ports[1], previousPort: ports[0]
  })))
  session.release()
})

it('truncates an oversized lifecycle log and ignores a subsequent write failure', async () => {
  const { accounts, manager, crash, ports, logPath, events } = await fixture()
  const session = await manager.acquire('session')
  await vi.waitFor(async () => expect(await events()).toHaveLength(1))
  await writeFile(logPath, Buffer.alloc(256 * 1024, 'x'))
  await crash()
  await accounts.ensureRouterRunning()
  await vi.waitFor(async () => expect(await events()).toHaveLength(4))
  expect((await stat(logPath)).size).toBeLessThan(256 * 1024)
  await rm(logPath)
  await mkdir(logPath)
  await crash()
  await accounts.ensureRouterRunning()
  expect(new Set(ports).size).toBe(1)
  expect(manager.status).toMatchObject({ state: 'running', leases: 1 })
  session.release()
})
