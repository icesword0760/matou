import { afterEach, describe, expect, it, vi } from 'vitest'
import { EventEmitter } from 'node:events'
import { createServer, type Server } from 'node:http'
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import type { SubscriptionProvider } from '@matou/contracts'
import { SubscriptionAccounts } from './subscription-accounts'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { ModelRouterManager, type RouterConnection, type RouterLease } from './manager'
import { RouterManagementClient } from './management-client'
import { RouterProcessOwner } from './process-owner'
import { OAuthLoopbackFlow } from './oauth'

const project = resolve(import.meta.dirname, '../../../..')
const output = join(project, 'output/model-router-stage3')
const disposals: Array<() => Promise<void>> = []
afterEach(async () => { try { while (disposals.length) await disposals.pop()!() } finally { vi.restoreAllMocks() } })
const startCallbackFlow = OAuthLoopbackFlow.start.bind(OAuthLoopbackFlow)
async function listen(server: Server) { await new Promise<void>(done => server.listen(0, '127.0.0.1', done)); return `http://127.0.0.1:${(server.address() as { port: number }).port}` }
async function closeServer(server: Server) { server.closeAllConnections(); if (server.listening) await new Promise<void>(done => server.close(() => done())) }
async function until(check: () => boolean | Promise<boolean>, timeout = 10000) { const deadline = Date.now() + timeout; while (Date.now() < deadline) { if (await check()) return; await delay(20) } throw new Error('Condition timed out') }
const mapping = { opus: 'gpt-6.1-sol', sonnet: 'gpt-6.1-sol', haiku: 'gpt-6.1-sol' }
const googleMapping = { opus: 'gemini-4-argon-high', sonnet: 'gemini-4-argon-high', haiku: 'gemini-4-argon-high' }

/** Accepted test boundary (#8): only successful OAuth exchange/result is
 * simulated. Official auth-url/callback handling, credential upload/adoption,
 * fields/status/models/aliases and api-call all execute in the pinned binary.
 * api-call URLs are rewritten only by this test proxy to a local fake service.
 * All CONNECT attempts are rejected: no provider network/real account is used.
 */
async function fixture(options: { loginTimeoutMs?: number; callbackPort?: number; holdInitialStatus?: boolean; catalog?: 'large' | 'failed' | 'empty'; emptyDefinitions?: boolean; openBrowser?(): Promise<void> } = {}) {
  await mkdir(output, { recursive: true })
  const dataRoot = await mkdtemp(join(output, 'accounts-'))
  const store = new ProviderConfigStore(dataRoot)
  const probes: string[] = [], traffic: string[] = [], stages: string[] = [], blockedTargets: string[] = []
  let failQuota = false, failCommit = false, sameAccount = false, identity = 0, holdCallback = false
  let initialStatusEntered = false, staleStatusErrors = 0
  let catalog = options.catalog, emptyDefinitions = options.emptyDefinitions
  const successes = new Map<string, string>()
  const simulatedPending = new Set<string>()
  const upstream = createServer(async (request, response) => {
    probes.push(request.url!)
    response.setHeader('Content-Type', 'application/json')
    if (request.url!.includes('usage')) {
      if (failQuota) { response.writeHead(503); response.end('{"error":"temporary"}'); return }
      response.end(JSON.stringify({ plan_type: 'plus', rate_limit: { primary_window: { used_percent: 25, limit_window_seconds: 18000, reset_at: 1800000000 } } }))
    } else if (request.url!.includes('loadCodeAssist')) response.end(JSON.stringify({ paidTier: { id: 'g1-pro-tier', name: 'Google AI Pro' }, cloudaicompanionProject: 'fixture-project' }))
    else if (request.url!.includes('retrieveUserQuotaSummary')) response.end(JSON.stringify({ groups: [{ displayName: 'Gemini Models', buckets: [{ bucketId: 'gemini-5h', remainingFraction: .75 }] }] }))
    else if (request.url!.includes('fetchAvailableModels')) response.end(JSON.stringify({ models: { 'gemini-4-argon-high': { displayName: 'Fixture Gemini', contextWindow: 1000000, maxOutputTokens: 64000 } } }))
    else if (catalog === 'failed') { response.writeHead(503); response.end('{"error":"fixture model read failed"}') }
    else if (catalog === 'empty') response.end('{"models":[]}')
    else if (catalog === 'large') response.end(JSON.stringify({ models: ['gpt-6.1-sol', 'gpt-6-astra', 'gpt-6.1-terra', 'gpt-6-sol', 'gpt-reserve', 'gpt-6-luna', 'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna', 'codex-auto-review'].map(slug => ({ slug, display_name: 'Incorrect account label', context_window: 999999, max_output_tokens: 999999, instructions: 'model instruction '.repeat(4200) })) }))
    else response.end(JSON.stringify({ models: [{ id: 'gpt-6.1-sol', context_window: 400000, max_output_tokens: 128000 }, { id: 'gemini-4-argon-high', context_window: 1000000, max_output_tokens: 64000 }] }))
  })
  const upstreamUrl = await listen(upstream)
  const rejectProxy = createServer(async (request, response) => {
    if (!request.url?.startsWith(upstreamUrl + '/')) { response.writeHead(502); response.end(); return }
    const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
    const result = await fetch(request.url, { method: request.method!, headers: { 'Content-Type': 'application/json' }, ...(chunks.length ? { body: Buffer.concat(chunks) } : {}) })
    response.writeHead(result.status, { 'Content-Type': 'application/json' }); response.end(await result.text())
  })
  rejectProxy.on('connect', (request, socket) => { blockedTargets.push(request.url!); socket.end('HTTP/1.1 502 Fixture blocks external traffic\r\nContent-Length: 0\r\n\r\n') })
  const outboundProxyUrl = await listen(rejectProxy)
  const owner = new RouterProcessOwner({ executable: process.execPath, supervisor: join(project, 'apps/runtime/dist/model-router-supervisor.cjs'), binary: join(project, `.cache/model-router/8.0.69/darwin-${process.arch}/model-router`), dataRoot })
  let real: RouterConnection | undefined
  const manager = new ModelRouterManager({ dataRoot, control: owner, idleMs: 30000, resolveProxy: async () => ({ source: 'system', proxyUrl: outboundProxyUrl }) })
  const directAcquire = manager.acquire.bind(manager)
  const direct = async (path: string, method = 'GET', body?: unknown) => {
    if (!real) throw new Error('Router not started')
    return fetch(`${real.baseUrl}/v8/management${path}`, { method, headers: { Authorization: `Bearer ${real.managementKey}`, 'Content-Type': 'application/json' }, ...(body === undefined ? {} : { body: JSON.stringify(body) }) })
  }
  const upload = async (provider: SubscriptionProvider, name: string) => {
    const claims = { email: `${provider}-${identity}@example.invalid`, 'https://api.openai.com/auth': { chatgpt_account_id: `fixture-${identity}`, chatgpt_plan_type: 'plus' } }
    const jwt = `${Buffer.from('{}').toString('base64url')}.${Buffer.from(JSON.stringify(claims)).toString('base64url')}.fake`
    const response = await direct(`/credentials?${new URLSearchParams({ name })}`, 'POST', { type: provider, ...(provider === 'antigravity' ? { base_url: upstreamUrl } : {}), email: claims.email, account_id: `fixture-${identity}`, plan_type: 'plus', project_id: 'fixture-project', access_token: 'synthetic-access-token', refresh_token: 'synthetic-refresh-token', id_token: jwt, expired: '2099-01-01T00:00:00Z' })
    if (!response.ok) throw new Error(`Fixture upload failed ${response.status}: ${await response.text()}`)
    await response.body?.cancel()
  }
  const proxy = createServer(async (request, response) => {
    try {
      const path = request.url!.replace('/v8/management', ''), url = new URL(request.url!, 'http://fixture')
      const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
      let body: unknown = chunks.length ? JSON.parse(Buffer.concat(chunks).toString('utf8')) : undefined
      traffic.push(`${request.method} ${url.pathname}`)
      if (emptyDefinitions && url.pathname.endsWith('/routing/model-definitions/codex')) { response.setHeader('Content-Type', 'application/json'); response.end('{"models":[]}'); return }
      if (holdCallback && request.method === 'DELETE' && url.pathname.endsWith('/oauth/session') && simulatedPending.has(url.searchParams.get('state')!)) {
        response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify({ status: 'ok', cancelled: false })); return
      }
      if (url.pathname.endsWith('/oauth/status') && simulatedPending.has(url.searchParams.get('state')!)) {
        response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify({ status: 'wait' })); return
      }
      if (url.pathname.endsWith('/oauth/status') && successes.has(url.searchParams.get('state')!)) {
        response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify({ status: 'ok', auth_id: successes.get(url.searchParams.get('state')!) })); return
      }
      if (options.holdInitialStatus && url.pathname.endsWith('/oauth/status')) {
        initialStatusEntered = true
        // Deterministically reproduce the load-dependent stale in-flight poll:
        // it entered before callback adoption, but reads the real cancelled
        // exchange only after the fixture has installed its synthetic success.
        await until(() => successes.has(url.searchParams.get('state')!))
      }
      if (url.pathname.endsWith('/requests/api-call')) {
        const input = body as { url: string }; const target = new URL(input.url)
        input.url = `${upstreamUrl}${target.pathname}${target.search}`
      }
      if (failCommit && request.method === 'PATCH' && url.pathname.endsWith('/credentials/fields')) { response.writeHead(500); response.end('{}'); return }
      if (url.pathname.endsWith('/oauth/callback') && (body as { code?: string })?.code) simulatedPending.add((body as { state: string }).state)
      const result = await direct(path, request.method!, body)
      const text = await result.text()
      if (url.pathname.endsWith('/oauth/status') && JSON.parse(text).status === 'error') staleStatusErrors++
      // A status poll can enter before callback handling claims the simulated
      // exchange, then return from the real engine after we cancel that real
      // exchange. Recheck at response time too: its now-stale error must not
      // race the synthetic success and terminate the local callback flow.
      if (url.pathname.endsWith('/oauth/status')) {
        const state = url.searchParams.get('state')!
        if (successes.has(state) || simulatedPending.has(state)) {
          response.setHeader('Content-Type', 'application/json')
          response.end(JSON.stringify(successes.has(state) ? { status: 'ok', auth_id: successes.get(state)! } : { status: 'wait' }))
          return
        }
      }
      if (url.pathname.endsWith('/oauth/callback') && result.ok && (body as { code?: string })?.code) {
        if (holdCallback) await delay(150)
        const input = body as { provider: SubscriptionProvider; state: string }
        // Cancel the official pending exchange before injecting the synthetic
        // exchange result. The callback was genuinely state-checked/accepted.
        await (await direct(`/oauth/session?${new URLSearchParams({ state: input.state })}`, 'DELETE')).body?.cancel()
        if (!sameAccount) identity++
        const name = `${input.provider}-fixture-${identity}.json`
        await upload(input.provider, name)
        await until(async () => (await new RouterManagementClient(real!).credentials()).some(file => file.name === name))
        successes.set(input.state, name)
        simulatedPending.delete(input.state)
        stages.push('official callback accepted -> simulated successful exchange -> official credential upload')
      }
      response.writeHead(result.status, { 'Content-Type': 'application/json' }); response.end(text)
    } catch (error) { response.writeHead(500); response.end(JSON.stringify({ error: String(error) })) }
  })
  const proxyUrl = await listen(proxy)
  manager.acquire = async (scope: string): Promise<RouterLease> => {
    const lease = await directAcquire(scope); real = lease.connection
    return { connection: { ...lease.connection, baseUrl: proxyUrl }, release: lease.release }
  }
  const opened: Array<{ provider: SubscriptionProvider; url: string }> = []
  // Real dual-loopback listeners, with OS-allocated ports. Observe the actual
  // bound port only in this fixture; provider authorization URLs stay fixed.
  const callbackPorts: Partial<Record<SubscriptionProvider, number>> = {}
  vi.spyOn(OAuthLoopbackFlow, 'start').mockImplementation(async input => {
    const flow = await startCallbackFlow(input)
    callbackPorts[input.provider] = flow.port
    return flow
  })
  const accounts = new SubscriptionAccounts({ dataRoot, manager, store, openAuthorizationUrl: async (provider, url) => { opened.push({ provider, url }); await options.openBrowser?.() }, callbackPorts: { codex: options.callbackPort ?? 0, antigravity: 0 }, ...(options.loginTimeoutMs ? { loginTimeoutMs: options.loginTimeoutMs } : {}) })
  const port = new EventEmitter(), connection = accounts.bindConnection(port)
  disposals.push(async () => { await accounts.close(); await manager.close(); await Promise.all([closeServer(proxy), closeServer(upstream), closeServer(rejectProxy)]) })
  async function login(provider: SubscriptionProvider = 'codex') {
    const start = await connection.start({ provider })
    if (options.holdInitialStatus) await until(() => initialStatusEntered)
    const authorization = new URL(opened.at(-1)!.url)
    const callback = new URL(authorization.searchParams.get('redirect_uri')!)
    callback.hostname = '127.0.0.1'
    callback.port = String(callbackPorts[provider]!)
    callback.searchParams.set('state', authorization.searchParams.get('state')!)
    callback.searchParams.set('code', 'synthetic-code')
    const response = await fetch(callback)
    expect(response.status, await response.text()).toBe(200)
    await until(() => { const view = connection.status(start.attemptId); return ['ready', 'cancelled'].includes(view.status) || (view.status === 'failed' && !view.canReopen) })
    expect(connection.status(start.attemptId).status).toBe('ready')
    stages.push(`${provider}: start -> callback -> status ok -> ready`)
    return start.attemptId
  }
  async function save(attemptId: string, provider: SubscriptionProvider = 'codex') {
    return connection.save({ kind: 'subscription', cli: 'claude-code', name: 'My subscription', modelMapping: provider === 'codex' ? mapping : googleMapping, subscription: { provider, attemptId } })
  }
  return { dataRoot, store, manager, accounts, connection, port, opened, probes, stages, traffic, blockedTargets, callbackPorts, staleStatusErrors: () => staleStatusErrors, upload, direct, login, save,
    setCatalog: (value: typeof catalog) => { catalog = value }, setEmptyDefinitions: (value: boolean) => { emptyDefinitions = value },
    setFailQuota: () => { failQuota = true }, setFailCommit: (value: boolean) => { failCommit = value }, sameAccount: () => { sameAccount = true }, holdCallback: () => { holdCallback = true } }
}

describe.runIf(process.env.MATOU_TEST_MODEL_ROUTER === '1')('subscription adoption with pinned official engine and simulated exchange only', () => {
  it.each(['large', 'failed'] as const)('uses routed conversation candidates and authoritative metadata when the account catalog is %s', async catalog => {
    const f = await fixture({ catalog }), attemptId = await f.login()
    const models = f.connection.status(attemptId).details!.models
    expect(models.map(model => model.id)).toEqual(expect.arrayContaining(['gpt-6-astra', 'gpt-6.1-sol', 'gpt-6-luna']))
    expect(models.some(model => /image|video|codex-auto-review/i.test(model.id))).toBe(false)
    expect(models.find(model => model.id === 'gpt-6-astra')).toMatchObject({ displayName: 'GPT 6.0 Astra', contextWindow: 272000, maxOutputTokens: 128000 })
    expect(JSON.stringify(models)).not.toMatch(/instruction|Incorrect account label|999999/)
  }, 30000)
  it('re-reads empty draft models without a new OAuth or quota request, and keeps attempt ownership', async () => {
    const f = await fixture({ catalog: 'empty', emptyDefinitions: true }), attemptId = await f.login()
    expect(f.connection.status(attemptId).details!.models).toEqual([])
    const quotaReads = f.probes.filter(path => path.includes('usage')).length, browserOpens = f.opened.length
    f.setCatalog('large'); f.setEmptyDefinitions(false)
    const details = await f.connection.models({ attemptId })
    expect(details.models.map(model => model.id)).toContain('gpt-6-astra')
    expect(f.connection.status(attemptId).details!.models).toEqual(details.models)
    expect(f.probes.filter(path => path.includes('usage'))).toHaveLength(quotaReads)
    expect(f.opened).toHaveLength(browserOpens)
    const other = f.accounts.bindConnection(new EventEmitter())
    await expect(other.models({ attemptId })).rejects.toThrow('this connection')
    const saved = await f.save(attemptId)
    expect((await f.connection.models({ providerId: saved.id })).models.map(model => model.id)).toContain('gpt-6-astra')
    expect(f.probes.filter(path => path.includes('usage'))).toHaveLength(quotaReads)
    expect(f.opened).toHaveLength(browserOpens)
  }, 30000)
  it('completes start -> callback -> status ok -> aliases -> adoption -> nonsecret card snapshot, reads settings once and deletes its credential', async () => {
    const f = await fixture({ holdInitialStatus: true }), attempt = await f.login(), saved = await f.save(attempt)
    expect(f.staleStatusErrors()).toBeGreaterThan(0)
    expect(saved.kind).toBe('subscription')
    expect(saved.subscription?.account.email).toBe('codex-1@example.invalid')
    expect(f.connection.status(attempt).status).toBe('saved')
    const settings = await f.connection.settings()
    expect(settings.accounts[saved.id]?.account.plan).toBe('Plus')
    expect(settings.accounts[saved.id]?.quota?.[0]?.usedPercent).toBe(25)
    expect(settings.accounts[saved.id]?.models.some(model => model.id === 'gpt-6.1-sol')).toBe(true)
    const reads = f.probes.length
    await Promise.all([f.connection.settings(), f.connection.settings()])
    expect(f.probes).toHaveLength(reads)
    for (const secret of ['synthetic-access-token', 'synthetic-refresh-token', 'credentialRef', 'auth_index', 'managementKey', 'attemptId']) expect(JSON.stringify(settings)).not.toContain(secret)
    await f.connection.closeSettings(); f.setFailQuota()
    const withoutQuota = await f.connection.settings()
    expect(withoutQuota.accounts[saved.id]?.quota).toBeUndefined()
    expect(withoutQuota.accounts[saved.id]?.models.length).toBeGreaterThan(0)
    expect(withoutQuota.accounts[saved.id]?.account.status).toBe('ready')
    const ref = (await f.store.subscriptions())[0]!.subscription.credentialRef
    const stored = JSON.parse(await readFile(join(f.dataRoot, 'provider-config.json'), 'utf8'))
    expect(stored.version).toBe(1)
    const fields = (await (await f.direct('/credentials')).json()).files.find((file: { name: string }) => file.name === ref.name)
    expect(JSON.parse(await readFile(join(f.dataRoot, 'model-router/credentials', ref.name), 'utf8')).prefix).toBe('matou-chatgpt')
    expect(fields.disabled).toBe(false)
    const models = await (await f.direct(`/credentials/models?name=${ref.name}`)).json()
    expect(models.models.map((m: { id: string }) => m.id)).toContain('matou-chatgpt/matou-opus')
    stored.activeProviderIds['claude-code'] = saved.id
    await writeFile(join(f.dataRoot, 'provider-config.json'), JSON.stringify(stored))
    await expect(f.accounts.delete('claude-code', saved.id)).rejects.toThrow('Active provider')
    stored.activeProviderIds['claude-code'] = 'anthropic-official'
    await writeFile(join(f.dataRoot, 'provider-config.json'), JSON.stringify(stored))
    await f.accounts.delete('claude-code', saved.id)
    expect((await f.accounts.snapshot()).providers['claude-code']).toHaveLength(1)
    expect((await (await f.direct('/credentials')).json()).files.some((file: { name: string }) => file.name === ref.name)).toBe(false)
    await writeFile(join(output, 'chain-evidence.json'), JSON.stringify({ boundary: 'OAuth successful exchange/status ok is simulated in test proxy. All other management/adoption and api-call operations run in official 8.0.69 binary. Real account authorization not tested.', stages: f.stages, traffic: f.traffic, probes: f.probes, snapshot: settings.snapshot, quotaFailure: withoutQuota.accounts[saved.id], modelIds: models.models.map((m: { id: string }) => m.id) }, null, 2))
  }, 30000)
  it('replaces a different account atomically, keeps the existing card/name/mappings and handles same-account re-login cancellation', async () => {
    const f = await fixture(), first = await f.save(await f.login())
    const oldRef = (await f.store.subscriptions())[0]!.subscription.credentialRef.name
    const second = await f.save(await f.login())
    expect(second.id).toBe(first.id)
    expect(second.name).toBe(first.name)
    expect(second.modelMapping).toEqual(first.modelMapping)
    expect((await f.store.subscriptions())[0]!.subscription.credentialRef.name).not.toBe(oldRef)
    expect((await (await f.direct('/credentials')).json()).files.some((row: { name: string }) => row.name === oldRef)).toBe(false)
    f.sameAccount()
    const sameAttempt = await f.login()
    await f.connection.cancel(sameAttempt)
    expect((await f.store.subscriptions())[0]!.id).toBe(first.id)
    expect((await (await f.direct('/credentials')).json()).files).toHaveLength(1)
    const currentFile = (await (await f.direct('/credentials')).json()).files[0]
    expect(currentFile.disabled).toBe(false)
    expect((await (await f.direct(`/credentials/models?name=${currentFile.name}`)).json()).models.map((model: { id: string }) => model.id)).toContain('matou-chatgpt/matou-opus')
  }, 30000)
  it('cancels before and after callback, enforces owner binding and removes unadopted drafts when the settings connection closes', async () => {
    const f = await fixture(), start = await f.connection.start({ provider: 'codex' })
    const other = f.accounts.bindConnection(new EventEmitter())
    expect(() => other.status(start.attemptId)).toThrow('this connection')
    await expect(other.start({ provider: 'codex' })).rejects.toThrow('another window')
    await f.connection.cancel(start.attemptId)
    expect(f.connection.status(start.attemptId).status).toBe('cancelled')
    expect((await (await f.direct('/credentials')).json()).files).toHaveLength(0)
    const after = await f.login()
    await f.connection.cancel(after)
    expect((await (await f.direct('/credentials')).json()).files).toHaveLength(0)
    await f.login()
    f.port.emit('close')
    await until(async () => (await (await f.direct('/credentials')).json()).files.length === 0)
  }, 30000)
  it('returns waiting before browser launch completes and ignores a late failure after cancellation', async () => {
    let fail!: (error: Error) => void
    const browser = new Promise<void>((_resolve, reject) => { fail = reject })
    const f = await fixture({ openBrowser: () => browser })
    const start = await f.connection.start({ provider: 'codex' })
    expect(start.status).toBe('waiting')
    expect(f.opened).toHaveLength(1)
    await f.connection.cancel(start.attemptId)
    fail(new Error('Injected browser failure'))
    await delay(20)
    expect(f.connection.status(start.attemptId)).toMatchObject({ status: 'cancelled' })
    expect(f.connection.status(start.attemptId).canReopen).toBeUndefined()
  }, 15000)
  it('keeps browser failures recoverable on the same attempt and original timeout', async () => {
    const openBrowser = vi.fn().mockRejectedValueOnce(new Error('private browser diagnostic')).mockResolvedValue(undefined)
    const f = await fixture({ openBrowser, loginTimeoutMs: 2000 })
    const start = await f.connection.start({ provider: 'codex' })
    await until(() => f.connection.status(start.attemptId).status === 'failed')
    expect(f.connection.status(start.attemptId)).toMatchObject({ status: 'failed', canReopen: true, reason: '浏览器没有打开', reasonLocalized: { zh: '浏览器没有打开', en: "The browser didn't open" } })
    expect(JSON.stringify(f.connection.status(start.attemptId))).not.toContain('private browser diagnostic')
    const other = f.accounts.bindConnection(new EventEmitter())
    await expect(other.start({ provider: 'codex' })).rejects.toThrow('another window')
    await delay(800)
    expect(await f.connection.reopen(start.attemptId)).toEqual(start)
    expect(f.opened[1]).toEqual(f.opened[0])
    expect(f.traffic.filter(line => line.includes('/oauth/auth-url'))).toHaveLength(1)
    await until(() => f.connection.status(start.attemptId).status === 'expired', 1600)
    expect(f.connection.status(start.attemptId).canReopen).toBeUndefined()
    await expect(f.connection.reopen(start.attemptId)).rejects.toThrow('no longer waiting')
  }, 15000)
  it('accepts a callback even when opening the browser reported failure', async () => {
    const f = await fixture({ openBrowser: async () => { throw new Error('browser unavailable') } })
    // The helper sends the real callback to the original flow and checks ready.
    const attemptId = await f.login()
    expect(f.connection.status(attemptId)).toMatchObject({ status: 'ready' })
    expect(f.connection.status(attemptId).canReopen).toBeUndefined()
  }, 15000)
  it('replaces an unacknowledged same-owner login only after releasing its listener', async () => {
    const reservation = createServer(), address = new URL(await listen(reservation))
    const callbackPort = Number(address.port)
    await closeServer(reservation)
    const f = await fixture({ callbackPort })
    const first = await f.connection.start({ provider: 'codex' })
    const second = await f.connection.start({ provider: 'codex' })
    expect(second.attemptId).not.toBe(first.attemptId)
    expect(f.connection.status(first.attemptId).status).toBe('cancelled')
    expect(second.status).toBe('waiting')
    expect(f.callbackPorts.codex).toBe(callbackPort)
    expect(f.traffic.filter(line => line.includes('/oauth/auth-url'))).toHaveLength(2)
    await f.connection.cancel(second.attemptId)
    expect(f.manager.status.leases).toBe(0)
  }, 15000)
  it('reopens only the owning waiting attempt with the original URL/state and original timeout', async () => {
    const f = await fixture({ loginTimeoutMs: 700, callbackPort: 0 })
    const start = await f.connection.start({ provider: 'codex' })
    const url = f.opened[0]!.url, other = f.accounts.bindConnection(new EventEmitter())
    await expect(other.reopen(start.attemptId)).rejects.toThrow('this connection')
    await delay(450)
    expect(await f.connection.reopen(start.attemptId)).toEqual(start)
    expect(f.opened).toEqual([{ provider: 'codex', url }, { provider: 'codex', url }])
    expect(f.traffic.filter(line => line.includes('/oauth/auth-url'))).toHaveLength(1)
    expect(JSON.stringify(start)).not.toContain(url)
    await until(() => f.connection.status(start.attemptId).status === 'expired', 500)
    await expect(f.connection.reopen(start.attemptId)).rejects.toThrow('no longer waiting')
    expect(f.opened).toHaveLength(2)
  }, 15000)
  it('publishes router-not-running during the one restart and after restart exhaustion without changing the selected card', async () => {
    const f = await fixture(), saved = await f.save(await f.login())
    await f.store.activate('claude-code', saved.id)
    const snapshots: Promise<string | undefined>[] = []
    const unsubscribe = f.accounts.onChange(() => { snapshots.push(f.accounts.snapshot().then(snapshot => snapshot.providers['claude-code'].find(row => row.id === saved.id)?.subscription?.account.status)) })
    const lease = await f.manager.acquire('status-test')
    try {
      const first = f.manager.status.pid!
      process.kill(first, 'SIGKILL')
      await until(() => f.manager.status.pid !== undefined && f.manager.status.pid !== first)
      await until(async () => (await f.accounts.snapshot()).providers['claude-code'].find(row => row.id === saved.id)?.subscription?.account.status === 'ready')
      expect(await Promise.all(snapshots)).toContain('router-not-running')
      process.kill(f.manager.status.pid!, 'SIGKILL')
      await until(() => f.manager.status.state === 'failed')
      expect((await f.accounts.snapshot()).providers['claude-code'].find(row => row.id === saved.id)?.subscription?.account.status).toBe('router-not-running')
      expect((await f.store.snapshot()).activeProviderIds['claude-code']).toBe(saved.id)
      await delay(150)
      expect(f.manager.status.pid).toBeUndefined()
      await writeFile(join(project, 'output/model-router-stage4/router-status.json'), JSON.stringify({ snapshots: await Promise.all(snapshots), final: await f.accounts.snapshot(), state: f.manager.status }, null, 2))
    } finally { unsubscribe(); lease.release() }
  }, 20000)
  it('does not adopt a credential when cancellation races the callback commit', async () => {
    const f = await fixture(); f.holdCallback()
    const start = await f.connection.start({ provider: 'codex' })
    const authorization = new URL(f.opened.at(-1)!.url), callback = new URL(authorization.searchParams.get('redirect_uri')!)
    callback.hostname = '127.0.0.1'; callback.port = String(f.callbackPorts.codex!); callback.searchParams.set('state', authorization.searchParams.get('state')!); callback.searchParams.set('code', 'synthetic-code')
    const browser = fetch(callback)
    await until(() => f.traffic.includes('POST /v8/management/oauth/callback'))
    await f.connection.cancel(start.attemptId)
    expect((await browser).status).toBe(410)
    await until(() => f.stages.length > 0)
    await delay(300)
    expect((await f.accounts.snapshot()).providers['claude-code']).toHaveLength(1)
    expect((await (await f.direct('/credentials')).json()).files).toHaveLength(0)
  }, 30000)
  it('restores the old credential and card when preparing the replacement fails, then reconciles an orphan and a missing adopted file on restart', async () => {
    const f = await fixture(), first = await f.save(await f.login())
    const oldRef = (await f.store.subscriptions())[0]!.subscription.credentialRef.name
    const replacing = await f.login(); f.setFailCommit(true)
    await expect(f.save(replacing)).rejects.toThrow()
    expect((await f.store.subscriptions())[0]!.subscription.credentialRef.name).toBe(oldRef)
    expect((await (await f.direct('/credentials')).json()).files.find((file: { name: string }) => file.name === oldRef).disabled).toBe(false)
    f.setFailCommit(false)
    await f.connection.cancel(replacing)
    await f.upload('codex', 'orphan.json')
    const restarted = new SubscriptionAccounts({ dataRoot: f.dataRoot, manager: f.manager, store: f.store, openAuthorizationUrl: async () => {} })
    await restarted.initialize()
    expect((await (await f.direct('/credentials')).json()).files.map((row: { name: string }) => row.name)).toEqual([oldRef])
    await restarted.close()
    await (await f.direct(`/credentials?name=${oldRef}`, 'DELETE')).body?.cancel()
    const missing = new SubscriptionAccounts({ dataRoot: f.dataRoot, manager: f.manager, store: f.store, openAuthorizationUrl: async () => {} })
    await missing.initialize()
    expect((await missing.snapshot()).providers['claude-code'].find(row => row.id === first.id)?.subscription?.account.status).toBe('needs-sign-in')
    await missing.delete('claude-code', first.id)
    expect(await f.store.subscriptions()).toHaveLength(0)
    await missing.close()
  }, 30000)
  it('requires persistent Antigravity risk confirmation and uses separate aliases/prefix/models for the two subscription types', async () => {
    const f = await fixture()
    await expect(f.connection.start({ provider: 'antigravity' })).rejects.toThrow('risk confirmation')
    await f.connection.confirmRisk(true)
    const google = await f.save(await f.login('antigravity'), 'antigravity')
    expect(google.subscription?.provider).toBe('antigravity')
    expect(google.subscription?.account.plan).toBe('Google AI Pro')
    await f.save(await f.login())
    expect((await f.store.subscriptions())).toHaveLength(2)
    expect(await new ProviderConfigStore(f.dataRoot).antigravityRiskAccepted()).toBe(true)
    const settings = await f.connection.settings()
    expect(settings.accounts[google.id]?.quota?.[0]?.usedPercent).toBe(25)
    expect(settings.accounts[google.id]?.models.map(model => model.id)).not.toContain('gpt-6.1-sol')
  }, 30000)
  it('expires a pending real router login and reports an occupied callback port without leaving a lease', async () => {
    const f = await fixture({ loginTimeoutMs: 80 })
    const start = await f.connection.start({ provider: 'codex' })
    await until(() => f.connection.status(start.attemptId).status === 'expired' && f.manager.status.leases === 0)
    expect(f.connection.status(start.attemptId).reasonLocalized).toEqual({ zh: '登录已超时，请重试。', en: 'Sign-in timed out. Try again.' })
    expect(f.manager.status.leases).toBe(0)
    const busy = createServer(); const base = await listen(busy)
    const g = await fixture({ callbackPort: Number(new URL(base).port) })
    try { await expect(g.connection.start({ provider: 'codex' })).rejects.toThrow('登录端口'); expect(g.manager.status.leases).toBe(0) }
    finally { await closeServer(busy) }
  }, 30000)
})
